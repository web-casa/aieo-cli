//! Linear text scanners over page bodies, ported from aieo.ee's readiness
//! engine (`rules/html.ts`, `rules/resources.ts`, `checks/meta.ts`) with the
//! same results. Every delimiter is ASCII, so byte offsets found in an
//! ASCII-lower-cased copy are valid char boundaries of the original.

use aieo_core::text::{collapse_ws, is_js_space, utf16_len};

const STRIPPED_ELEMENTS: [&str; 4] = ["script", "style", "noscript", "svg"];

fn is_ws_byte_or_char(s: &str, at: usize) -> bool {
    s[at..].chars().next().is_some_and(is_js_space)
}

/// Name of the element opened at `lower[i]` ('<'), or None.
fn opener_at<'a>(lower: &str, i: usize, names: &[&'a str]) -> Option<&'a str> {
    for name in names {
        if !lower[i + 1..].starts_with(name) {
            continue;
        }
        let next = i + 1 + name.len();
        if next < lower.len() && (lower.as_bytes()[next] == b'>' || is_ws_byte_or_char(lower, next))
        {
            return Some(name);
        }
    }
    None
}

/// Non-overlapping element spans in document order (end exclusive): an
/// opener `<name` + whitespace/'>' runs to the first `</name>` after it, or
/// to the end of the text.
fn element_spans(html: &str, names: &[&str]) -> Vec<(usize, usize)> {
    let lower = html.to_ascii_lowercase();
    let mut spans = Vec::new();
    let mut i = lower.find('<');
    while let Some(at) = i {
        let Some(name) = opener_at(&lower, at, names) else {
            i = lower[at + 1..].find('<').map(|j| at + 1 + j);
            continue;
        };
        let close_tag = format!("</{name}>");
        let from = at + name.len() + 2;
        let end = if from > lower.len() {
            html.len()
        } else {
            match lower[from..].find(&close_tag) {
                Some(j) => from + j + name.len() + 3,
                None => html.len(),
            }
        };
        spans.push((at, end));
        if end >= html.len() {
            break;
        }
        i = lower[end..].find('<').map(|j| end + j);
    }
    spans
}

/// Replace every script/style/noscript/svg element with one space.
fn strip_elements(html: &str) -> String {
    let spans = element_spans(html, &STRIPPED_ELEMENTS);
    if spans.is_empty() {
        return html.to_string();
    }
    let mut out = String::with_capacity(html.len());
    let mut at = 0;
    for (start, end) in spans {
        out.push_str(&html[at..start]);
        out.push(' ');
        at = end;
    }
    out.push_str(&html[at..]);
    out
}

/// Replace every `<…>` tag (at least one character inside) with one space.
fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    let mut i = text.find('<');
    while let Some(lt) = i {
        let Some(gt) = text[lt + 1..].find('>').map(|j| lt + 1 + j) else {
            break;
        };
        if gt == lt + 1 {
            i = text[gt + 1..].find('<').map(|j| gt + 1 + j);
            continue;
        }
        out.push_str(&text[at..lt]);
        out.push(' ');
        at = gt + 1;
        i = text[at..].find('<').map(|j| at + j);
    }
    out.push_str(&text[at..]);
    out
}

/// Visible text: non-visible elements and tags stripped, whitespace collapsed.
pub fn visible_text(html: &str) -> String {
    collapse_ws(&strip_tags(&strip_elements(html)))
}

/// `visibleTextLength` — in UTF-16 code units.
pub fn visible_text_length(html: &str) -> usize {
    utf16_len(&visible_text(html))
}

/// `visibleTextLower`.
pub fn visible_text_lower(html: &str) -> String {
    visible_text(html).to_lowercase()
}

/// `scriptsLength` — total length of `<script>` elements, in UTF-16 code units.
/// Script length, JSON-LD blocks left out: they describe the page, it does
/// not need them to run.
pub fn scripts_length(html: &str) -> usize {
    element_spans(html, &["script"])
        .into_iter()
        .filter(|&(s, e)| {
            let opener = html[s..e]
                .find('>')
                .map_or(&html[s..e], |gt| &html[s..s + gt]);
            !opener.to_ascii_lowercase().contains("application/ld+json")
        })
        .map(|(s, e)| utf16_len(&html[s..e]))
        .sum()
}

