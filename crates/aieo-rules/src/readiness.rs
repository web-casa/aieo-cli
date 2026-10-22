//! Readiness modules F01–F18 judged from a build directory — the same
//! judgement aieo.ee makes over HTTP (`@aieo/ai-readiness`), held to it by
//! the conformance fixtures in `spec/fixtures/conformance`.
//!
//! What a URL scan fetches, the build answers: a path that maps to a file
//! is a 200, anything else a 404; response headers come from `_headers`.
//! F01 (HTTPS and redirects) needs a live origin and is `not_applicable`.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;
use url::Url;

use aieo_core::bundle::Scores;
use aieo_core::spec::{spec, RuleClass};
use aieo_core::text::{js_lower, js_prefix, js_trim, utf16_len};
use aieo_core::{Evidence, Localized};
use aieo_site::html::{jsonld_types, Doc};
use aieo_site::robots::{
    evaluate_path, parse_robots, rules_for_agent, PathDecision, SCANNER_TOKEN,
};
use aieo_site::scan::{
    is_html_document, is_probably_html, scripts_length, starts_like_html, visible_text_length,
    visible_text_lower,
};
use aieo_site::sitemap::parse_sitemap;
use aieo_site::Site;

use crate::llms::{self, FileState};

pub const BONUS_CAP: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Partial,
    Fail,
    Info,
    NotApplicable,
    Unverified,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Partial => "partial",
            Status::Fail => "fail",
            Status::Info => "info",
            Status::NotApplicable => "not_applicable",
            Status::Unverified => "unverified",
        }
    }

    fn parse(s: &str) -> Status {
        match s {
            "pass" => Status::Pass,
            "partial" => Status::Partial,
            "fail" => Status::Fail,
            "info" => Status::Info,
            "not_applicable" => Status::NotApplicable,
            _ => Status::Unverified,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModuleResult {
    pub id: &'static str,
    pub status: Status,
    /// unrounded, as the module computed it
    pub ratio: f64,
    pub reason: Option<&'static str>,
    pub evidence: Vec<Evidence>,
    /// title of the fix, when the module has one
    pub fix: Option<Localized>,
}

fn l(en: &str, zh: &str) -> Localized {
    Localized::new(en, zh)
}

fn ev(en: &str, zh: &str, detail: impl Into<String>) -> Evidence {
    Evidence::new(l(en, zh), detail)
}

fn res(
    id: &'static str,
    status: Status,
    ratio: f64,
    evidence: Vec<Evidence>,
    fix: Option<Localized>,
) -> ModuleResult {
    ModuleResult {
        id,
        status,
        ratio,
        reason: None,
        evidence,
        fix,
    }
}

fn pass(id: &'static str, evidence: Vec<Evidence>) -> ModuleResult {
    res(id, Status::Pass, 1.0, evidence, None)
}

fn partial(id: &'static str, evidence: Vec<Evidence>, fix: Option<Localized>) -> ModuleResult {
    res(id, Status::Partial, 0.5, evidence, fix)
}

fn fail(id: &'static str, evidence: Vec<Evidence>, fix: Option<Localized>) -> ModuleResult {
    res(id, Status::Fail, 0.0, evidence, fix)
}

fn info(id: &'static str, evidence: Vec<Evidence>, fix: Option<Localized>) -> ModuleResult {
    res(id, Status::Info, 0.0, evidence, fix)
}

/// `graded()`: 0..1 rounded to 2 dp; ≥0.99 pass, ≤0.01 fail, else partial (≥0.5) or fail.
fn graded(ratio: f64) -> (Status, f64) {
    let r = ((ratio * 100.0).round() / 100.0).clamp(0.0, 1.0);
    if r >= 0.99 {
        (Status::Pass, 1.0)
    } else if r <= 0.01 {
        (Status::Fail, 0.0)
    } else if r >= 0.5 {
        (Status::Partial, r)
    } else {
        (Status::Fail, r)
    }
}

fn graded_res(
    id: &'static str,
    ratio: f64,
    evidence: Vec<Evidence>,
    fix: Option<Localized>,
) -> ModuleResult {
    let (status, r) = graded(ratio);
    let fix = if r < 1.0 { fix } else { None };
    res(id, status, r, evidence, fix)
}

// ---------------------------------------------------------------------------
// resources
// ---------------------------------------------------------------------------

/// What a URL scan would have fetched for one resource.
#[derive(Debug, Clone)]
pub struct Resource {
    pub url: String,
    /// 200 when a file answers, 404 otherwise
    pub status: u16,
    pub text: Option<String>,
    /// served body size in bytes
    pub bytes: usize,
    /// the file is not valid UTF-8: its text cannot be trusted
    pub uncertain: bool,
    pub file: Option<String>,
}

impl Resource {
    fn absent(url: String) -> Resource {
        Resource {
            url,
            status: 404,
            text: None,
            bytes: 0,
            uncertain: false,
            file: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Home,
    Robots,
    Sitemap,
    Text,
}

fn gunzip(bytes: Vec<u8>) -> Option<Vec<u8>> {
    if bytes.len() < 3 || bytes[0] != 0x1f || bytes[1] != 0x8b {
        return Some(bytes);
    }
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes.as_slice())
        .take(50_000_000)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

fn fetch(site: &Site, path: &str, kind: Kind) -> Resource {
    let url = site.url_for(path);
    let Ok(parsed) = Url::parse(&url) else {
        return Resource::absent(url);
    };
    let followed = site.follow_http(&parsed);
    let Some(file) = followed.file else {
        return Resource::absent(url);
    };
    let Some(mut bytes) = site.read_bytes(&file) else {
        return Resource::absent(url);
    };
    if kind == Kind::Sitemap {
        match gunzip(bytes) {
            Some(b) => bytes = b,
            None => {
                return Resource {
                    url,
                    status: 200,
                    text: None,
                    bytes: 0,
                    uncertain: true,
                    file: Some(file),
                }
            }
        }
    }
    let size = bytes.len();
    let (text, uncertain) = match String::from_utf8(bytes) {
        Ok(t) => (t, false),
        Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), true),
    };
    let text = text
        .strip_prefix('\u{FEFF}')
        .map(str::to_string)
        .unwrap_or(text);
    let mut r = Resource {
        url,
        status: 200,
        text: Some(text),
        bytes: size,
        uncertain,
        file: Some(file),
    };
    match kind {
        // the homepage keeps even untrusted text (the runner reports it)
        Kind::Home => {}
        // catch-all hosts answer robots.txt with HTML: no rules
        Kind::Robots => {
            if r.text.as_deref().is_some_and(is_probably_html) {
                r.text = None;
            }
        }
        Kind::Sitemap | Kind::Text => {
            if r.text.as_deref().is_some_and(is_html_document) {
                r.text = None;
                r.uncertain = false;
            } else if r.uncertain {
                r.text = None;
            }
        }
    }
    r
}

/// Every resource F02–F18 read.
pub struct Resources {
    pub home: Resource,
    pub home_x_robots: Option<String>,
    pub robots: Resource,
    pub sitemap: Resource,
    pub llms: Resource,
    pub llms_full: Resource,
    pub ai_plugin: Resource,
    pub openapi: Resource,
    /// F15 in a build: a `.md` twin of the homepage
    pub home_markdown: Resource,
    pub final_url: String,
    pub site_key: String,
}

static SITEMAP_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^sitemap:\s*\S+").unwrap());

impl Resources {
    pub fn load(site: &Site) -> Resources {
        let base = site.base_path.clone();
        let at = |p: &str| format!("{base}{p}");
        let robots = fetch(site, &at("/robots.txt"), Kind::Robots);
        let home = fetch(site, &at("/"), Kind::Home);

        // sitemap: the first same-host robots declaration, else /sitemap.xml
        let mut sitemap_path = at("/sitemap.xml");
        if let Some(text) = &robots.text {
            let declared = text
                .split('\n')
                .map(|l| js_trim(l.split('#').next().unwrap_or_default()).to_string())
                .find(|l| SITEMAP_LINE.is_match(l));
            if let Some(line) = declared {
                let candidate = js_trim(&line[line.find(':').map_or(0, |i| i + 1)..]).to_string();
                if let Ok(u) = Url::parse(&candidate) {
                    let same_host =
                        u.host_str() == site.origin.host_str() && u.port() == site.origin.port();
                    if matches!(u.scheme(), "http" | "https") && same_host {
                        sitemap_path = u.path().to_string();
                    }
                }
            }
        }
        let sitemap = fetch(site, &sitemap_path, Kind::Sitemap);

        let mut home_markdown = fetch(site, &at("/index.md"), Kind::Text);
        if home_markdown.text.is_none() {
            if let Some(doc) = home.text.as_deref().and_then(Doc::parse) {
                if let Some(href) = doc.markdown_alternates().first() {
                    if let Ok(u) = Url::parse(&home.url).and_then(|b| b.join(href)) {
                        if site.is_own_url(&u) {
                            let r = fetch(site, u.path(), Kind::Text);
                            if r.text.is_some() {
                                home_markdown = r;
                            }
                        }
                    }
                }
            }
        }

        Resources {
            home_x_robots: site.header(&at("/"), "x-robots-tag"),
            final_url: site.url_for(&at("/")),
            site_key: site.host().to_string(),
            robots,
            sitemap,
            llms: fetch(site, &at("/llms.txt"), Kind::Text),
            llms_full: fetch(site, &at("/llms-full.txt"), Kind::Text),
            ai_plugin: fetch(site, &at("/.well-known/ai-plugin.json"), Kind::Text),
            openapi: fetch(site, &at("/openapi.json"), Kind::Text),
            home_markdown,
            home,
        }
    }
}

// ---------------------------------------------------------------------------
// site type (rules/site-type.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteType {
    Content,
    Docs,
    Api,
    Ecommerce,
}

impl SiteType {
    pub fn as_str(self) -> &'static str {
        match self {
            SiteType::Content => "content",
            SiteType::Docs => "docs",
            SiteType::Api => "api",
            SiteType::Ecommerce => "ecommerce",
        }
    }
}

