//! K rules: what the framework and deployment configuration does to the
//! site — origin, base path and trailing-slash settings against the URLs
//! the build publishes, redirect rules (loops, chains, dead targets),
//! header rules that noindex pages, settings a static export drops, and
//! client-rendered shells answering many URLs.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use url::Url;

use aieo_core::{Evidence, Localized};
use aieo_site::hosting::RuleSource;
use aieo_site::preset::{Project, Rendering, TrailingSlash};
use aieo_site::Site;

use crate::checks::{Checker, Graph};
use crate::emit::Emitter;
use crate::pages::{PageFacts, PageKind};

fn ev(en: &str, zh: &str, detail: impl Into<String>) -> Evidence {
    Evidence::new(Localized::new(en, zh), detail.into())
}

fn examples<'a>(items: impl Iterator<Item = &'a String>) -> String {
    let all: Vec<&String> = items.collect();
    let mut out: Vec<String> = all.iter().take(3).map(|s| s.to_string()).collect();
    if all.len() > 3 {
        out.push(format!("… {} more", all.len() - 3));
    }
    out.join(", ")
}

/// `_redirects` / `_headers` live in the build; other rule files in the project.
fn in_build(source: &RuleSource) -> bool {
    matches!(source.file.as_str(), "_redirects" | "_headers")
}

pub struct Ctx<'a> {
    pub site: &'a Site,
    pub checker: &'a Checker<'a>,
    pub pages: &'a [PageFacts],
    pub graph: &'a Graph,
    pub project: Option<&'a Project>,
    pub rendering: Option<Rendering>,
}

pub fn run(c: &Ctx, e: &mut Emitter) {
    if let Some(p) = c.project {
        origin(c, p, e);
        base_path(c, p, e);
        trailing_slash(c, p, e);
        for i in &p.ignored {
            if let Some(cfg) = &p.config {
                e.emit_config(
                    "K-EXPORT-IGNORED-CONFIG",
                    None,
                    None,
                    &cfg.file,
                    false,
                    Some(i.line),
                    &i.key,
                    vec![
                        ev("Setting", "配置项", i.key.clone()),
                        ev("Why", "原因", i.why.clone()),
                    ],
                );
            }
        }
    }
    redirects(c, e);
    header_noindex(c, e);
    spa_shell(c, e);
}

fn config_at(p: &Project, key: Option<&str>) -> Option<(String, Option<usize>)> {
    let cfg = p.config.as_ref()?;
    Some((
        cfg.file.clone(),
        key.and_then(|k| p.config_ref(k)).map(|(_, l)| l),
    ))
}

fn origin(c: &Ctx, p: &Project, e: &mut Emitter) {
    let Some(declared) = p.site.as_deref().and_then(|s| Url::parse(s).ok()) else {
        return;
    };
    let ours = c.site.origin_str();
    let theirs = declared.origin().ascii_serialization();
    if theirs == ours {
        return;
    }
    let key = p.site_key.as_deref().unwrap_or("site");
    let evidence = vec![
        ev(
            "Config",
            "配置",
            format!("{key} = {}", p.site.as_deref().unwrap_or_default()),
        ),
        ev("Checked origin", "检查的站点来源", ours.clone()),
    ];
    let at = p
        .site_at
        .clone()
        .map(|(file, line)| (file, Some(line)))
        .or_else(|| config_at(p, Some(key)));
    match at {
        Some((file, line)) => e.emit_config(
            "K-SITE-ORIGIN-MISMATCH",
            None,
            None,
            &file,
            false,
            line,
            key,
            evidence,
        ),
        None => e.emit_config(
            "K-SITE-ORIGIN-MISMATCH",
            None,
            None,
            key,
            false,
            None,
            key,
            evidence,
        ),
    }
}

fn own_canonicals<'a>(c: &'a Ctx) -> impl Iterator<Item = (&'a PageFacts, &'a Url)> {
    c.pages
        .iter()
        .filter(|p| p.kind == PageKind::Content)
        .filter_map(|p| p.canonical.as_ref().map(|u| (p, u)))
        .filter(|(_, u)| c.site.is_own_url(u))
}

fn base_path(c: &Ctx, p: &Project, e: &mut Emitter) {
    let Some(base) = p.base.as_deref() else {
        return;
    };
    let outside: Vec<String> = own_canonicals(c)
        .filter(|(_, u)| {
            let path = u.path();
            path != base && !path.starts_with(&format!("{base}/"))
        })
        .map(|(_, u)| u.to_string())
        .collect();
    if outside.is_empty() {
        return;
    }
    let key = p.base_key.as_deref().unwrap_or("base");
    let Some((file, line)) = config_at(p, Some(key)) else {
        return;
    };
    e.emit_config(
        "K-BASE-PATH-MISMATCH",
        None,
        None,
        &file,
        false,
        line,
        key,
        vec![
            ev("Base path", "base 路径", base.to_string()),
            ev(
                "Canonicals outside it",
                "base 之外的 canonical",
                format!("{}: {}", outside.len(), examples(outside.iter())),
            ),
        ],
    );
}

