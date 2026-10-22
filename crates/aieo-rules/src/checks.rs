//! T (per page) and C (cross-artifact) rules over a build's page inventory.

use std::collections::{BTreeMap, HashMap, HashSet};

use url::Url;

use aieo_core::spec::spec;
use aieo_core::text::utf16_len;
use aieo_core::{Evidence, Localized};
use aieo_site::robots::{
    evaluate_path, parse_robots, rules_for_agent, PathDecision, RobotsFile, RobotsGroup,
};
use aieo_site::sitemap::{decode_xml_entities, parse_sitemap, SitemapKind};
use aieo_site::Site;

use crate::emit::{Emitter, Loc};
use crate::llms;
use crate::pages::{PageFacts, PageKind};

fn ev(en: &str, zh: &str, detail: impl Into<String>) -> Evidence {
    Evidence::new(Localized::new(en, zh), detail)
}

/// One `<loc>` of a sitemap in the build.
#[derive(Debug, Clone)]
pub struct SitemapEntry {
    pub loc: String,
    pub sitemap_url: String,
    pub sitemap_file: String,
}

/// What the rules learned about the whole build.
#[derive(Debug, Default)]
pub struct Graph {
    /// distinct linking pages per target file
    pub inlinks: HashMap<String, u64>,
    pub in_sitemap: HashSet<String>,
    pub sitemaps: Vec<String>,
    pub sitemap_entries: Vec<SitemapEntry>,
    pub robots_txt: bool,
    pub llms_txt: bool,
}

pub struct Checker<'a> {
    pub site: &'a Site,
    pub pages: &'a [PageFacts],
    by_file: HashMap<&'a str, usize>,
    robots: Option<RobotsFile>,
    robots_file: Option<String>,
    home_path: String,
}

/// Why an hreflang value is not one Google accepts, or None: `x-default`, or
/// an ISO 639-1 language (3-letter BCP 47 languages pass on their shape),
/// optionally an ISO 15924 script, optionally an ISO 3166-1 alpha-2 region
/// or a UN M.49 area (`es-419`).
fn hreflang_problem(code: &str) -> Option<String> {
    let lower = code.to_ascii_lowercase();
    if lower == "x-default" {
        return None;
    }
    let mut parts = lower.split('-');
    let lang = parts.next().unwrap_or_default();
    let alpha = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase());
    match lang.len() {
        2 if crate::iso::ISO_639_1.binary_search(&lang).is_ok() => {}
        2 => return Some(format!("“{lang}” is not an ISO 639-1 language code")),
        3 if alpha(lang) => {}
        _ => return Some(format!("“{lang}” is not a language code")),
    }
    let mut rest: Vec<&str> = parts.collect();
    if rest.first().is_some_and(|s| s.len() == 4 && alpha(s)) {
        let script = rest.remove(0);
        let titled = format!("{}{}", script[..1].to_ascii_uppercase(), &script[1..]);
        if crate::iso::ISO_15924
            .binary_search(&titled.as_str())
            .is_err()
        {
            return Some(format!("“{titled}” is not an ISO 15924 script"));
        }
    }
    match rest.as_slice() {
        [] => None,
        [r] if r.len() == 3 && r.bytes().all(|b| b.is_ascii_digit()) => None,
        [r] if r.len() == 2 && alpha(r) => {
            let upper = r.to_ascii_uppercase();
            if crate::iso::ISO_3166_1
                .binary_search(&upper.as_str())
                .is_ok()
            {
                None
            } else if upper == "UK" {
                Some("“UK” is not an ISO 3166-1 region; the United Kingdom is “GB”".into())
            } else {
                Some(format!("“{upper}” is not an ISO 3166-1 region code"))
            }
        }
        _ => Some("expected language[-Script][-REGION]".into()),
    }
}

fn page_loc(p: &PageFacts) -> Loc {
    Loc {
        url_path: Some(p.path.clone()),
        url: Some(p.url.to_string()),
        file: Some(p.file.clone()),
        source: p.source.clone(),
    }
}

