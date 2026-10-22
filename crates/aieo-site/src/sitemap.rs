//! XML sitemaps, read the way aieo.ee's F05 reads them: the root element
//! after the prolog must be `urlset` or `sitemapindex`, entries are the
//! `<loc>` values of the prolog-stripped body.

use std::sync::LazyLock;

use regex::Regex;

use crate::scan::strip_xml_prolog;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SitemapKind {
    UrlSet,
    Index,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SitemapDoc {
    pub kind: SitemapKind,
    /// `<loc>` values as written
    pub locs: Vec<String>,
}

static ROOT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^<(urlset|sitemapindex)[\s>]").unwrap());
static LOC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<loc>\s*([^<\s]+)\s*</loc>").unwrap());

/// None when the body is not a urlset / sitemapindex document.
pub fn parse_sitemap(text: &str) -> Option<SitemapDoc> {
    let stripped = strip_xml_prolog(aieo_core::text::js_trim(text));
    let root = ROOT.captures(stripped)?;
    let kind = if root[1].eq_ignore_ascii_case("sitemapindex") {
        SitemapKind::Index
    } else {
        SitemapKind::UrlSet
    };
    let locs = LOC
        .captures_iter(stripped)
        .map(|c| c[1].to_string())
        .collect();
    Some(SitemapDoc { kind, locs })
}

/// Undo the five predefined XML entities (and numeric references) in a `<loc>`.
pub fn decode_xml_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(end) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let entity = &tail[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_urlsets_and_indexes() {
        let d = parse_sitemap("<?xml version=\"1.0\"?>\n<urlset xmlns=\"x\"><url><loc> https://a.test/ </loc></url><url><loc>/rel</loc></url></urlset>").unwrap();
        assert_eq!(d.kind, SitemapKind::UrlSet);
        assert_eq!(d.locs, vec!["https://a.test/", "/rel"]);
        let i = parse_sitemap(
            "<sitemapindex><sitemap><loc>https://a.test/s-0.xml</loc></sitemap></sitemapindex>",
        )
        .unwrap();
        assert_eq!(i.kind, SitemapKind::Index);
        assert!(parse_sitemap("not xml").is_none());
        assert!(parse_sitemap("<html><urlset>").is_none());
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(
            decode_xml_entities("https://a.test/?a=1&amp;b=2&#x41;&#66;&bogus"),
            "https://a.test/?a=1&b=2AB&bogus"
        );
    }
}