static CART: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)addtocart|shopping-?cart|checkout").unwrap());
static DOCS_FW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)docusaurus|vitepress|nextra|mkdocs|sphinx|gitbook").unwrap());
static OPENAPI_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)"openapi"\s*:"#).unwrap());
static SWAGGER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)swagger").unwrap());

pub fn classify_site(site_key: &str, home_html: Option<&str>) -> SiteType {
    let html = home_html.unwrap_or_default();
    let types = Doc::parse(html)
        .map(|d| jsonld_types(&d.jsonld_blocks()))
        .unwrap_or_default();
    let has = |t: &str| types.iter().any(|x| x == t);
    let text = js_lower(js_prefix(html, 200_000));
    let head = js_prefix(&text, 50_000);
    // order matters for ties: content, docs, api, ecommerce (a stable sort)
    let mut scores = [
        (SiteType::Content, 0),
        (SiteType::Docs, 0),
        (SiteType::Api, 0),
        (SiteType::Ecommerce, 0),
    ];
    let mut add = |t: SiteType, n: i32| scores.iter_mut().find(|(k, _)| *k == t).unwrap().1 += n;
    // the TypeScript classifier compares lower-case type names (sic)
    if has("product") || has("productcollection") || has("store") {
        add(SiteType::Ecommerce, 3);
    }
    if CART.is_match(head) {
        add(SiteType::Ecommerce, 2);
    }
    if has("techarticle") || has("docs") {
        add(SiteType::Docs, 2);
    }
    if site_key.starts_with("docs.")
        || site_key.starts_with("developer.")
        || site_key.starts_with("developers.")
    {
        add(SiteType::Docs, 2);
    }
    if DOCS_FW.is_match(&text) {
        add(SiteType::Docs, 2);
    }
    if OPENAPI_REF.is_match(&text) || SWAGGER.is_match(head) {
        add(SiteType::Api, 3);
    }
    if site_key.starts_with("api.") {
        add(SiteType::Api, 2);
    }
    if has("blog") || has("article") || has("newsarticle") {
        add(SiteType::Content, 2);
    }
    if has("organization") || has("website") {
        add(SiteType::Content, 1);
    }
    let mut ranked = scores.to_vec();
    ranked.sort_by_key(|r| std::cmp::Reverse(r.1));
    let (best, points) = ranked[0];
    if points <= 1 {
        SiteType::Content
    } else {
        best
    }
}