fn read_text(site: &Site, rel: &str) -> Option<String> {
    let bytes = site.read_bytes(rel)?;
    let bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(bytes.as_slice())
            .take(50_000_000)
            .read_to_end(&mut out)
            .ok()?;
        out
    } else {
        bytes
    };
    String::from_utf8(bytes).ok()
}

impl<'a> Checker<'a> {
    pub fn new(site: &'a Site, pages: &'a [PageFacts]) -> Self {
        let by_file = pages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.file.as_str(), i))
            .collect();
        let robots_path = format!("{}/robots.txt", site.base_path);
        let robots_file = match site.resolve_path(&robots_path) {
            aieo_site::Resolution::File(f) => Some(f),
            _ => None,
        };
        let robots = robots_file
            .as_deref()
            .and_then(|f| read_text(site, f))
            .map(|t| parse_robots(&t));
        Checker {
            site,
            pages,
            by_file,
            robots,
            robots_file,
            home_path: format!("{}/", site.base_path),
        }
    }

    pub fn page(&self, file: &str) -> Option<&'a PageFacts> {
        self.by_file.get(file).map(|i| &self.pages[*i])
    }

    /// The page names itself (or nothing) as canonical.
    pub fn canonical_self(&self, p: &PageFacts) -> bool {
        match &p.canonical {
            None => true,
            Some(c) => {
                self.site.is_own_url(c)
                    && self.site.follow(c).file.as_deref() == Some(p.file.as_str())
            }
        }
    }

    pub fn indexable(&self, p: &PageFacts) -> bool {
        p.kind == PageKind::Content && !p.noindex && self.canonical_self(p)
    }

    pub fn run(&self, e: &mut Emitter) -> Graph {
        let mut graph = Graph {
            robots_txt: self.robots_file.is_some(),
            ..Default::default()
        };
        self.links(e, &mut graph);
        self.page_rules(e, &graph);
        self.canonicals(e);
        self.hreflangs(e);
        self.sitemaps(e, &mut graph);
        self.robots_rules(e);
        self.llms(e, &mut graph);
        graph
    }

    fn links(&self, e: &mut Emitter, graph: &mut Graph) {
        let mut sources: HashMap<String, HashSet<&str>> = HashMap::new();
        for p in self.pages.iter().filter(|p| p.kind == PageKind::Content) {
            let mut reported = HashSet::new();
            for (href, target) in &p.links {
                let followed = self.site.follow(target);
                match followed.file {
                    Some(f) if f != p.file => {
                        sources.entry(f).or_default().insert(p.file.as_str());
                    }
                    Some(_) => {}
                    None if !followed.foreign
                        && !followed.looped
                        && followed.server.is_none()
                        && reported.insert(target.path().to_string()) =>
                    {
                        e.emit(
                            "C-LINK-BROKEN",
                            None,
                            page_loc(p),
                            target.path(),
                            vec![
                                ev("Link", "链接", format!("{href} → {target}")),
                                ev(
                                    "Build",
                                    "构建产物",
                                    "no file and no redirect answers this path",
                                ),
                            ],
                            Some(format!("href=\"{href}\"")),
                        );
                    }
                    None => {}
                }
            }
            for (href, target) in &p.markdown_alternates {
                let Some(target) = target.as_ref().filter(|t| self.site.is_own_url(t)) else {
                    continue;
                };
                if !self.site.follow(target).answers() {
                    e.emit(
                        "C-MD-ALTERNATE-MISSING",
                        None,
                        page_loc(p),
                        target.path(),
                        vec![ev(
                            "Markdown alternate",
                            "Markdown 备选",
                            target.to_string(),
                        )],
                        Some(href.clone()),
                    );
                }
            }
        }
        graph.inlinks = sources
            .into_iter()
            .map(|(k, v)| (k, v.len() as u64))
            .collect();
        for p in self
            .pages
            .iter()
            .filter(|p| self.indexable(p) && p.path != self.home_path)
        {
            if graph.inlinks.get(&p.file).copied().unwrap_or(0) == 0 {
                e.emit(
                    "C-ORPHAN-PAGE",
                    None,
                    page_loc(p),
                    "",
                    vec![ev("Inbound internal links", "站内入链", "0")],
                    None,
                );
            }
        }
    }

    fn page_rules(&self, e: &mut Emitter, _graph: &Graph) {
        let mut titles: BTreeMap<&str, Vec<&PageFacts>> = BTreeMap::new();
        for p in self.pages.iter().filter(|p| p.kind == PageKind::Content) {
            for (i, err) in p.jsonld_failures.iter().enumerate() {
                e.emit(
                    "T-JSONLD-INVALID",
                    None,
                    page_loc(p),
                    &i.to_string(),
                    vec![ev("JSON parse error", "JSON 解析错误", err.clone())],
                    Some("application/ld+json".into()),
                );
            }
            for issue in &p.jsonld_issues {
                e.emit(
                    issue.rule,
                    None,
                    page_loc(p),
                    &issue.key,
                    vec![
                        ev("Type", "类型", issue.ty.clone()),
                        match issue.rule {
                            "T-JSONLD-MISSING-REQUIRED" => {
                                ev("Missing", "缺少", issue.detail.clone())
                            }
                            "T-JSONLD-NO-CONTEXT" => {
                                ev("@context", "@context", issue.detail.clone())
                            }
                            _ => ev("Status", "状态", issue.detail.clone()),
                        },
                        ev("Reference", "参考", issue.doc.clone()),
                    ],
                    Some(format!("\"{}\"", issue.ty)),
                );
            }
            if p.bytes > 1_000_000 {
                e.emit(
                    "T-HTML-SIZE",
                    None,
                    page_loc(p),
                    "",
                    vec![ev("HTML size", "HTML 大小", format!("{} bytes", p.bytes))],
                    None,
                );
            }
            if !self.indexable(p) {
                continue;
            }
            if p.title.is_empty() {
                e.emit(
                    "T-MISSING-TITLE",
                    None,
                    page_loc(p),
                    "",
                    vec![],
                    Some("<head".into()),
                );
            } else {
                titles.entry(p.title.as_str()).or_default().push(p);
                let len = utf16_len(&p.title);
                if !(30..=60).contains(&len) {
                    e.emit(
                        "T-TITLE-LENGTH",
                        None,
                        page_loc(p),
                        "",
                        vec![ev("Title length", "标题长度", format!("{len} characters"))],
                        Some("<title".into()),
                    );
                }
            }
            if p.description.is_none() {
                e.emit(
                    "T-MISSING-META-DESCRIPTION",
                    None,
                    page_loc(p),
                    "",
                    vec![],
                    Some("<head".into()),
                );
            }
            match p.h1_count {
                0 => e.emit("T-MISSING-H1", None, page_loc(p), "", vec![], None),
                1 => {}
                n => e.emit(
                    "T-MULTIPLE-H1",
                    None,
                    page_loc(p),
                    "",
                    vec![ev("h1 count", "h1 数量", n.to_string())],
                    Some("<h1".into()),
                ),
            }
            if p.lang.is_none() {
                e.emit(
                    "T-MISSING-LANG",
                    None,
                    page_loc(p),
                    "",
                    vec![],
                    Some("<html".into()),
                );
            }
            if p.canonical_href.is_none() {
                e.emit(
                    "T-MISSING-CANONICAL",
                    None,
                    page_loc(p),
                    "",
                    vec![],
                    Some("<head".into()),
                );
            }
            if p.images_missing_alt > 0 {
                e.emit(
                    "T-IMAGES-MISSING-ALT",
                    None,
                    page_loc(p),
                    "",
                    vec![ev(
                        "Images without alt",
                        "缺少 alt 的图片",
                        p.images_missing_alt.to_string(),
                    )],
                    Some("<img".into()),
                );
            }
            if p.words < 300 && p.path != self.home_path {
                e.emit(
                    "T-THIN-CONTENT",
                    None,
                    page_loc(p),
                    "",
                    vec![ev("Visible words", "可见文字", p.words.to_string())],
                    None,
                );
            }
        }
        // language versions that name each other in hreflang are one cluster
        // to search engines, not competing duplicates
        let alternates = |p: &PageFacts| -> HashSet<String> {
            p.hreflang
                .iter()
                .filter_map(|h| h.url.as_ref())
                .filter_map(|u| self.site.follow(u).file)
                .collect()
        };
        for (title, group) in titles.into_iter().filter(|(_, g)| g.len() > 1) {
            let alts: Vec<HashSet<String>> = group.iter().map(|p| alternates(p)).collect();
            for (i, p) in group.iter().enumerate() {
                let others: Vec<String> = group
                    .iter()
                    .enumerate()
                    .filter(|(j, o)| {
                        o.file != p.file
                            && !alts[i].contains(&o.file)
                            && !alts[*j].contains(&p.file)
                    })
                    .take(5)
                    .map(|(_, o)| o.path.clone())
                    .collect();
                if others.is_empty() {
                    continue;
                }
                e.emit(
                    "T-DUPLICATE-TITLE",
                    None,
                    page_loc(p),
                    title,
                    vec![
                        ev("Title", "标题", title),
                        ev("Also used by", "也被以下页面使用", others.join(", ")),
                    ],
                    Some("<title".into()),
                );
            }
        }
    }

    fn canonicals(&self, e: &mut Emitter) {
        for p in self.pages.iter().filter(|p| p.kind == PageKind::Content) {
            let Some(c) = &p.canonical else { continue };
            let needle = Some("rel=\"canonical\"".to_string());
            if !self.site.is_own_url(c) {
                e.emit(
                    "C-CANONICAL-FOREIGN-HOST",
                    None,
                    page_loc(p),
                    c.host_str().unwrap_or_default(),
                    vec![
                        ev("Canonical", "Canonical", c.to_string()),
                        ev("Site origin", "站点来源", self.site.origin_str()),
                    ],
                    needle,
                );
                continue;
            }
            let followed = self.site.follow(c);
            let Some(target) = followed.file else {
                if !followed.looped && followed.server.is_none() {
                    e.emit(
                        "C-CANONICAL-TARGET-MISSING",
                        None,
                        page_loc(p),
                        c.path(),
                        vec![ev("Canonical", "Canonical", c.to_string())],
                        needle,
                    );
                }
                continue;
            };
            if target == p.file {
                continue;
            }
            let Some(t) = self.page(&target) else {
                continue;
            };
            if t.noindex {
                e.emit(
                    "C-CANONICAL-TARGET-NOINDEX",
                    None,
                    page_loc(p),
                    c.path(),
                    vec![
                        ev("Canonical", "Canonical", c.to_string()),
                        ev("Target", "目标", t.noindex_via.clone().unwrap_or_default()),
                    ],
                    needle,
                );
            } else if !self.canonical_self(t) {
                e.emit(
                    "C-CANONICAL-CHAIN",
                    None,
                    page_loc(p),
                    c.path(),
                    vec![
                        ev("Canonical", "Canonical", c.to_string()),
                        ev(
                            "Target's canonical",
                            "目标的 canonical",
                            t.canonical.as_ref().map(Url::to_string).unwrap_or_default(),
                        ),
                    ],
                    needle,
                );
            }
        }
    }

    fn hreflangs(&self, e: &mut Emitter) {
        for p in self
            .pages
            .iter()
            .filter(|p| p.kind == PageKind::Content && !p.hreflang.is_empty())
        {
            let mut lists_self = false;
            for h in &p.hreflang {
                let needle = Some(format!("hreflang=\"{}\"", h.code));
                if let Some(problem) = hreflang_problem(&h.code) {
                    e.emit(
                        "C-HREFLANG-INVALID-CODE",
                        None,
                        page_loc(p),
                        &h.code,
                        vec![
                            ev("hreflang", "hreflang", h.code.clone()),
                            ev("Problem", "问题", problem),
                        ],
                        needle.clone(),
                    );
                }
                let Some(target) = h.url.as_ref().filter(|u| self.site.is_own_url(u)) else {
                    continue;
                };
                let followed = self.site.follow(target);
                let Some(file) = followed.file else {
                    if followed.server.is_none() {
                        e.emit(
                            "C-HREFLANG-TARGET-MISSING",
                            None,
                            page_loc(p),
                            target.path(),
                            vec![ev("Alternate", "备选", format!("{} → {target}", h.code))],
                            needle,
                        );
                    }
                    continue;
                };
                if file == p.file {
                    lists_self = true;
                    continue;
                }
                let Some(t) = self.page(&file) else { continue };
                let returns = t
                    .hreflang
                    .iter()
                    .filter_map(|b| b.url.as_ref())
                    .any(|u| self.site.follow(u).file.as_deref() == Some(p.file.as_str()));
                if !returns {
                    e.emit(
                        "C-HREFLANG-NOT-RECIPROCAL",
                        None,
                        page_loc(p),
                        target.path(),
                        vec![ev(
                            "Alternate",
                            "备选",
                            format!("{} → {target} (no hreflang back to {})", h.code, p.path),
                        )],
                        needle,
                    );
                }
            }
            if !lists_self {
                e.emit(
                    "C-HREFLANG-SELF-MISSING",
                    None,
                    page_loc(p),
                    "",
                    vec![ev(
                        "Declared",
                        "已声明",
                        p.hreflang
                            .iter()
                            .map(|h| h.code.clone())
                            .collect::<Vec<_>>()
                            .join(", "),
                    )],
                    Some("hreflang".into()),
                );
            }
        }
    }

    fn robots_group_for(&self, agent: &str) -> Option<RobotsGroup> {
        self.robots.as_ref().and_then(|r| rules_for_agent(r, agent))
    }

    fn sitemaps(&self, e: &mut Emitter, graph: &mut Graph) {
        let base = &self.site.base_path;
        let mut queue: Vec<Url> = Vec::new();
        let declared: Vec<String> = self
            .robots
            .as_ref()
            .map(|r| r.sitemaps.clone())
            .unwrap_or_default();
        for s in &declared {
            if let Ok(u) = self.site.origin.join(s) {
                if self.site.is_own_url(&u) {
                    queue.push(u);
                }
            }
        }
        let mut found_by_default = false;
        for name in ["sitemap.xml", "sitemap-index.xml", "sitemap_index.xml"] {
            let u = Url::parse(&self.site.url_for(&format!("{base}/{name}"))).expect("site URL");
            if self.site.follow_http(&u).file.is_some() && !queue.contains(&u) {
                found_by_default = true;
                queue.push(u);
            }
        }
        // no robots.txt at all is F02's finding; nothing points crawlers at
        // the sitemap either way
        if found_by_default && declared.is_empty() {
            e.emit(
                "C-SITEMAP-UNDECLARED",
                None,
                Loc {
                    url_path: Some(format!("{base}/robots.txt")),
                    url: Some(self.site.url_for(&format!("{base}/robots.txt"))),
                    file: self.robots_file.clone(),
                    source: None,
                },
                "",
                vec![ev(
                    "Sitemap in the build",
                    "构建中的 sitemap",
                    queue
                        .iter()
                        .map(Url::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                )],
                None,
            );
        }

        let googlebot = self.robots_group_for("googlebot");
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(sitemap_url) = queue.pop() {
            if !seen.insert(sitemap_url.to_string()) || seen.len() > 500 {
                continue;
            }
            let Some(file) = self.site.follow_http(&sitemap_url).file else {
                continue;
            };
            let Some(doc) = read_text(self.site, &file).and_then(|t| parse_sitemap(&t)) else {
                continue;
            };
            graph.sitemaps.push(sitemap_url.to_string());
            for raw in &doc.locs {
                let loc = decode_xml_entities(raw);
                let Ok(u) = sitemap_url.join(&loc) else {
                    continue;
                };
                let at = |rule: &str, key: &str, evidence: Vec<Evidence>, e: &mut Emitter| {
                    e.emit(
                        rule,
                        None,
                        Loc {
                            url_path: Some(u.path().to_string()),
                            url: Some(u.to_string()),
                            file: Some(file.clone()),
                            source: None,
                        },
                        key,
                        evidence,
                        Some(raw.clone()),
                    )
                };
                if !self.site.is_own_url(&u) {
                    at(
                        "C-SITEMAP-FOREIGN-HOST",
                        u.as_str(),
                        vec![
                            ev("Entry", "条目", u.to_string()),
                            ev("Site origin", "站点来源", self.site.origin_str()),
                        ],
                        e,
                    );
                    continue;
                }
                if doc.kind == SitemapKind::Index {
                    queue.push(u);
                    continue;
                }
                graph.sitemap_entries.push(SitemapEntry {
                    loc: u.to_string(),
                    sitemap_url: sitemap_url.to_string(),
                    sitemap_file: file.clone(),
                });
                if evaluate_path(googlebot.as_ref(), u.path()) == PathDecision::Disallow {
                    at(
                        "C-SITEMAP-ROBOTS-BLOCKED",
                        "",
                        vec![ev("Entry", "条目", u.to_string())],
                        e,
                    );
                }
                let followed = self.site.follow_http(&u);
                let Some(target) = followed.file else {
                    if !followed.looped && !followed.foreign && followed.server.is_none() {
                        at(
                            "C-SITEMAP-URL-MISSING",
                            "",
                            vec![ev("Entry", "条目", u.to_string())],
                            e,
                        );
                    }
                    continue;
                };
                graph.in_sitemap.insert(target.clone());
                if !followed.hops.is_empty() {
                    at(
                        "C-SITEMAP-URL-NONCANONICAL",
                        "",
                        vec![
                            ev("Entry", "条目", u.to_string()),
                            ev("Redirects to", "重定向到", followed.hops.join(" → ")),
                        ],
                        e,
                    );
                    continue;
                }
                let Some(p) = self.page(&target) else {
                    continue;
                };
                if p.noindex {
                    at(
                        "C-SITEMAP-URL-NOINDEX",
                        "",
                        vec![
                            ev("Entry", "条目", u.to_string()),
                            ev(
                                "noindex",
                                "noindex",
                                p.noindex_via.clone().unwrap_or_default(),
                            ),
                        ],
                        e,
                    );
                } else if !self.canonical_self(p) {
                    at(
                        "C-SITEMAP-URL-NONCANONICAL",
                        "",
                        vec![
                            ev("Entry", "条目", u.to_string()),
                            ev(
                                "Canonical",
                                "Canonical",
                                p.canonical.as_ref().map(Url::to_string).unwrap_or_default(),
                            ),
                        ],
                        e,
                    );
                }
            }
        }
        graph.sitemaps.sort();
        if !graph.sitemap_entries.is_empty() {
            for p in self
                .pages
                .iter()
                .filter(|p| self.indexable(p) && !graph.in_sitemap.contains(&p.file))
            {
                e.emit("C-PAGE-NOT-IN-SITEMAP", None, page_loc(p), "", vec![], None);
            }
        }
    }

    /// Whether a robots group shuts the site out: every content page but the
    /// homepage is disallowed (a one-page site: the homepage). `Disallow: /$`
    /// alone blocks the homepage only; `Disallow: /` with `Allow: /$` blocks
    /// the rest. Returns (pages disallowed, content pages).
    fn blocks_site(&self, group: Option<&RobotsGroup>) -> Option<(usize, usize)> {
        let home = format!("{}/", self.site.base_path);
        let pages: Vec<&str> = self
            .pages
            .iter()
            .filter(|p| p.kind == PageKind::Content)
            .map(|p| p.path.as_str())
            .collect();
        let disallowed = |path: &str| evaluate_path(group, path) == PathDecision::Disallow;
        let rest: Vec<&str> = pages.iter().copied().filter(|p| *p != home).collect();
        let shut = if rest.is_empty() {
            disallowed(&home)
        } else {
            rest.iter().all(|p| disallowed(p))
        };
        shut.then(|| {
            (
                pages.iter().filter(|p| disallowed(p)).count(),
                pages.len().max(1),
            )
        })
    }

    fn robots_rules(&self, e: &mut Emitter) {
        let Some(robots) = &self.robots else { return };
        let base = &self.site.base_path;
        let loc = || Loc {
            url_path: Some(format!("{base}/robots.txt")),
            url: Some(self.site.url_for(&format!("{base}/robots.txt"))),
            file: self.robots_file.clone(),
            source: None,
        };
        let star = robots
            .groups
            .iter()
            .any(|g| g.agents.iter().any(|a| a == "*"))
            .then(|| rules_for_agent(robots, "*"))
            .flatten();
        if let Some((blocked, total)) = self.blocks_site(star.as_ref()) {
            e.emit(
                "C-ROBOTS-BLOCKS-ALL",
                None,
                loc(),
                "",
                vec![ev(
                    "User-agent: *",
                    "User-agent: *",
                    format!("disallows {blocked} of {total} pages"),
                )],
                Some("Disallow: /".into()),
            );
        }
        for c in spec()
            .crawlers
            .iter()
            .filter(|c| c.purpose == "search_index" && !c.robots_only)
        {
            let Some(group) = rules_for_agent(robots, &c.robots_token) else {
                continue;
            };
            if !group.agents.iter().any(|a| a == &c.robots_token) {
                continue;
            }
            // the homepage, or everything but it
            if evaluate_path(Some(&group), "/") == PathDecision::Disallow
                || self.blocks_site(Some(&group)).is_some()
            {
                e.emit(
                    "C-ROBOTS-AI-SEARCH-BLOCKED",
                    None,
                    loc(),
                    &c.robots_token,
                    vec![ev(
                        "Crawler",
                        "爬虫",
                        format!("{} ({})", c.name, c.operator),
                    )],
                    Some(c.name.clone()),
                );
            }
        }
    }

    fn llms(&self, e: &mut Emitter, graph: &mut Graph) {
        let path = format!("{}/llms.txt", self.site.base_path);
        let Ok(url) = Url::parse(&self.site.url_for(&path)) else {
            return;
        };
        let Some(file) = self.site.follow_http(&url).file else {
            return;
        };
        let Some(text) = read_text(self.site, &file) else {
            return;
        };
        if aieo_site::scan::is_html_document(&text) {
            return;
        }
        graph.llms_txt = true;
        let Ok(facts) = llms::parse_llms_index(&text) else {
            return;
        };
        let mut reported = HashSet::new();
        for entry in &facts.entries {
            let Ok(mut target) = url.join(&entry.url) else {
                continue;
            };
            target.set_fragment(None);
            if !self.site.is_own_url(&target) || !reported.insert(target.path().to_string()) {
                continue;
            }
            let loc = Loc {
                url_path: Some(path.clone()),
                url: Some(url.to_string()),
                file: Some(file.clone()),
                source: None,
            };
            let followed = self.site.follow(&target);
            match followed.file {
                None if !followed.looped && !followed.foreign && followed.server.is_none() => e
                    .emit(
                        "C-LLMS-LINK-BROKEN",
                        None,
                        loc,
                        target.path(),
                        vec![ev("Link", "链接", format!("{} → {target}", entry.title))],
                        Some(entry.url.clone()),
                    ),
                Some(f) => {
                    if let Some(p) = self
                        .page(&f)
                        .filter(|p| !self.indexable(p) && p.kind == PageKind::Content)
                    {
                        let why = if p.noindex {
                            p.noindex_via.clone().unwrap_or_else(|| "noindex".into())
                        } else {
                            format!(
                                "canonical → {}",
                                p.canonical.as_ref().map(Url::to_string).unwrap_or_default()
                            )
                        };
                        e.emit(
                            "C-LLMS-LINK-NOINDEX",
                            None,
                            loc,
                            target.path(),
                            vec![
                                ev("Link", "链接", target.to_string()),
                                ev("Page", "页面", why),
                            ],
                            Some(entry.url.clone()),
                        );
                    }
                }
                None => {}
            }
        }
    }
}

#[cfg(test)]
mod hreflang_tests {
    use super::hreflang_problem;

    #[test]
    fn hreflang_codes_follow_iso() {
        for ok in [
            "en",
            "en-GB",
            "zh-Hant",
            "zh-Hant-TW",
            "es-419",
            "x-default",
            "fil",
            "EN-us",
        ] {
            assert_eq!(hreflang_problem(ok), None, "{ok}");
        }
        for bad in [
            "en-UK", "jp", "cn", "us", "en-XX", "zh-Hanx", "english", "en-GB-x",
        ] {
            assert!(hreflang_problem(bad).is_some(), "{bad}");
        }
        assert!(hreflang_problem("en-UK").unwrap().contains("GB"));
    }
}
