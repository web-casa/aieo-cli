//! llms.txt INDEX parsing and judgement (F16), ported from aieo.ee
//! (`llms/parse.ts`, `llms/spec.ts`, `llms/link-health.ts`). Parsed as a
//! CommonMark AST — markdown-rs builds the same mdast tree as
//! mdast-util-from-markdown — and only ROOT-level nodes count.

use std::collections::{HashMap, HashSet};

use markdown::mdast::{Node, Paragraph};
use url::Url;

pub const LLMS_RULES_VERSION: &str = "llms-index/3";
/// links sampled into F16 per scan (aieo.ee `LINK_HEALTH_BUDGET.freeSampleLimit`)
pub const FREE_SAMPLE_LIMIT: usize = 3;
/// ratio lost per sampled link confirmed gone
pub const DEAD_LINK_PENALTY: f64 = 0.15;

#[derive(Debug, Clone, PartialEq)]
pub struct LlmsEntry {
    pub url: String,
    pub title: String,
    pub section: usize,
    pub has_description: bool,
    pub relative: bool,
    pub text_before_link: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LlmsFacts {
    pub h1_count: usize,
    pub h1_text: Option<String>,
    /// None = no root blockquote; Some("") = an empty one
    pub summary: Option<String>,
    pub h2_count: usize,
    pub max_heading_depth: u8,
    pub entries: Vec<LlmsEntry>,
    pub non_link_item_count: usize,
    pub duplicate_url_count: usize,
}

/// mdast-util-to-string: literal values, image alt text, children.
fn node_text(node: &Node) -> String {
    match node {
        Node::Image(i) => i.alt.clone(),
        Node::ImageReference(i) => i.alt.clone(),
        Node::Definition(_)
        | Node::Break(_)
        | Node::ThematicBreak(_)
        | Node::FootnoteReference(_) => String::new(),
        other => match other.children() {
            Some(children) => children.iter().map(node_text).collect(),
            None => other.to_string(),
        },
    }
}

fn collect_definitions(node: &Node, defs: &mut HashMap<String, String>) {
    if let Node::Definition(d) = node {
        if !d.url.trim().is_empty() {
            defs.entry(d.identifier.clone())
                .or_insert_with(|| d.url.trim().to_string());
        }
    }
    if let Some(children) = node.children() {
        children.iter().for_each(|c| collect_definitions(c, defs));
    }
}

struct Found<'a> {
    url: String,
    title: String,
    node: &'a Node,
}

/// First resolvable link of a paragraph, in document order.
fn first_link<'a>(nodes: &'a [Node], defs: &HashMap<String, String>) -> Option<Found<'a>> {
    for node in nodes {
        match node {
            Node::Link(l) => {
                if !l.url.trim().is_empty() {
                    return Some(Found {
                        url: l.url.trim().to_string(),
                        title: aieo_core::text::js_trim(&node_text(node)).to_string(),
                        node,
                    });
                }
            }
            Node::LinkReference(r) => {
                if let Some(url) = defs.get(&r.identifier) {
                    return Some(Found {
                        url: url.clone(),
                        title: aieo_core::text::js_trim(&node_text(node)).to_string(),
                        node,
                    });
                }
            }
            other => {
                if let Some(children) = other.children() {
                    if let Some(found) = first_link(children, defs) {
                        return Some(found);
                    }
                }
            }
        }
    }
    None
}

/// Paragraph text before and after one of its nodes (by identity).
fn text_around(paragraph: &Paragraph, target: &Node) -> (String, String) {
    fn walk(
        nodes: &[Node],
        target: &Node,
        passed: &mut bool,
        before: &mut String,
        after: &mut String,
    ) {
        for node in nodes {
            if std::ptr::eq(node, target) {
                *passed = true;
                continue;
            }
            if let Some(children) = node.children() {
                walk(children, target, passed, before, after);
                continue;
            }
            let text = match node {
                Node::Image(i) => i.alt.clone(),
                Node::ImageReference(i) => i.alt.clone(),
                Node::Break(_) => String::new(),
                other => other.to_string(),
            };
            if *passed {
                after.push_str(&text);
            } else {
                before.push_str(&text);
            }
        }
    }
    let (mut passed, mut before, mut after) = (false, String::new(), String::new());
    walk(
        &paragraph.children,
        target,
        &mut passed,
        &mut before,
        &mut after,
    );
    (before, after)
}

