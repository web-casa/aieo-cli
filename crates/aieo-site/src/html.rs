//! HTML inspection with the same queries aieo.ee's rules make over linkedom
//! (`rules/html.ts`), on html5ever via `scraper`.

use std::sync::LazyLock;

use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use url::Url;

use aieo_core::text::{js_trim, utf16_len};

macro_rules! sel {
    ($name:ident, $css:expr) => {
        static $name: LazyLock<Selector> = LazyLock::new(|| Selector::parse($css).expect($css));
    };
}

sel!(TITLE, "title");
sel!(HTML, "html");
sel!(CANONICAL, r#"link[rel="canonical"]"#);
sel!(HREFLANG, r#"link[rel="alternate"][hreflang]"#);
sel!(JSONLD, r#"script[type="application/ld+json"]"#);
sel!(H1, "h1");
sel!(HEADINGS, "h1,h2,h3,h4,h5,h6");
sel!(ANCHORS, "a[href]");
sel!(NAV, r#"nav, [role="navigation"]"#);
sel!(AUTHOR_META, r#"meta[name="author"]"#);
sel!(
    AUTHOR_HINT,
    r#"[rel="author"], .author, [itemprop="author"]"#
);
sel!(MAILTO, r#"a[href^="mailto:"]"#);
sel!(IMG, "img");
sel!(
    MD_ALTERNATE,
    r#"link[rel="alternate"][type="text/markdown"]"#
);

pub struct Doc {
    html: Html,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkInfo {
    pub href: String,
    pub text: String,
    pub external: bool,
}

fn text_of(el: ElementRef) -> String {
    el.text().collect()
}

fn host_key(u: &Url) -> (Option<String>, Option<u16>) {
    (u.host_str().map(str::to_string), u.port())
}

impl Doc {
    /// `parseDocument`: None for bodies shorter than 12 UTF-16 units.
    pub fn parse(text: &str) -> Option<Doc> {
        if utf16_len(text) < 12 {
            return None;
        }
        Some(Doc {
            html: Html::parse_document(text),
        })
    }

    fn first(&self, sel: &Selector) -> Option<ElementRef<'_>> {
        self.html.select(sel).next()
    }

    pub fn title(&self) -> String {
        self.first(&TITLE)
            .map(|e| js_trim(&text_of(e)).to_string())
            .unwrap_or_default()
    }

    /// `metaContent`: `meta[name=X]`, else `meta[property=X]`; trimmed, non-empty.
    pub fn meta_content(&self, name: &str) -> Option<String> {
        let by_name = Selector::parse(&format!(r#"meta[name="{name}"]"#)).ok()?;
        let by_prop = Selector::parse(&format!(r#"meta[property="{name}"]"#)).ok()?;
        let el = self.first(&by_name).or_else(|| self.first(&by_prop))?;
        let content = js_trim(el.value().attr("content")?);
        (!content.is_empty()).then(|| content.to_string())
    }

    pub fn canonical_href(&self) -> Option<String> {
        self.first(&CANONICAL)
            .and_then(|e| e.value().attr("href"))
            .map(str::to_string)
    }

    pub fn html_lang(&self) -> Option<String> {
        let lang = js_trim(self.first(&HTML)?.value().attr("lang")?);
        (!lang.is_empty()).then(|| lang.to_string())
    }

    /// `hreflangAlternates`: lower-cased codes, empty ones dropped.
    pub fn hreflang_codes(&self) -> Vec<String> {
        self.html
            .select(&HREFLANG)
            .map(|e| {
                e.value()
                    .attr("hreflang")
                    .unwrap_or_default()
                    .to_lowercase()
            })
            .filter(|c| !c.is_empty())
            .collect()
    }

    /// (code as written, href) pairs of every hreflang alternate.
    pub fn hreflang_links(&self) -> Vec<(String, String)> {
        self.html
            .select(&HREFLANG)
            .filter_map(|e| {
                let code = e.value().attr("hreflang")?.trim().to_string();
                let href = e.value().attr("href")?.trim().to_string();
                (!code.is_empty()).then_some((code, href))
            })
            .collect()
    }

    fn jsonld_texts(&self) -> impl Iterator<Item = String> + '_ {
        self.html
            .select(&JSONLD)
            .map(text_of)
            .filter(|t| !js_trim(t).is_empty())
    }

    /// `jsonLdBlocks`: parsed blocks, top-level arrays flattened.
    pub fn jsonld_blocks(&self) -> Vec<Value> {
        let mut blocks = Vec::new();
        for raw in self.jsonld_texts() {
            match serde_json::from_str::<Value>(&raw) {
                Ok(Value::Array(items)) => blocks.extend(items),
                Ok(v) => blocks.push(v),
                Err(_) => {}
            }
        }
        blocks
    }

    /// Blocks that fail to parse, with a short excerpt of each.
    pub fn jsonld_failures(&self) -> Vec<String> {
        self.jsonld_texts()
            .filter_map(|raw| match serde_json::from_str::<Value>(&raw) {
                Ok(_) => None,
                Err(e) => Some(format!("{e}")),
            })
            .collect()
    }

    /// `headings`: non-empty h1 texts and the count per heading level.
    pub fn headings(&self) -> (Vec<String>, [usize; 7]) {
        let h1 = self
            .html
            .select(&H1)
            .map(|e| js_trim(&text_of(e)).to_string())
            .filter(|t| !t.is_empty())
            .collect();
        let mut counts = [0usize; 7];
        for e in self.html.select(&HEADINGS) {
            if let Some(level) = e
                .value()
                .name()
                .strip_prefix('h')
                .and_then(|d| d.parse::<usize>().ok())
            {
                if (1..=6).contains(&level) {
                    counts[level] += 1;
                }
            }
        }
        (h1, counts)
    }

    pub fn has_nav(&self) -> bool {
        self.first(&NAV).is_some()
    }

    /// `links`: crawlable `<a href>`s (not fragments, mailto/tel/javascript).
    pub fn links(&self, base: &Url) -> Vec<LinkInfo> {
        let base_host = host_key(base);
        let mut out = Vec::new();
        for el in self.html.select(&ANCHORS) {
            let href = el.value().attr("href").unwrap_or_default();
            if href.is_empty() || href.starts_with('#') {
                continue;
            }
            let lower = href.to_ascii_lowercase();
            if lower.starts_with("mailto:")
                || lower.starts_with("tel:")
                || lower.starts_with("javascript:")
            {
                continue;
            }
            let external = match base.join(href) {
                Ok(u) => host_key(&u) != base_host,
                Err(_) => true,
            };
            out.push(LinkInfo {
                href: href.to_string(),
                text: js_trim(&text_of(el)).to_string(),
                external,
            });
        }
        out
    }

    /// F13's author signal: `meta[name=author]` content, else the text of a
    /// rel/class/itemprop author element. Empty strings count as absent.
    pub fn author_signal(&self) -> bool {
        let meta = self
            .first(&AUTHOR_META)
            .map(|e| e.value().attr("content").map(str::to_string));
        let value = match meta {
            Some(Some(content)) => Some(content),
            _ => self.first(&AUTHOR_HINT).map(text_of),
        };
        value.is_some_and(|v| !v.is_empty())
    }

    pub fn has_mailto(&self) -> bool {
        self.first(&MAILTO).is_some()
    }

    /// `href`s of the first `limit` anchors (F13's contact/about hint).
    pub fn first_hrefs(&self, limit: usize) -> Vec<String> {
        self.html
            .select(&ANCHORS)
            .take(limit)
            .map(|e| e.value().attr("href").unwrap_or_default().to_string())
            .collect()
    }

    pub fn images_missing_alt(&self) -> usize {
        self.html
            .select(&IMG)
            .filter(|e| e.value().attr("alt").is_none())
            .count()
    }

    pub fn markdown_alternates(&self) -> Vec<String> {
        self.html
            .select(&MD_ALTERNATE)
            .filter_map(|e| e.value().attr("href"))
            .map(str::to_string)
            .collect()
    }
}

/// `jsonLdTypes`: `@type` strings, following arrays and `@graph`.
pub fn jsonld_types(blocks: &[Value]) -> Vec<String> {
    fn visit(node: &Value, out: &mut Vec<String>) {
        match node {
            Value::Array(items) => items.iter().for_each(|i| visit(i, out)),
            Value::Object(obj) => {
                match obj.get("@type") {
                    Some(Value::String(t)) => push(out, t),
                    Some(Value::Array(ts)) => ts
                        .iter()
                        .filter_map(Value::as_str)
                        .for_each(|t| push(out, t)),
                    _ => {}
                }
                if let Some(graph) = obj.get("@graph") {
                    if !graph.is_null() && graph != &Value::Bool(false) {
                        visit(graph, out);
                    }
                }
            }
            _ => {}
        }
    }
    fn push(out: &mut Vec<String>, t: &str) {
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    let mut out = Vec::new();
    blocks.iter().for_each(|b| visit(b, &mut out));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r##"<!doctype html><html lang=" en "><head><title> T </title>
<meta name="description" content="  d ">
<meta property="og:title" content="og">
<link rel="canonical" href="/x">
<link rel="alternate" hreflang="EN" href="/x"><link rel="alternate" hreflang="zh-CN" href="/zh/x">
<script type="application/ld+json">[{"@type":"Organization","name":"A"},{"@graph":[{"@type":["WebSite","Thing"]}]}]</script>
<script type="application/ld+json">{bad</script>
</head><body><nav><a href="/a">A</a><a href="https://other.test/">O</a><a href="#top">t</a><a href="mailto:x@y">m</a></nav>
<h1>One</h1><h1> </h1><h2>s</h2><img src=a><img src=b alt=""></body></html>"##;

    #[test]
    fn mirrors_the_typescript_queries() {
        let d = Doc::parse(PAGE).unwrap();
        assert_eq!(d.title(), "T");
        assert_eq!(d.meta_content("description").as_deref(), Some("d"));
        assert_eq!(d.meta_content("og:title").as_deref(), Some("og"));
        assert_eq!(d.canonical_href().as_deref(), Some("/x"));
        assert_eq!(d.html_lang().as_deref(), Some("en"));
        assert_eq!(d.hreflang_codes(), vec!["en", "zh-cn"]);
        assert_eq!(
            jsonld_types(&d.jsonld_blocks()),
            vec!["Organization", "WebSite", "Thing"]
        );
        assert_eq!(d.jsonld_failures().len(), 1);
        let (h1, counts) = d.headings();
        assert_eq!(h1, vec!["One"]);
        assert_eq!((counts[1], counts[2]), (2, 1));
        let links = d.links(&Url::parse("https://site.test/").unwrap());
        assert_eq!(links.len(), 2);
        assert!(!links[0].external && links[1].external);
        assert!(d.has_nav() && d.has_mailto());
        assert_eq!(d.images_missing_alt(), 1);
        assert!(Doc::parse("<p>tiny</p>").is_none());
    }
}