/// Whether a URL path can carry the trailing-slash policy (not the root,
/// not a file name with an extension).
fn slash_applies(path: &str, base: &str) -> bool {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == base {
        return false;
    }
    let last = trimmed.rsplit('/').next().unwrap_or_default();
    !last.contains('.')
}

fn trailing_slash(c: &Ctx, p: &Project, e: &mut Emitter) {
    let Some(policy) = p.trailing_slash else {
        return;
    };
    let base = p.base.clone().unwrap_or_default();
    // against the setting AND against the page file that answers it
    // (`/ko/` served by `ko/index.html` is that file's own URL, whatever the
    // setting says): only then does the URL cost a redirect or a duplicate
    let wrong = |u: &Url| {
        let path = u.path();
        let against_setting = slash_applies(path, &base)
            && match policy {
                TrailingSlash::Always => !path.ends_with('/'),
                TrailingSlash::Never => path.ends_with('/'),
            };
        against_setting
            && c.site
                .follow_http(u)
                .file
                .is_some_and(|f| c.site.page_url_path(&f).ends_with('/') != path.ends_with('/'))
    };
    let canon: Vec<String> = own_canonicals(c)
        .filter(|(_, u)| wrong(u))
        .map(|(_, u)| u.to_string())
        .collect();
    let mut seen = HashSet::new();
    let sitemap: Vec<String> = c
        .graph
        .sitemap_entries
        .iter()
        .filter_map(|s| Url::parse(&s.loc).ok())
        .filter(|u| c.site.is_own_url(u) && wrong(u))
        .map(|u| u.to_string())
        .filter(|u| seen.insert(u.clone()))
        .collect();
    if canon.is_empty() && sitemap.is_empty() {
        return;
    }
    let key = p.trailing_slash_key.as_deref().unwrap_or("trailingSlash");
    let setting = match policy {
        TrailingSlash::Always => "URLs end with /",
        TrailingSlash::Never => "URLs end without /",
    };
    let mut evidence = vec![ev("Setting", "设置", format!("{key}: {setting}"))];
    if !canon.is_empty() {
        evidence.push(ev(
            "Canonical URLs",
            "canonical URL",
            format!("{}: {}", canon.len(), examples(canon.iter())),
        ));
    }
    if !sitemap.is_empty() {
        evidence.push(ev(
            "Sitemap URLs",
            "sitemap URL",
            format!("{}: {}", sitemap.len(), examples(sitemap.iter())),
        ));
    }
    let key_only = key.split(' ').next().unwrap_or(key);
    match config_at(p, Some(key_only)) {
        Some((file, line)) => e.emit_config(
            "K-TRAILING-SLASH-MISMATCH",
            None,
            None,
            &file,
            false,
            line,
            key_only,
            evidence,
        ),
        None => e.emit_config(
            "K-TRAILING-SLASH-MISMATCH",
            None,
            None,
            "(framework default)",
            false,
            None,
            key_only,
            evidence,
        ),
    }
}

fn redirects(c: &Ctx, e: &mut Emitter) {
    let site = c.site;
    let rules = site.redirects.rules();
    for chain in site.redirects.loops(&site.origin) {
        let Some(rule) = rules.iter().find(|r| r.from == chain[0]) else {
            continue;
        };
        // an existing file shadows unforced rules (Netlify, Cloudflare
        // Pages): only a loop that following the URL really meets counts
        let start = Url::parse(&site.url_for(&chain[0]));
        if !start.is_ok_and(|u| site.follow_http(&u).looped) {
            continue;
        }
        e.emit_config(
            "K-REDIRECT-LOOP",
            Some(chain[0].clone()),
            Some(site.url_for(&chain[0])),
            &rule.source.file,
            in_build(&rule.source),
            rule.source.line,
            "",
            vec![ev("Loop", "循环", chain.join(" → "))],
        );
    }
    for rule in rules
        .iter()
        .filter(|r| r.fixed && !r.internal && r.status != 200 && !(400..500).contains(&r.status))
    {
        let Ok(target) = site.origin.join(&rule.to) else {
            continue;
        };
        if !site.is_own_url(&target) || rule.to.contains(':') && !rule.to.contains("://") {
            continue;
        }
        // a file at the source shadows an unforced rule: it never applies
        let shadowed = !rule.force
            && Url::parse(&site.url_for(&rule.from))
                .is_ok_and(|u| site.follow_http(&u).hops.is_empty());
        if shadowed {
            continue;
        }
        let followed = site.follow_http(&target);
        if followed.looped {
            continue; // reported as a loop
        }
        let at = |rule_id: &str, evidence: Vec<Evidence>, e: &mut Emitter| {
            e.emit_config(
                rule_id,
                Some(rule.from.clone()),
                Some(site.url_for(&rule.from)),
                &rule.source.file,
                in_build(&rule.source),
                rule.source.line,
                "",
                evidence,
            );
        };
        if !followed.answers() && !followed.foreign {
            at(
                "K-REDIRECT-TARGET-MISSING",
                vec![ev(
                    "Redirect",
                    "重定向",
                    format!("{} → {} ({})", rule.from, rule.to, rule.status),
                )],
                e,
            );
        } else if !followed.hops.is_empty() {
            let mut chain = vec![rule.from.clone(), target.to_string()];
            chain.extend(followed.hops.iter().cloned());
            at(
                "K-REDIRECT-CHAIN",
                vec![ev("Chain", "跳转链", chain.join(" → "))],
                e,
            );
        }
    }
}