fn is_absolute_http(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

pub fn parse_llms_index(raw: &str) -> Result<LlmsFacts, String> {
    let text = raw.strip_prefix('\u{FEFF}').unwrap_or(raw);
    let tree =
        markdown::to_mdast(text, &markdown::ParseOptions::default()).map_err(|e| e.to_string())?;
    let mut defs = HashMap::new();
    collect_definitions(&tree, &mut defs);
    let mut facts = LlmsFacts::default();
    let mut seen: HashSet<String> = HashSet::new();
    for node in tree.children().map(Vec::as_slice).unwrap_or_default() {
        match node {
            Node::Heading(h) => {
                facts.max_heading_depth = facts.max_heading_depth.max(h.depth);
                if h.depth == 1 {
                    facts.h1_count += 1;
                    if facts.h1_text.is_none() {
                        facts.h1_text =
                            Some(aieo_core::text::js_trim(&node_text(node)).to_string());
                    }
                } else if h.depth == 2 {
                    facts.h2_count += 1;
                }
            }
            Node::Blockquote(_) => {
                if facts.summary.is_none() {
                    facts.summary = Some(aieo_core::text::js_trim(&node_text(node)).to_string());
                }
            }
            Node::List(list) => {
                for item in &list.children {
                    let first = item.children().and_then(|c| {
                        c.iter().find_map(|n| match n {
                            Node::Paragraph(p) => Some(p),
                            _ => None,
                        })
                    });
                    let link = first.and_then(|p| first_link(&p.children, &defs));
                    let (Some(paragraph), Some(link)) = (first, link) else {
                        if facts.h2_count >= 1 {
                            facts.non_link_item_count += 1;
                        }
                        continue;
                    };
                    let (before, after) = text_around(paragraph, link.node);
                    let after = after.trim_start_matches(|c: char| {
                        aieo_core::text::is_js_space(c) || matches!(c, ':' | '：' | '–' | '—' | '-')
                    });
                    if seen.contains(&link.url) {
                        facts.duplicate_url_count += 1;
                    }
                    seen.insert(link.url.clone());
                    facts.entries.push(LlmsEntry {
                        title: if link.title.is_empty() {
                            link.url.clone()
                        } else {
                            link.title.clone()
                        },
                        relative: !is_absolute_http(&link.url),
                        url: link.url,
                        section: facts.h2_count,
                        has_description: !aieo_core::text::js_trim(after).is_empty(),
                        text_before_link: !aieo_core::text::js_trim(&before).is_empty(),
                    });
                }
            }
            _ => {}
        }
    }
    Ok(facts)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    Readable,
    ConfirmedAbsent,
    Unverified,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub status: &'static str,
    pub ratio: f64,
    pub reason: &'static str,
}

fn round2(x: f64) -> f64 {
    (x.clamp(0.0, 1.0) * 100.0).round() / 100.0
}

pub fn judge_index(facts: &LlmsFacts) -> Verdict {
    if facts.h1_count == 0 {
        return Verdict {
            status: "fail",
            ratio: 0.0,
            reason: "index-h1-missing",
        };
    }
    if facts.h1_count > 1 {
        return Verdict {
            status: "fail",
            ratio: 0.0,
            reason: "index-h1-multiple",
        };
    }
    let entry_score = ((if facts.entries.is_empty() { 0.0 } else { 0.5 })
        - 0.1 * facts.non_link_item_count as f64)
        .max(0.0);
    let summary = facts.summary.as_deref().is_some_and(|s| !s.is_empty());
    let raw = (if summary { 0.25 } else { 0.0 })
        + (if facts.h2_count >= 1 { 0.25 } else { 0.0 })
        + entry_score;
    let ratio = round2(raw);
    if ratio >= 0.99 {
        Verdict {
            status: "pass",
            ratio: 1.0,
            reason: "index-valid",
        }
    } else {
        Verdict {
            status: "partial",
            ratio,
            reason: "index-incomplete",
        }
    }
}

pub fn judge_without_index(index: FileState, full: FileState) -> Verdict {
    match (index, full) {
        (FileState::Unverified, _) => Verdict {
            status: "unverified",
            ratio: 0.0,
            reason: "index-unverified",
        },
        (_, FileState::Readable) => Verdict {
            status: "partial",
            ratio: 0.0,
            reason: "full-only",
        },
        (_, FileState::Unverified) => Verdict {
            status: "unverified",
            ratio: 0.0,
            reason: "full-unverified",
        },
        _ => Verdict {
            status: "info",
            ratio: 0.0,
            reason: "absent",
        },
    }
}

pub fn apply_dead_links(v: Verdict, dead: usize) -> Verdict {
    if dead == 0 || v.status == "fail" {
        return v;
    }
    Verdict {
        status: "partial",
        ratio: round2(v.ratio - DEAD_LINK_PENALTY * dead as f64),
        reason: "index-dead-links",
    }
}

/// Distinct absolute http(s) targets in document order, fragments dropped,
/// each with the section of its first appearance.
pub fn link_candidates(facts: &LlmsFacts, file_url: &Url) -> Vec<(Url, usize)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for e in &facts.entries {
        let Ok(mut u) = file_url.join(&e.url) else {
            continue;
        };
        if !matches!(u.scheme(), "http" | "https") {
            continue;
        }
        u.set_fragment(None);
        if seen.insert(u.to_string()) {
            out.push((u, e.section));
        }
    }
    out
}