// ---------------------------------------------------------------------------
// modules
// ---------------------------------------------------------------------------

struct Ctx<'a> {
    r: &'a Resources,
    site: &'a Site,
    site_type: SiteType,
}

fn home_text<'a>(ctx: &'a Ctx) -> Option<&'a str> {
    ctx.r.home.text.as_deref()
}

fn unavailable(id: &'static str) -> ModuleResult {
    fail(id, vec![ev("Homepage unavailable", "首页不可用", "")], None)
}

fn f01() -> ModuleResult {
    res(
        "F01",
        Status::NotApplicable,
        0.0,
        vec![ev(
            "Build directory",
            "构建目录",
            "HTTPS and redirects are checked on the live site",
        )],
        None,
    )
}

fn f02(ctx: &Ctx) -> ModuleResult {
    let robots = &ctx.r.robots;
    let missing = robots.status >= 400 || robots.text.as_deref().is_none_or(starts_like_html);
    if missing {
        return fail(
            "F02",
            vec![ev("HTTP status", "HTTP 状态", robots.status.to_string())],
            Some(l("Publish a robots.txt", "发布 robots.txt")),
        );
    }
    let text = robots.text.as_deref().unwrap_or_default();
    let parsed = parse_robots(text);
    if parsed.is_empty() {
        return partial(
            "F02",
            vec![ev(
                "robots.txt present but empty",
                "robots.txt 存在但为空",
                format!("{} bytes", text.len()),
            )],
            Some(l("Add rules to robots.txt", "为 robots.txt 添加规则")),
        );
    }
    pass(
        "F02",
        vec![ev(
            "Groups / sitemap lines",
            "规则组 / sitemap 行",
            format!(
                "{} group(s), {} sitemap(s)",
                parsed.groups.len(),
                parsed.sitemaps.len()
            ),
        )],
    )
}

fn f03(ctx: &Ctx) -> ModuleResult {
    let robots = &ctx.r.robots;
    let Some(text) = robots
        .text
        .as_deref()
        .filter(|t| !t.is_empty() && robots.status < 400)
    else {
        return info(
            "F03",
            vec![ev(
                "No robots.txt",
                "无 robots.txt",
                "matrix not determinable",
            )],
            None,
        );
    };
    let parsed = parse_robots(text);
    let cats = ["search", "user_access", "training"];
    let mut blocked: [Vec<&str>; 3] = Default::default();
    let mut allowed: [Vec<&str>; 3] = Default::default();
    let mut mentioned = [false; 3];
    for (token, cat) in spec().f03_matrix() {
        let Some(group) = rules_for_agent(&parsed, token) else {
            continue;
        };
        if !group.agents.iter().any(|a| a == token) {
            continue;
        }
        let i = cats
            .iter()
            .position(|c| *c == cat)
            .expect("known F03 category");
        match evaluate_path(Some(&group), "/") {
            PathDecision::Unspecified => continue,
            PathDecision::Disallow => blocked[i].push(token),
            PathDecision::Allow => allowed[i].push(token),
        }
        mentioned[i] = true;
    }
    let labels = [
        ("AI search", "AI 搜索"),
        ("User-initiated access", "用户主动访问"),
        ("Training", "训练"),
    ];
    let mut evidence = Vec::new();
    for i in 0..3 {
        if !blocked[i].is_empty() {
            evidence.push(Evidence::new(
                l(
                    &format!("{} (blocked)", labels[i].0),
                    &format!("{}（已禁止）", labels[i].1),
                ),
                blocked[i].join(", "),
            ));
        }
        if !allowed[i].is_empty() {
            evidence.push(Evidence::new(
                l(
                    &format!("{} (allowed)", labels[i].0),
                    &format!("{}（已允许）", labels[i].1),
                ),
                allowed[i].join(", "),
            ));
        }
    }
    let declared = mentioned.iter().filter(|m| **m).count();
    if declared == 0 {
        return info(
            "F03",
            vec![ev(
                "No AI crawler declarations",
                "未发现 AI 爬虫声明",
                "no known tokens in robots.txt",
            )],
            Some(l("Declare your AI crawler policy", "声明你的 AI 爬虫策略")),
        );
    }
    res("F03", Status::Pass, declared as f64 / 3.0, evidence, None)
}

fn directive_set(value: &str) -> Vec<String> {
    js_lower(value)
        .split(',')
        .map(|d| js_trim(d).to_string())
        .collect()
}

fn f04(ctx: &Ctx) -> ModuleResult {
    let Some(html) = home_text(ctx) else {
        return unavailable("F04");
    };
    let mut evidence = Vec::new();
    let mut directives: Vec<String> = Vec::new();
    if let Some(meta) = Doc::parse(html).and_then(|d| d.meta_content("robots")) {
        evidence.push(ev("meta robots", "meta robots", meta.clone()));
        directives.extend(directive_set(&meta));
    }
    if let Some(x) = ctx.r.home_x_robots.as_deref().filter(|x| !x.is_empty()) {
        evidence.push(ev("X-Robots-Tag", "X-Robots-Tag", x));
        directives.extend(directive_set(x));
    }
    let has = |d: &str| directives.iter().any(|x| x == d);
    let blocking: Vec<&str> = ["noindex", "none"].into_iter().filter(|d| has(d)).collect();
    let degrading: Vec<&str> = ["nofollow", "nosnippet", "noarchive", "max-snippet:0"]
        .into_iter()
        .filter(|d| has(d) || has(&format!("{}:0", d.replace(":0", ""))))
        .collect();
    if !blocking.is_empty() {
        return fail(
            "F04",
            evidence,
            Some(l("Remove the noindex directive", "移除 noindex 指令")),
        );
    }
    if !degrading.is_empty() {
        evidence.push(ev(
            "Limiting directives",
            "限制性指令",
            degrading.join(", "),
        ));
        return partial(
            "F04",
            evidence,
            Some(l("Review content-limiting directives", "检查内容限制指令")),
        );
    }
    if evidence.is_empty() {
        evidence.push(ev(
            "No robots directives",
            "无 robots 指令",
            "fully permissive",
        ));
    }
    pass("F04", evidence)
}