fn says_noindex(value: &str) -> bool {
    value
        .to_lowercase()
        .split(',')
        .any(|d| matches!(d.rsplit(':').next().unwrap_or(d).trim(), "noindex" | "none"))
}

fn header_noindex(c: &Ctx, e: &mut Emitter) {
    let site = c.site;
    let home = format!("{}/", site.base_path);
    for (i, rule) in site.header_rules().rules().enumerate() {
        let Some((_, value)) = rule
            .set
            .iter()
            .find(|(n, v)| n == "x-robots-tag" && says_noindex(v))
        else {
            continue;
        };
        let hit: Vec<&PageFacts> = c
            .pages
            .iter()
            .filter(|p| {
                p.kind == PageKind::Content
                    && site.header_rules().rule_matches(i, site.host(), &p.path)
                    // a later rule may remove it again (`! X-Robots-Tag` in _headers)
                    && site
                        .header_rules()
                        .for_request(site.host(), &p.path)
                        .iter()
                        .any(|(n, v)| n == "x-robots-tag" && says_noindex(v))
            })
            .collect();
        if hit.is_empty() {
            continue;
        }
        let in_sitemap = hit
            .iter()
            .filter(|p| c.graph.in_sitemap.contains(&p.file))
            .count();
        let on_home = hit.iter().any(|p| p.path == home);
        // a noindexed drafts section is deliberate; the homepage or sitemap pages are not
        if in_sitemap == 0 && !on_home {
            continue;
        }
        let paths: Vec<String> = hit.iter().map(|p| p.path.clone()).collect();
        e.emit_config(
            "K-HEADERS-NOINDEX",
            None,
            None,
            &rule.source.file,
            in_build(rule.source),
            rule.source.line,
            rule.pattern,
            vec![
                ev(
                    "Rule",
                    "规则",
                    format!("{} → X-Robots-Tag: {value}", rule.pattern),
                ),
                ev(
                    "Pages",
                    "页面",
                    format!(
                        "{} ({in_sitemap} in the sitemap): {}",
                        hit.len(),
                        examples(paths.iter())
                    ),
                ),
            ],
        );
    }
}

fn spa_shell(c: &Ctx, e: &mut Emitter) {
    let site = c.site;
    let preset_spa = c.rendering == Some(Rendering::Spa);
    // only a rewrite (status 200) answers a URL with another page's file
    if !preset_spa && !site.redirects.rules().iter().any(|r| r.status == 200) {
        return;
    }
    // URLs (sitemap entries, internal links) a rewrite answers with another page's file
    let mut by_file: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut urls: Vec<Url> = c
        .graph
        .sitemap_entries
        .iter()
        .filter_map(|s| Url::parse(&s.loc).ok())
        .collect();
    for p in c.pages.iter().filter(|p| p.kind == PageKind::Content) {
        urls.extend(p.links.iter().map(|(_, u)| u.clone()));
    }
    for u in urls.iter().filter(|u| site.is_own_url(u)) {
        let followed = site.follow_http(u);
        let Some(file) = followed.file else { continue };
        if !followed.hops.is_empty() || !file.ends_with(".html") {
            continue;
        }
        let own = aieo_site::preset::route_key(&site.page_url_path(&file));
        if aieo_site::preset::route_key(u.path()) != own {
            by_file
                .entry(file)
                .or_default()
                .insert(u.path().to_string());
        }
    }
    let shell = by_file.iter().max_by_key(|(_, s)| s.len());
    let (file, answered) = match shell {
        Some((f, s)) if s.len() >= 2 => (f.clone(), s.len()),
        _ if preset_spa => {
            let Some(home) = site.resolve_exact(&format!("{}/", site.base_path)) else {
                return;
            };
            (home.to_string(), 0)
        }
        _ => return,
    };
    let page = c.checker.page(&file);
    let mut evidence = vec![ev("Shell", "空壳页面", file.clone())];
    if answered > 0 {
        let paths: Vec<String> = by_file[&file].iter().cloned().collect();
        evidence.push(ev(
            "URLs it answers",
            "它应答的 URL",
            format!("{answered}: {}", examples(paths.iter())),
        ));
    }
    if let Some(p) = page {
        evidence.push(ev(
            "Visible text",
            "可见文本",
            format!("{} characters; title {:?}", p.text_len, p.title),
        ));
    }
    if preset_spa {
        evidence.push(ev(
            "Rendering",
            "渲染方式",
            "client-side (framework preset)",
        ));
    }
    e.emit_config(
        "K-SPA-SHELL",
        Some(site.page_url_path(&file)),
        Some(site.url_for(&site.page_url_path(&file))),
        &file,
        true,
        None,
        "",
        evidence,
    );
}
