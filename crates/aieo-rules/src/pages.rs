//! Page inventory: the facts T, C and D rules read from every HTML file.

use std::sync::LazyLock;

use rayon::prelude::*;
use regex::Regex;
use sha2::{Digest, Sha256};
use url::Url;

use aieo_site::html::{jsonld_types, Doc};
use aieo_site::preset::Project;
use aieo_site::scan::visible_text;
use aieo_site::Site;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    Content,
    /// the host's 404 page
    NotFound,
    /// a host fallback shell, never a page of its own (`200.html` SPA
    /// fallback, `500.html`, Next.js `_global-error.html`)
    Fallback,
    /// a meta-refresh redirect page
    Redirect,
    /// search-console verification file
    Verification,
}

#[derive(Debug, Clone)]
pub struct Hreflang {
    pub code: String,
    pub href: String,
    pub url: Option<Url>,
}

#[derive(Debug, Clone)]
pub struct PageFacts {
    pub file: String,
    /// URL path on the site (base path included)
    pub path: String,
    pub url: Url,
    pub source: Option<String>,
    pub kind: PageKind,
    pub bytes: u64,
    pub undecodable: bool,
    pub title: String,
    pub description: Option<String>,
    pub lang: Option<String>,
    pub noindex: bool,
    /// where the noindex came from (meta robots / X-Robots-Tag)
    pub noindex_via: Option<String>,
    pub canonical_href: Option<String>,
    pub canonical: Option<Url>,
    pub hreflang: Vec<Hreflang>,
    pub h1_count: usize,
    pub images_missing_alt: usize,
    pub jsonld_failures: Vec<String>,
    pub jsonld_types: Vec<String>,
    /// schema.org issues of the blocks that parsed
    pub jsonld_issues: Vec<crate::structured::Issue>,
    /// organization identity across the page's JSON-LD (P03, P04)
    pub entities: crate::structured::Entities,
    pub has_nav: bool,
    /// visible text length, whitespace collapsed, in UTF-16 units (as GEOReady counts)
    pub text_len: usize,
    pub words: u64,
    /// same-site link targets (fragment dropped), as written and resolved
    pub links: Vec<(String, Url)>,
    pub markdown_alternates: Vec<(String, Option<Url>)>,
    pub hash: String,
}

static VERIFICATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|/)(google[0-9a-f]{8,}|yandex_[0-9a-f]+)\.html$").unwrap());

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

/// Words of visible text: whitespace-separated tokens, each CJK character
/// counting as one word.
pub fn word_count(text: &str) -> u64 {
    let mut n = 0u64;
    for token in text
        .split(aieo_core::text::is_js_space)
        .filter(|t| !t.is_empty())
    {
        let cjk = token.chars().filter(|c| is_cjk(*c)).count() as u64;
        let rest = token.chars().any(|c| !is_cjk(c) && c.is_alphanumeric());
        n += cjk + u64::from(rest);
    }
    n
}