static ROBOTS_SITEMAP_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?im)^sitemap:\s*\S+").unwrap());
static ABSOLUTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^https?://").unwrap());

fn f05(ctx: &Ctx) -> ModuleResult {
    let sitemap = &ctx.r.sitemap;
    let robots = &ctx.r.robots;
    let declared = robots.status < 400
        && robots
            .text
            .as_deref()
            .is_some_and(|t| ROBOTS_SITEMAP_LINE.is_match(t));
    let Some(text) = sitemap.text.as_deref() else {
        return fail(
            "F05",
            vec![ev(
                "Sitemap",
                "Sitemap",
                format!("{} → {}", sitemap.url, sitemap.status),
            )],
            Some(l("Publish an XML sitemap", "发布 XML sitemap")),
        );
    };
    let Some(doc) = parse_sitemap(text) else {
        let trimmed = js_trim(text);
        return fail(
            "F05",
            vec![ev(
                "Sitemap body",
                "Sitemap 内容",
                format!(
                    "not a valid urlset/sitemapindex XML ({}…)",
                    js_prefix(trimmed, 40)
                ),
            )],
            Some(l("Serve a valid XML sitemap", "提供合法的 XML sitemap")),
        );
    };
    let index = doc.kind == aieo_site::sitemap::SitemapKind::Index;
    let mut evidence = vec![
        ev("Sitemap URL", "Sitemap 地址", sitemap.url.clone()),
        ev(
            "Type / entries",
            "类型 / 条目",
            if index {
                format!("sitemapindex, {} child sitemap(s)", doc.locs.len())
            } else {
                format!("urlset, {} URL(s)", doc.locs.len())
            },
        ),
    ];
    if doc.locs.is_empty() {
        return fail(
            "F05",
            evidence,
            Some(l("Sitemap has no usable entries", "Sitemap 没有可用条目")),
        );
    }
    let absolute = doc.locs.iter().filter(|u| ABSOLUTE.is_match(u)).count();
    let ratio = if absolute == doc.locs.len() {
        1.0
    } else if (absolute as f64 / doc.locs.len() as f64) < 0.8 {
        0.5
    } else {
        1.0
    };
    if declared {
        evidence.push(ev("Declared in robots.txt", "已在 robots.txt 声明", "yes"));
    }
    graded_res(
        "F05",
        ratio,
        evidence,
        Some(l(
            "Use absolute URLs in the sitemap",
            "Sitemap 请使用绝对 URL",
        )),
    )
}

fn f06(ctx: &Ctx) -> ModuleResult {
    let Some(html) = home_text(ctx) else {
        return unavailable("F06");
    };
    let final_url = &ctx.r.final_url;
    let Some(canonical) = Doc::parse(html)
        .and_then(|d| d.canonical_href())
        .filter(|c| !c.is_empty())
    else {
        return partial(
            "F06",
            vec![ev("Canonical", "Canonical", "missing")],
            Some(l("Add a canonical link", "添加 canonical 标签")),
        );
    };
    let Ok(absolute) = Url::parse(final_url).and_then(|b| b.join(&canonical)) else {
        return partial(
            "F06",
            vec![ev(
                "Canonical (unparseable)",
                "Canonical（无法解析）",
                canonical,
            )],
            None,
        );
    };
    let norm = |u: &str| js_lower(u.strip_suffix('/').unwrap_or(u));
    if norm(absolute.as_str()) == norm(final_url) {
        return pass(
            "F06",
            vec![ev("Canonical", "Canonical", absolute.to_string())],
        );
    }
    partial(
        "F06",
        vec![ev(
            "Canonical mismatch",
            "Canonical 不一致",
            format!("canonical={absolute} vs final={final_url}"),
        )],
        Some(l(
            "Canonical does not match the final URL",
            "Canonical 与最终 URL 不一致",
        )),
    )
}

fn f07(ctx: &Ctx) -> ModuleResult {
    let Some(html) = home_text(ctx) else {
        return unavailable("F07");
    };
    let doc = Doc::parse(html);
    let Some(lang) = doc.as_ref().and_then(Doc::html_lang) else {
        return partial(
            "F07",
            vec![ev("html lang", "html lang", "missing")],
            Some(l("Declare the page language", "声明页面语言")),
        );
    };
    let alternates = doc.as_ref().map(Doc::hreflang_codes).unwrap_or_default();
    let mut evidence = vec![ev("html lang", "html lang", lang)];
    if !alternates.is_empty() {
        evidence.push(ev(
            "hreflang alternates",
            "hreflang 备选",
            alternates.join(", "),
        ));
    }
    pass("F07", evidence)
}

fn f08(ctx: &Ctx) -> ModuleResult {
    let Some(html) = home_text(ctx) else {
        return unavailable("F08");
    };
    let doc = Doc::parse(html);
    let title = doc.as_ref().map(Doc::title).unwrap_or_default();
    let description = doc.as_ref().and_then(|d| d.meta_content("description"));
    let len = utf16_len(&title);
    let mut score = 0.0;
    if len > 0 {
        score += 0.4;
    }
    if (10..=65).contains(&len) {
        score += 0.2;
    }
    if description.is_some() {
        score += 0.4;
    }
    let evidence = vec![
        ev(
            "title",
            "title",
            if title.is_empty() {
                "(empty)".into()
            } else {
                title.clone()
            },
        ),
        ev(
            "meta description",
            "meta description",
            description.unwrap_or_else(|| "(missing)".into()),
        ),
    ];
    graded_res(
        "F08",
        score,
        evidence,
        Some(l("Improve title & meta description", "完善标题与摘要")),
    )
}