/// The character references page text commonly carries: numeric ones and
/// `amp lt gt quot apos nbsp` (any case), so raw-HTML text compares with
/// JSON-LD strings. Anything else is left as written.
pub fn decode_basic_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 1..];
        let decoded = tail.find(';').filter(|&semi| semi <= 8).and_then(|semi| {
            let r = &tail[..semi];
            let c = if let Some(hex) = r
                .strip_prefix(['#'])
                .and_then(|h| h.strip_prefix(['x', 'X']))
            {
                (!hex.is_empty() && hex.len() <= 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
                    .then(|| u32::from_str_radix(hex, 16).ok())
                    .flatten()
                    .and_then(|n| (n > 0).then_some(n).and_then(char::from_u32))
            } else if let Some(dec) = r.strip_prefix('#') {
                (!dec.is_empty() && dec.len() <= 7 && dec.bytes().all(|b| b.is_ascii_digit()))
                    .then(|| dec.parse::<u32>().ok())
                    .flatten()
                    .and_then(|n| (n > 0).then_some(n).and_then(char::from_u32))
            } else {
                match r.to_ascii_lowercase().as_str() {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some('\u{a0}'),
                    _ => None,
                }
            };
            c.map(|c| (c, semi))
        });
        match decoded {
            Some((c, semi)) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `stripXmlProlog`: drop declaration, comments and processing instructions
/// in front of the root element, one at a time.
pub fn strip_xml_prolog(text: &str) -> &str {
    let mut pos = 0;
    let at = |pos: usize, s: &str| text[pos..].starts_with(s);
    loop {
        let start = pos;
        if at(pos, "<?xml-stylesheet") {
            if let Some(end) = text[pos + 16..].find("?>") {
                pos = pos + 16 + end + 2;
            }
        }
        if at(pos, "<?xml") && is_ws_byte_or_char(text, pos + 5) {
            if let Some(gt) = text
                .get(pos + 6..)
                .and_then(|t| t.find('>'))
                .map(|j| pos + 6 + j)
            {
                if gt >= pos + 7 && text.as_bytes()[gt - 1] == b'?' {
                    pos = gt + 1;
                }
            }
        }
        if at(pos, "<?xml?>") {
            pos += 7;
        }
        if at(pos, "<!--") {
            if let Some(end) = text[pos + 4..].find("-->") {
                pos = pos + 4 + end + 3;
            }
        }
        if at(pos, "<?") {
            if let Some(end) = text[pos + 2..].find("?>") {
                pos = pos + 2 + end + 2;
            }
        }
        while pos < text.len() && is_ws_byte_or_char(text, pos) {
            pos += text[pos..].chars().next().map_or(1, char::len_utf8);
        }
        if pos == start {
            break;
        }
    }
    &text[pos..]
}

/// The body IS an HTML document (starts as one) — the strict test text
/// resources use to recognise a soft-404 page:
/// `/^﻿?\s*(?:<!--[\s\S]*?-->\s*)*<(?:!doctype\s+html|html[\s>]|head[\s>]|body[\s>])/i`
/// over the first 2048 UTF-16 units.
pub fn is_html_document(text: &str) -> bool {
    let head = aieo_core::text::js_prefix(text, 2048);
    let mut s = head.strip_prefix('\u{FEFF}').unwrap_or(head);
    loop {
        s = s.trim_start_matches(is_js_space);
        if let Some(rest) = s.strip_prefix("<!--") {
            match rest.find("-->") {
                Some(end) => s = &rest[end + 3..],
                None => return false,
            }
        } else {
            break;
        }
    }
    let lower = s.get(..s.len().min(256)).unwrap_or(s).to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("<!doctype") {
        let trimmed = rest.trim_start_matches(is_js_space);
        return trimmed.len() < rest.len() && trimmed.starts_with("html");
    }
    for tag in ["<html", "<head", "<body"] {
        if let Some(rest) = lower.strip_prefix(tag) {
            return rest.starts_with('>') || rest.chars().next().is_some_and(is_js_space);
        }
    }
    false
}

/// Lenient sniff: does the text look like HTML at all (first 512 units)?
pub fn is_probably_html(text: &str) -> bool {
    let head = aieo_core::text::js_prefix(text, 512).to_lowercase();
    head.contains("<!doctype html") || head.contains("<html") || head.contains("<head")
}

/// `/^\s*<(?:!doctype|html)/i` — an HTML error page where robots.txt should be.
pub fn starts_like_html(text: &str) -> bool {
    let t = text.trim_start_matches(is_js_space);
    let lower = t.get(..t.len().min(9)).unwrap_or(t).to_ascii_lowercase();
    lower.starts_with("<!doctype") || lower.starts_with("<html")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_ld_is_not_script_and_references_decode() {
        let html = r#"<script type="application/ld+json">{"a":1}</script><script>x()</script>"#;
        assert_eq!(scripts_length(html), "<script>x()</script>".len());
        assert_eq!(
            decode_basic_entities("Runner &amp; Co &#38; &#x26; &AMP; &copy; &#0; & x;"),
            "Runner & Co & & & &copy; &#0; & x;"
        );
    }

    #[test]
    fn visible_text_strips_scripts_and_tags() {
        let html = "<p>Hello <b>world</b></p><script>var x = '<p>no</p>';</script><SVG><text>t</text></SVG> end";
        assert_eq!(visible_text(html), "Hello world end");
        assert_eq!(
            scripts_length(html),
            "<script>var x = '<p>no</p>';</script>".len()
        );
        // an unclosed raw element runs to the end
        assert_eq!(visible_text("a <style>b"), "a");
        // "<>" is not a tag
        assert_eq!(visible_text("a <> b"), "a <> b");
        // the opener needs whitespace or '>' after its name
        assert_eq!(visible_text("<scripts>x</scripts>"), "x");
    }

    #[test]
    fn xml_prolog() {
        let t = "<?xml version=\"1.0\"?>\n<!-- c -->\n<?xml-stylesheet href=\"a\"?><urlset>";
        assert_eq!(strip_xml_prolog(t), "<urlset>");
        assert_eq!(strip_xml_prolog("<urlset/>"), "<urlset/>");
    }

    #[test]
    fn html_document_sniffs() {
        assert!(is_html_document("<!doctype html><html>"));
        assert!(is_html_document("\u{FEFF}  <!-- x --> <html lang=en>"));
        assert!(!is_html_document("# llms\n<html>"));
        assert!(!is_html_document("<!doctypehtml>"));
        assert!(is_probably_html("<HTML>"));
        assert!(starts_like_html("  <!DOCTYPE html>"));
        assert!(!starts_like_html("User-agent: *"));
    }
}