fn hash16(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `noindex` / `none` in a robots directive list. X-Robots-Tag may name a
/// crawler first (`googlebot: noindex`); any crawler's noindex counts.
fn says_noindex(value: &str) -> bool {
    value.to_lowercase().split(',').any(|d| {
        let d = d.trim();
        let d = d
            .rsplit_once(':')
            .map(|(_, v)| v.trim())
            .filter(|_| !d.starts_with("max-"))
            .unwrap_or(d);
        d == "noindex" || d == "none"
    })
}

fn page_kind(site: &Site, rel: &str) -> PageKind {
    if matches!(rel, "404.html" | "404/index.html" | "_not-found.html") {
        PageKind::NotFound
    } else if matches!(rel, "200.html" | "500.html" | "_global-error.html") {
        PageKind::Fallback
    } else if site.redirect_stub(rel).is_some() {
        PageKind::Redirect
    } else if VERIFICATION.is_match(rel) {
        PageKind::Verification
    } else {
        PageKind::Content
    }
}

fn extract(site: &Site, project: Option<&Project>, rel: &str) -> PageFacts {
    let path = site.page_url_path(rel);
    let url = Url::parse(&site.url_for(&path)).expect("site URL");
    let bytes = site.read_bytes(rel).unwrap_or_default();
    let size = bytes.len() as u64;
    let (html, undecodable) = match String::from_utf8(bytes) {
        Ok(t) => (t, false),
        Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), true),
    };
    let doc = Doc::parse(&html);
    let text = visible_text(&html);
    let mut facts = PageFacts {
        file: rel.to_string(),
        source: project.and_then(|p| p.source_for(&path)),
        kind: page_kind(site, rel),
        bytes: size,
        undecodable,
        title: String::new(),
        description: None,
        lang: None,
        noindex: false,
        noindex_via: None,
        canonical_href: None,
        canonical: None,
        hreflang: Vec::new(),
        h1_count: 0,
        images_missing_alt: 0,
        jsonld_failures: Vec::new(),
        jsonld_types: Vec::new(),
        jsonld_issues: Vec::new(),
        entities: Default::default(),
        has_nav: false,
        text_len: aieo_core::text::utf16_len(&text),
        words: word_count(&text),
        links: Vec::new(),
        markdown_alternates: Vec::new(),
        hash: hash16(&text),
        path: path.clone(),
        url: url.clone(),
    };
    if let Some(x) = site
        .header(&path, "x-robots-tag")
        .filter(|x| says_noindex(x))
    {
        facts.noindex = true;
        facts.noindex_via = Some(format!("X-Robots-Tag: {x}"));
    }
    let Some(doc) = doc else { return facts };
    facts.title = doc.title();
    facts.description = doc.meta_content("description");
    facts.lang = doc.html_lang();
    for name in ["robots", "googlebot"] {
        if let Some(meta) = doc.meta_content(name).filter(|m| says_noindex(m)) {
            if !facts.noindex {
                facts.noindex = true;
                facts.noindex_via = Some(format!("<meta name=\"{name}\" content=\"{meta}\">"));
            }
        }
    }
    facts.canonical_href = doc.canonical_href().filter(|c| !c.trim().is_empty());
    facts.canonical = facts
        .canonical_href
        .as_deref()
        .and_then(|c| url.join(c.trim()).ok());
    facts.hreflang = doc
        .hreflang_links()
        .into_iter()
        .map(|(code, href)| Hreflang {
            url: url.join(&href).ok(),
            code,
            href,
        })
        .collect();
    facts.h1_count = doc.headings().0.len();
    facts.images_missing_alt = doc.images_missing_alt();
    facts.jsonld_failures = doc.jsonld_failures();
    let blocks = doc.jsonld_blocks();
    facts.jsonld_types = jsonld_types(&blocks);
    facts.jsonld_issues = crate::structured::check(&blocks);
    facts.entities = crate::structured::entities(&blocks);
    facts.has_nav = doc.has_nav();
    for link in doc.links(&url) {
        if let Ok(mut target) = url.join(&link.href) {
            target.set_fragment(None);
            if site.is_own_url(&target) {
                facts.links.push((link.href, target));
            }
        }
    }
    facts.markdown_alternates = doc
        .markdown_alternates()
        .into_iter()
        .map(|h| (h.clone(), url.join(&h).ok()))
        .collect();
    facts
}

/// Facts of every HTML file in the build, in file order.
pub fn collect(site: &Site, project: Option<&Project>) -> Vec<PageFacts> {
    let files: Vec<&str> = site
        .files()
        .filter(|f| f.ends_with(".html") || f.ends_with(".htm"))
        .collect();
    files
        .par_iter()
        .map(|rel| extract(site, project, rel))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words_across_scripts() {
        assert_eq!(word_count("Hello world, again"), 3);
        assert_eq!(word_count("安装 Ubuntu 系统"), 5);
        assert_eq!(word_count(" — "), 0);
    }

    #[test]
    fn reads_noindex_directives() {
        assert!(says_noindex("noindex, nofollow"));
        assert!(says_noindex("googlebot: noindex"));
        assert!(says_noindex("NONE"));
        assert!(!says_noindex("max-snippet:0, nofollow"));
        assert!(!says_noindex("index, follow"));
    }
}