fn f09(ctx: &Ctx) -> ModuleResult {
    let Some(html) = home_text(ctx) else {
        return unavailable("F09");
    };
    let Some(doc) = Doc::parse(html) else {
        return fail(
            "F09",
            vec![ev("Unparseable HTML", "HTML 无法解析", "")],
            None,
        );
    };
    let (h1, counts) = doc.headings();
    let nav = doc.has_nav();
    let base = Url::parse(&ctx.r.final_url).expect("site URL");
    let internal = doc.links(&base).iter().filter(|l| !l.external).count();
    let evidence = vec![
        ev("h1 count", "h1 数量", h1.len().to_string()),
        ev(
            "nav landmark",
            "nav 导航标记",
            if nav { "yes" } else { "missing" },
        ),
        ev("internal links", "站内链接", internal.to_string()),
    ];
    let mut score = 0.0;
    if h1.len() == 1 {
        score += 0.35;
    } else if h1.len() > 1 {
        score += 0.15;
    }
    if counts[2] + counts[3] >= 2 {
        score += 0.25;
    }
    if nav {
        score += 0.2;
    }
    if internal >= 3 {
        score += 0.2;
    }
    graded_res(
        "F09",
        score,
        evidence,
        Some(l("Improve semantic page structure", "改善页面语义结构")),
    )
}

fn f10(ctx: &Ctx) -> ModuleResult {
    let Some(html) = home_text(ctx) else {
        return unavailable("F10");
    };
    let text_len = visible_text_length(html);
    let script_len = scripts_length(html);
    let ratio = if script_len > 0 {
        text_len as f64 / script_len as f64
    } else if text_len > 500 {
        1.0
    } else {
        0.0
    };
    let evidence = vec![
        ev(
            "Visible text (initial HTML)",
            "首屏 HTML 可见文本",
            format!("{text_len} chars"),
        ),
        ev(
            "Inline/loaded scripts",
            "脚本体量",
            format!("{script_len} chars"),
        ),
    ];
    if text_len < 200 {
        return fail(
            "F10",
            evidence,
            Some(l(
                "Render core content in the initial HTML",
                "在初始 HTML 中输出核心内容",
            )),
        );
    }
    if ratio < 0.3 {
        return partial(
            "F10",
            evidence,
            Some(l(
                "Reduce JavaScript dependency of core content",
                "降低核心内容对 JavaScript 的依赖",
            )),
        );
    }
    pass("F10", evidence)
}

fn f11(ctx: &Ctx) -> ModuleResult {
    let Some(html) = home_text(ctx) else {
        return info("F11", vec![], None);
    };
    let Some(doc) = Doc::parse(html) else {
        return info("F11", vec![], None);
    };
    let failures = doc.jsonld_failures().len();
    let blocks = doc.jsonld_blocks();
    let mut evidence = vec![ev(
        "JSON-LD blocks",
        "JSON-LD 块",
        format!("{} valid, {} malformed", blocks.len(), failures),
    )];
    if blocks.is_empty() && failures == 0 {
        return info(
            "F11",
            evidence,
            Some(l("Add JSON-LD structured data", "添加 JSON-LD 结构化数据")),
        );
    }
    if blocks.is_empty() {
        evidence.push(ev(
            "Malformed JSON-LD",
            "畸形 JSON-LD",
            format!("{failures} block(s) failed JSON.parse"),
        ));
        return info(
            "F11",
            evidence,
            Some(l("Fix malformed JSON-LD", "修复畸形 JSON-LD")),
        );
    }
    let types = jsonld_types(&blocks);
    let shown = types
        .iter()
        .take(10)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    evidence.push(ev(
        "@type values",
        "@type 值",
        if shown.is_empty() {
            "(none)".into()
        } else {
            shown
        },
    ));
    pass("F11", evidence)
}

fn f12(ctx: &Ctx) -> ModuleResult {
    let Some(html) = home_text(ctx) else {
        return info("F12", vec![], None);
    };
    let Some(doc) = Doc::parse(html) else {
        return info("F12", vec![], None);
    };
    let blocks = doc.jsonld_blocks();
    if blocks.is_empty() {
        return info("F12", vec![], None);
    }
    let title = js_lower(&doc.title());
    let body = visible_text_lower(html);
    let mut names: Vec<String> = Vec::new();
    for block in &blocks {
        let Value::Object(obj) = block else { continue };
        // `obj.name ?? obj.headline`
        let name = match obj.get("name") {
            Some(v) if !v.is_null() => Some(v),
            _ => obj.get("headline"),
        };
        if let Some(Value::String(n)) = name {
            if !js_trim(n).is_empty() {
                names.push(js_trim(n).to_string());
            }
        }
        if let Some(Value::Array(graph)) = obj.get("@graph") {
            for g in graph {
                if let Some(Value::String(n)) = g.get("name") {
                    names.push(n.clone());
                }
            }
        }
    }
    if names.is_empty() {
        return info("F12", vec![], None);
    }
    let shown = names
        .iter()
        .take(5)
        .cloned()
        .collect::<Vec<_>>()
        .join(" | ");
    let mut evidence = vec![ev("Structured names checked", "检查的结构化名称", shown)];
    let consistent = names
        .iter()
        .filter(|n| {
            let needle = js_lower(n);
            body.contains(&needle) || title.contains(&needle)
        })
        .count();
    if consistent == 0 {
        return res(
            "F12",
            Status::Fail,
            0.0,
            evidence,
            Some(l(
                "Structured data does not match visible content",
                "结构化数据与可见内容不一致",
            )),
        );
    }
    evidence.push(ev(
        "Consistent",
        "一致",
        format!("{consistent}/{}", names.len()),
    ));
    res(
        "F12",
        Status::Pass,
        consistent as f64 / names.len() as f64,
        evidence,
        None,
    )
}

