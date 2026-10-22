//! robots.txt parsing and path evaluation, ported from aieo.ee
//! (`rules/robots.ts`): user-agent groups (RFC 9309 §2.2), merged exact
//! groups with a `*` fallback, longest-match Allow/Disallow with `*` and `$`.

use crate::scan::starts_like_html;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RobotsGroup {
    pub agents: Vec<String>,
    pub allows: Vec<String>,
    pub disallows: Vec<String>,
    saw_rule: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RobotsFile {
    pub groups: Vec<RobotsGroup>,
    pub sitemaps: Vec<String>,
}

impl RobotsFile {
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.sitemaps.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathDecision {
    Allow,
    Disallow,
    Unspecified,
}

/// `line.replace(/#.*$/, "")`
fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

pub fn parse_robots(raw: &str) -> RobotsFile {
    if raw.is_empty() {
        return RobotsFile::default();
    }
    let text = raw.replace('\r', "");
    if starts_like_html(&text) {
        return RobotsFile::default();
    }
    let mut file = RobotsFile::default();
    for raw_line in text.split('\n') {
        let line = aieo_core::text::js_trim(strip_comment(raw_line));
        if line.is_empty() {
            continue;
        }
        let Some(idx) = line.find(':') else { continue };
        let field = aieo_core::text::js_trim(&line[..idx]).to_lowercase();
        let value = aieo_core::text::js_trim(&line[idx + 1..]);
        match field.as_str() {
            "sitemap" => {
                if !value.is_empty() {
                    file.sitemaps.push(value.to_string());
                }
            }
            "user-agent" => {
                let agent = value.to_lowercase();
                match file.groups.last_mut() {
                    // consecutive user-agent lines share a group until a rule line
                    Some(current) if !current.saw_rule => current.agents.push(agent),
                    _ => file.groups.push(RobotsGroup {
                        agents: vec![agent],
                        ..Default::default()
                    }),
                }
            }
            "allow" | "disallow" => {
                let Some(current) = file.groups.last_mut() else {
                    continue;
                };
                current.saw_rule = true;
                if !value.is_empty() {
                    if field == "allow" {
                        current.allows.push(value.to_string());
                    } else {
                        current.disallows.push(value.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    file
}

/// Rules for `agent`: all exact groups merged, else all `*` groups merged.
pub fn rules_for_agent(robots: &RobotsFile, agent: &str) -> Option<RobotsGroup> {
    let agent = agent.to_lowercase();
    let merge = |groups: Vec<&RobotsGroup>| -> Option<RobotsGroup> {
        if groups.is_empty() {
            return None;
        }
        let mut agents: Vec<String> = Vec::new();
        for a in groups.iter().flat_map(|g| g.agents.iter()) {
            if !agents.contains(a) {
                agents.push(a.clone());
            }
        }
        Some(RobotsGroup {
            agents,
            allows: groups
                .iter()
                .flat_map(|g| g.allows.iter().cloned())
                .collect(),
            disallows: groups
                .iter()
                .flat_map(|g| g.disallows.iter().cloned())
                .collect(),
            saw_rule: true,
        })
    };
    let exact: Vec<&RobotsGroup> = robots
        .groups
        .iter()
        .filter(|g| g.agents.contains(&agent))
        .collect();
    if !exact.is_empty() {
        return merge(exact);
    }
    merge(
        robots
            .groups
            .iter()
            .filter(|g| g.agents.iter().any(|a| a == "*"))
            .collect(),
    )
}

/// Longest-match evaluation; on equal length Allow wins.
pub fn evaluate_path(group: Option<&RobotsGroup>, path: &str) -> PathDecision {
    let Some(group) = group else {
        return PathDecision::Unspecified;
    };
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let units = |s: &str| aieo_core::text::utf16_len(s);
    let mut best: isize = -1;
    let mut allow = false;
    let mut matched = false;
    for p in &group.disallows {
        if p.is_empty() {
            continue;
        }
        if pattern_matches(p, &path) && units(p) as isize > best {
            best = units(p) as isize;
            allow = false;
            matched = true;
        }
    }
    for p in &group.allows {
        if pattern_matches(p, &path) && units(p) as isize >= best {
            best = units(p) as isize;
            allow = true;
            matched = true;
        }
    }
    match (matched, allow) {
        (false, _) => PathDecision::Unspecified,
        (true, true) => PathDecision::Allow,
        (true, false) => PathDecision::Disallow,
    }
}

/// `*` matches any run, a trailing `$` anchors the end, everything else is
/// literal; a match is a prefix match unless anchored.
pub fn pattern_matches(pattern: &str, path: &str) -> bool {
    let anchored = pattern.ends_with('$');
    let p: Vec<char> = if anchored {
        pattern[..pattern.len() - 1].chars().collect()
    } else {
        pattern.chars().collect()
    };
    let s: Vec<char> = path.chars().collect();
    let (mut i, mut j) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while i < s.len() {
        if j < p.len() && p[j] != '*' && p[j] == s[i] {
            i += 1;
            j += 1;
        } else if j < p.len() && p[j] == '*' {
            star = Some(j);
            j += 1;
            mark = i;
        } else if !anchored && j == p.len() {
            return true;
        } else if let Some(st) = star {
            j = st + 1;
            mark += 1;
            i = mark;
        } else {
            return false;
        }
    }
    while j < p.len() && p[j] == '*' {
        j += 1;
    }
    j == p.len()
}

/// The agent token aieo.ee's readiness scanner obeys; the CLI applies the
/// same gate to llms.txt link samples so both report the same results.
pub const SCANNER_TOKEN: &str = "aieo-readiness";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_and_longest_match() {
        let r = parse_robots("User-agent: *\nDisallow: /admin\nAllow: /admin/public\n\nUser-agent: GPTBot\nUser-agent: ClaudeBot\nDisallow: /\n# c\nSitemap: https://a.test/s.xml");
        assert_eq!(r.groups.len(), 2);
        assert_eq!(r.sitemaps, vec!["https://a.test/s.xml"]);
        let star = rules_for_agent(&r, "Googlebot");
        assert_eq!(
            evaluate_path(star.as_ref(), "/admin/x"),
            PathDecision::Disallow
        );
        assert_eq!(
            evaluate_path(star.as_ref(), "/admin/public/x"),
            PathDecision::Allow
        );
        assert_eq!(evaluate_path(star.as_ref(), "/"), PathDecision::Unspecified);
        let gpt = rules_for_agent(&r, "gptbot").unwrap();
        assert!(gpt.agents.contains(&"claudebot".to_string()));
        assert_eq!(evaluate_path(Some(&gpt), "/"), PathDecision::Disallow);
    }

    #[test]
    fn rule_lines_close_groups() {
        let r = parse_robots("User-agent: a\nDisallow:\nUser-agent: b\nDisallow: /");
        assert_eq!(r.groups.len(), 2);
        assert!(r.groups[0].disallows.is_empty());
    }

    #[test]
    fn wildcards_and_anchors() {
        assert!(pattern_matches("/*.pdf$", "/a/b.pdf"));
        assert!(!pattern_matches("/*.pdf$", "/a/b.pdf?x"));
        assert!(pattern_matches("/a*b", "/axxbyy"));
        assert!(pattern_matches("/", "/anything"));
        assert!(!pattern_matches("/x", "/"));
    }

    #[test]
    fn html_is_not_robots() {
        assert!(parse_robots("<!doctype html><html>").is_empty());
        assert!(parse_robots("   \n").is_empty());
    }
}