/// Round-robin over sections (in order of first appearance), document
/// order inside a section — deterministic, and every prefix is a sample.
pub fn sampling_order(candidates: Vec<(Url, usize)>) -> Vec<Url> {
    let mut sections: Vec<usize> = Vec::new();
    let mut by_section: HashMap<usize, Vec<Url>> = HashMap::new();
    for (u, s) in candidates {
        if !by_section.contains_key(&s) {
            sections.push(s);
        }
        by_section.entry(s).or_default().push(u);
    }
    let total: usize = by_section.values().map(Vec::len).sum();
    let mut out = Vec::with_capacity(total);
    let mut round = 0;
    while out.len() < total {
        for s in &sections {
            if let Some(u) = by_section[s].get(round) {
                out.push(u.clone());
            }
        }
        round += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_proposal_structure() {
        let f = parse_llms_index(
            "\u{FEFF}# Acme\n\n> Summary\n\n- [free](/free) detail list before H2\n- not counted\n\n## Docs\n- [Guide](https://a.test/g): how to\n- See [API][api] here\n- plain item\n- [Guide again](https://a.test/g)\n\n```\n# not a heading\n```\n\n[api]: /api\n",
        )
        .unwrap();
        assert_eq!((f.h1_count, f.h2_count), (1, 1));
        assert_eq!(f.summary.as_deref(), Some("Summary"));
        assert_eq!(f.entries.len(), 4);
        assert_eq!(f.non_link_item_count, 1);
        assert_eq!(f.duplicate_url_count, 1);
        assert!(f.entries[1].has_description && !f.entries[1].relative);
        assert!(f.entries[2].text_before_link && f.entries[2].url == "/api");
        assert_eq!(f.entries[0].section, 0);
        assert_eq!(
            judge_index(&f),
            Verdict {
                status: "partial",
                ratio: 0.9,
                reason: "index-incomplete"
            }
        );
    }

    #[test]
    fn verdicts() {
        let f = parse_llms_index("# A\n\n> s\n\n## B\n- [x](/x)\n").unwrap();
        let v = judge_index(&f);
        assert_eq!(v.status, "pass");
        assert_eq!(
            apply_dead_links(v, 1),
            Verdict {
                status: "partial",
                ratio: 0.85,
                reason: "index-dead-links"
            }
        );
        assert_eq!(
            judge_index(&parse_llms_index("## no title\n").unwrap()).reason,
            "index-h1-missing"
        );
        assert_eq!(
            judge_without_index(FileState::ConfirmedAbsent, FileState::Readable).reason,
            "full-only"
        );
    }

    #[test]
    fn samples_round_robin_over_sections() {
        let f = parse_llms_index("# A\n- [a](/a)\n- [b](/b)\n## S1\n- [c](/c)\n- [d](/d#x)\n- [d2](/d)\n## S2\n- [e](/e)\n").unwrap();
        let base = Url::parse("https://s.test/llms.txt").unwrap();
        let order: Vec<String> = sampling_order(link_candidates(&f, &base))
            .iter()
            .map(|u| u.path().to_string())
            .collect();
        assert_eq!(order, vec!["/a", "/c", "/e", "/b", "/d"]);
    }
}