static CONTACT_HINT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)home|about|contact").unwrap());

fn f13(ctx: &Ctx) -> ModuleResult {
    let Some(html) = home_text(ctx) else {
        return info("F13", vec![], None);
    };
    let Some(doc) = Doc::parse(html) else {
        return info("F13", vec![], None);
    };
    let types = jsonld_types(&doc.jsonld_blocks());
    let orgish: Vec<&String> = types
        .iter()
        .filter(|t| {
            [
                "Organization",
                "Corporation",
                "LocalBusiness",
                "WebSite",
                "NewsMediaOrganization",
            ]
            .contains(&t.as_str())
        })
        .collect();
    let author = doc.author_signal();
    let contact = doc.has_mailto() || CONTACT_HINT.is_match(&doc.first_hrefs(50).join(" "));
    let evidence = vec![
        ev(
            "Organization signals",
            "组织信号",
            if orgish.is_empty() {
                "none".into()
            } else {
                orgish
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        ),
        ev(
            "Author signals",
            "作者信号",
            if author { "present" } else { "none" },
        ),
        ev(
            "Contact/about hints",
            "联系/关于线索",
            if contact { "found" } else { "none" },
        ),
    ];
    let mut score = 0.0;
    if !orgish.is_empty() {
        score += 0.5;
    }
    if author {
        score += 0.25;
    }
    if contact {
        score += 0.25;
    }
    graded_res(
        "F13",
        score,
        evidence,
        Some(l("Make the organization identifiable", "让组织身份可识别")),
    )
}

fn f14(ctx: &Ctx) -> ModuleResult {
    let Some(html) = home_text(ctx) else {
        return unavailable("F14");
    };
    let Some(doc) = Doc::parse(html) else {
        return fail(
            "F14",
            vec![ev("Unparseable HTML", "HTML 无法解析", "")],
            None,
        );
    };
    let base = Url::parse(&ctx.r.final_url).expect("site URL");
    let all = doc.links(&base);
    let internal: Vec<_> = all.iter().filter(|l| !l.external).collect();
    let with_text = internal.iter().filter(|l| !l.text.is_empty()).count();
    let evidence = vec![
        ev(
            "Crawlable links",
            "可爬取链接",
            format!("{} total, {} internal", all.len(), internal.len()),
        ),
        ev(
            "Internal links with anchor text",
            "带锚文本的站内链接",
            with_text.to_string(),
        ),
    ];
    if internal.is_empty() {
        return fail(
            "F14",
            evidence,
            Some(l(
                "No crawlable internal links found",
                "未发现可爬取的站内链接",
            )),
        );
    }
    let fix =
        (with_text < 3).then(|| l("Too few descriptive internal links", "描述性站内链接过少"));
    let (status, r) = graded((with_text as f64 / 5.0).min(1.0));
    res("F14", status, r, evidence, fix)
}

fn f15(ctx: &Ctx) -> ModuleResult {
    let md = &ctx.r.home_markdown;
    match md.text.as_deref() {
        Some(t) if utf16_len(t) >= 200 => pass(
            "F15",
            vec![ev(
                "Markdown twin",
                "Markdown 副本",
                format!("{}, {} bytes", md.url, md.bytes),
            )],
        ),
        Some(t) => info(
            "F15",
            vec![ev(
                "Markdown body (weak evidence)",
                "Markdown 正文（证据薄弱）",
                format!("{} bytes", t.len()),
            )],
            Some(l(
                "Offer a markdown version of key pages",
                "为关键页面提供 Markdown 版本",
            )),
        ),
        None => info(
            "F15",
            vec![],
            Some(l(
                "Offer a markdown version of key pages",
                "为关键页面提供 Markdown 版本",
            )),
        ),
    }
}

fn llms_state(r: &Resource) -> FileState {
    if r.uncertain {
        return FileState::Unverified;
    }
    if r.status == 404 || r.status == 410 {
        return FileState::ConfirmedAbsent;
    }
    if r.text.is_none() {
        return FileState::ConfirmedAbsent;
    }
    FileState::Readable
}

/// llms.txt link sample, judged offline: a same-site link opens when the
/// build answers it, is gone when nothing does; a link on another host
/// cannot be verified without the network.
#[derive(Debug, Clone, Default)]
pub struct LinkSample {
    pub candidates: usize,
    pub ok: usize,
    pub dead: Vec<String>,
    pub unverified: Vec<(String, &'static str)>,
}

fn sample_links(ctx: &Ctx, facts: &llms::LlmsFacts) -> Option<LinkSample> {
    if facts.h1_count != 1 {
        return None;
    }
    let file_url = Url::parse(&ctx.r.llms.url).ok()?;
    let order = llms::sampling_order(llms::link_candidates(facts, &file_url));
    if order.is_empty() {
        return None;
    }
    let robots = ctx
        .r
        .robots
        .text
        .as_deref()
        .map(parse_robots)
        .unwrap_or_default();
    let gate = rules_for_agent(&robots, SCANNER_TOKEN);
    let mut sample = LinkSample {
        candidates: order.len(),
        ..Default::default()
    };
    for url in order.into_iter().take(llms::FREE_SAMPLE_LIMIT) {
        if !ctx.site.is_own_url(&url) {
            sample
                .unverified
                .push((url.to_string(), "other host (not checked offline)"));
            continue;
        }
        if evaluate_path(gate.as_ref(), url.path()) == PathDecision::Disallow {
            sample
                .unverified
                .push((url.to_string(), "robots-disallowed"));
            continue;
        }
        let followed = ctx.site.follow_http(&url);
        if followed.file.is_some() {
            sample.ok += 1;
        } else if followed.looped {
            sample.unverified.push((url.to_string(), "redirect loop"));
        } else if followed.foreign {
            sample
                .unverified
                .push((url.to_string(), "redirects to another host"));
        } else {
            sample.dead.push(url.to_string());
        }
    }
    Some(sample)
}

fn f16(ctx: &Ctx) -> ModuleResult {
    let index = llms_state(&ctx.r.llms);
    let full = llms_state(&ctx.r.llms_full);
    let with_reason =
        |v: llms::Verdict, evidence: Vec<Evidence>, fix: Option<Localized>| ModuleResult {
            id: "F16",
            status: Status::parse(v.status),
            ratio: v.ratio,
            reason: Some(v.reason),
            evidence,
            fix,
        };
    if index != FileState::Readable {
        let v = llms::judge_without_index(index, full);
        let fix = if v.reason == "absent" {
            Some(l("Consider publishing llms.txt", "可考虑发布 llms.txt"))
        } else {
            Some(l("Complete your llms.txt", "完善你的 llms.txt"))
        };
        let detail = match index {
            FileState::ConfirmedAbsent => "not found",
            _ => "could not verify",
        };
        return with_reason(v, vec![ev("llms.txt", "llms.txt", detail)], fix);
    }
    let text = ctx.r.llms.text.as_deref().unwrap_or_default();
    let facts = match llms::parse_llms_index(text) {
        Ok(f) => f,
        Err(e) => {
            return ModuleResult {
                id: "F16",
                status: Status::Unverified,
                ratio: 0.0,
                reason: None,
                evidence: vec![ev(
                    "llms.txt",
                    "llms.txt",
                    format!("could not be parsed by our checker: {e}"),
                )],
                fix: None,
            }
        }
    };
    let sample = sample_links(ctx, &facts);
    let dead = sample.as_ref().map_or(0, |s| s.dead.len());
    let verdict = llms::apply_dead_links(llms::judge_index(&facts), dead);
    let mut evidence = vec![ev(
        "llms.txt",
        "llms.txt",
        format!(
            "{} bytes; H1 ×{}; summary {}; H2 sections {}; link entries {}; list items without a link {}",
            ctx.r.llms.bytes,
            facts.h1_count,
            if facts.summary.as_deref().is_some_and(|s| !s.is_empty()) { "yes" } else { "no" },
            facts.h2_count,
            facts.entries.len(),
            facts.non_link_item_count
        ),
    )];
    if let Some(s) = &sample {
        evidence.push(ev(
            "Link sample",
            "链接抽样",
            format!(
                "sampled {} of {} listed links; opened {}, gone {}, could not verify {}",
                s.ok + s.dead.len() + s.unverified.len(),
                s.candidates,
                s.ok,
                s.dead.len(),
                s.unverified.len()
            ),
        ));
        for d in &s.dead {
            evidence.push(ev("Gone", "已失效", format!("HTTP 404 {d}")));
        }
    }
    let fix = (verdict.status != "pass").then(|| l("Complete your llms.txt", "完善你的 llms.txt"));
    with_reason(verdict, evidence, fix)
}

fn f17(ctx: &Ctx) -> ModuleResult {
    let home = &ctx.r.home;
    if home.file.is_none() {
        return unavailable("F17");
    }
    let bytes = home.bytes;
    let evidence = vec![ev("HTML size", "HTML 大小", format!("{bytes} bytes"))];
    if bytes > 1_000_000 {
        return partial(
            "F17",
            evidence,
            Some(l("Very large homepage HTML", "首页 HTML 过大")),
        );
    }
    if bytes < 500 {
        return partial(
            "F17",
            evidence,
            Some(l("Homepage HTML is unusually small", "首页 HTML 异常小")),
        );
    }
    pass("F17", evidence)
}

fn json_object(text: &str) -> Option<serde_json::Map<String, Value>> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(o)) => Some(o),
        _ => None,
    }
}

fn f18(ctx: &Ctx) -> ModuleResult {
    let plugin = &ctx.r.ai_plugin;
    let openapi = &ctx.r.openapi;
    let mut evidence = Vec::new();
    let mut found = 0;
    let mut openapi_valid = false;
    if let Some(text) = openapi.text.as_deref().filter(|_| openapi.status < 400) {
        let valid = json_object(text).is_some_and(|o| {
            matches!(o.get("openapi"), Some(Value::String(_)))
                || matches!(o.get("swagger"), Some(Value::String(_)))
        });
        if valid {
            openapi_valid = true;
            found += 1;
            evidence.push(ev("OpenAPI", "OpenAPI", "valid spec"));
        } else {
            evidence.push(ev(
                "OpenAPI (unparseable)",
                "OpenAPI（无法解析）",
                "no openapi/swagger field or invalid JSON",
            ));
        }
    }
    let mut plugin_broken = false;
    if let Some(text) = plugin.text.as_deref().filter(|_| plugin.status < 400) {
        if json_object(text).is_some() {
            found += 1;
            evidence.push(ev(
                "ai-plugin.json",
                "ai-plugin.json",
                format!("{} bytes", text.len()),
            ));
        } else {
            plugin_broken = true;
            evidence.push(ev(
                "ai-plugin.json (invalid)",
                "ai-plugin.json（非法 JSON）",
                "not a JSON object",
            ));
        }
    }
    let declared_broken = !evidence.is_empty() && found == 0;
    if found == 0 && !declared_broken {
        if matches!(ctx.site_type, SiteType::Api | SiteType::Docs) {
            return info(
                "F18",
                vec![ev(
                    "No agent/API surface found",
                    "未发现 Agent/API 能力面",
                    format!("site type: {}", ctx.site_type.as_str()),
                )],
                Some(l(
                    "Publish a machine-readable API surface",
                    "发布机器可读的 API 能力面",
                )),
            );
        }
        return info(
            "F18",
            vec![ev(
                "No agent/API surface (normal for a content site)",
                "无 Agent/API 能力面（内容型站点的正常情况）",
                "none",
            )],
            None,
        );
    }
    if declared_broken {
        return fail(
            "F18",
            evidence,
            Some(l("Make the agent surface valid", "让 agent 能力面合法可用")),
        );
    }
    let ratio = if openapi_valid { 1.0 } else { 0.5 };
    let fix = if !openapi_valid {
        Some(l(
            "Make the OpenAPI spec parseable",
            "让 OpenAPI 规范可解析",
        ))
    } else if plugin_broken {
        Some(l(
            "Make .well-known/ai-plugin.json a valid JSON object",
            "让 .well-known/ai-plugin.json 成为合法 JSON 对象",
        ))
    } else {
        None
    };
    res(
        "F18",
        if ratio == 1.0 {
            Status::Pass
        } else {
            Status::Partial
        },
        ratio,
        evidence,
        fix,
    )
}

// ---------------------------------------------------------------------------
// runner + scores (rules/runner.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ReadinessReport {
    pub modules: Vec<ModuleResult>,
    pub scores: Scores,
    pub site_type: SiteType,
    pub incomplete_reasons: Vec<String>,
    pub link_sample: Option<LinkSample>,
    /// context for reading the scores (not findings)
    pub notes: Vec<Localized>,
}

/// Modules whose verdict rests on the homepage TEXT: an undecodable
/// homepage makes them `unverified` instead of judging mojibake.
const HOME_TEXT_MODULES: [&str; 11] = [
    "F04", "F06", "F07", "F08", "F09", "F10", "F11", "F12", "F13", "F14", "F15",
];

pub fn run(site: &Site) -> ReadinessReport {
    let r = Resources::load(site);
    let home_untrusted = r.home.uncertain;
    let site_type = classify_site(
        &r.site_key,
        if home_untrusted {
            None
        } else {
            r.home.text.as_deref()
        },
    );
    let ctx = Ctx {
        r: &r,
        site,
        site_type,
    };
    let modules: Vec<fn(&Ctx) -> ModuleResult> = vec![
        |_| f01(),
        f02,
        f03,
        f04,
        f05,
        f06,
        f07,
        f08,
        f09,
        f10,
        f11,
        f12,
        f13,
        f14,
        f15,
        f16,
        f17,
        f18,
    ];
    let mut results = Vec::new();
    for m in modules {
        let result = m(&ctx);
        let untrusted = (home_untrusted && HOME_TEXT_MODULES.contains(&result.id))
            || (result.id == "F05" && r.sitemap.uncertain);
        if untrusted {
            results.push(ModuleResult {
                id: result.id,
                status: Status::Unverified,
                ratio: 0.0,
                reason: None,
                evidence: vec![ev(
                    "Text could not be decoded as UTF-8",
                    "文本无法按 UTF-8 解码",
                    "decode-error",
                )],
                fix: None,
            });
        } else {
            results.push(result);
        }
    }
    let link_sample = if llms_state(&r.llms) == FileState::Readable {
        llms::parse_llms_index(r.llms.text.as_deref().unwrap_or_default())
            .ok()
            .and_then(|f| sample_links(&ctx, &f))
    } else {
        None
    };

    let mut incomplete_reasons = Vec::new();
    let home_html = r.home.text.as_deref().unwrap_or_default();
    if r.home.file.is_none() {
        incomplete_reasons.push("the build has no homepage (index.html)".to_string());
    } else if home_untrusted {
        incomplete_reasons.push("the homepage is not valid UTF-8".to_string());
    } else if !js_lower(js_prefix(home_html, 2048)).contains("<html")
        && !js_lower(js_prefix(home_html, 2048)).contains("<!doctype")
    {
        incomplete_reasons.push("the homepage is not parseable HTML".to_string());
    }
    let unverified_scoring: Vec<&str> = results
        .iter()
        .filter(|m| {
            m.status == Status::Unverified && spec().expect_rule(m.id).class == RuleClass::Scoring
        })
        .map(|m| m.id)
        .collect();
    if !unverified_scoring.is_empty() {
        incomplete_reasons.push(format!(
            "evidence for {} could not be verified",
            unverified_scoring.join(", ")
        ));
    }

    let (mut active, mut weighted, mut bonus) = (0.0, 0.0, 0.0);
    for m in &results {
        let rule = spec().expect_rule(m.id);
        let ratio = m.ratio.clamp(0.0, 1.0);
        match rule.class {
            RuleClass::Scoring
                if !matches!(m.status, Status::NotApplicable | Status::Unverified) =>
            {
                active += rule.weight;
                weighted += rule.weight * ratio;
            }
            RuleClass::Bonus => bonus += (rule.weight * ratio).clamp(0.0, rule.weight),
            _ => {}
        }
    }
    let base = if active > 0.0 {
        (weighted / active * 1000.0).round() / 10.0
    } else {
        0.0
    };
    let bonus = BONUS_CAP.min((bonus * 10.0).round() / 10.0);
    let final_score = 100f64.min(((base + bonus) * 10.0).round() / 10.0);
    let mut notes = Vec::new();
    if let Some(target) = r.home.file.as_deref().and_then(|f| site.redirect_stub(f)) {
        notes.push(l(
            &format!("The homepage is a meta-refresh redirect to {target}: these modules judge that redirect page, as aieo.ee and crawlers fetching {} do.", r.final_url),
            &format!("首页是指向 {target} 的 meta refresh 跳转页：以上模块评判的就是这个跳转页，与 aieo.ee 和抓取 {} 的爬虫看到的一致。", r.final_url),
        ));
    }
    ReadinessReport {
        notes,
        modules: results,
        scores: Scores {
            base,
            bonus,
            final_score,
            incomplete: !incomplete_reasons.is_empty(),
        },
        site_type,
        incomplete_reasons,
        link_sample,
    }
}

/// Ratio as `ScanFindings.modules` reports it (2 dp).
pub fn rounded_ratio(r: f64) -> f64 {
    (r.clamp(0.0, 1.0) * 100.0).round() / 100.0
}
