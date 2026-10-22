//! Route ↔ source-file index. Each preset turns its framework's file-based
//! routing into routes with canonical segments — literal, `[name]` (one
//! segment) or `[...name]` (any number, zero included) — so lookups and
//! server-route patterns work the same for every framework.

use std::collections::HashMap;
use std::path::Path;

use regex::Regex;

use crate::site::ServerRoute;

/// Normalised route: `/A/b/index.html` → `/a/b`, `/` → `/`.
pub fn route_key(path: &str) -> String {
    let mut p = path.trim_end_matches(".html").to_string();
    if let Some(stripped) = p.strip_suffix("/index") {
        p = stripped.to_string();
    }
    let p = p.trim_end_matches('/').to_lowercase();
    if p.is_empty() || p == "/index" {
        "/".into()
    } else if p.starts_with('/') {
        p
    } else {
        format!("/{p}")
    }
}

#[derive(Debug, Clone, Default)]
pub struct SourceIndex {
    exact: HashMap<String, String>,
    /// dynamic routes: segments → file, in insertion order
    dynamic: Vec<(Vec<String>, String)>,
}

impl SourceIndex {
    /// Register a route (`segments` canonical, without empty parts) built
    /// from `file` (relative to the project root). First registration wins.
    pub fn add(&mut self, segments: &[String], file: &str) {
        if segments.iter().any(|s| s.starts_with('[')) {
            if !self.dynamic.iter().any(|(s, _)| s == segments) {
                self.dynamic.push((segments.to_vec(), file.to_string()));
            }
        } else {
            self.exact
                .entry(route_key(&format!("/{}", segments.join("/"))))
                .or_insert_with(|| file.to_string());
        }
    }

    pub fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.dynamic.is_empty()
    }

    pub fn lookup(&self, key: &str) -> Option<String> {
        if let Some(f) = self.exact.get(key) {
            return Some(f.clone());
        }
        let parts: Vec<&str> = key.split('/').filter(|s| !s.is_empty()).collect();
        // the most specific dynamic route: fewest catch-alls, then most segments
        self.dynamic
            .iter()
            .filter(|(segs, _)| dynamic_match(segs, &parts))
            .min_by_key(|(segs, _)| {
                (
                    segs.iter().filter(|s| s.starts_with("[...")).count(),
                    usize::MAX - segs.len(),
                )
            })
            .map(|(_, f)| f.clone())
    }

    /// Every route as a request-path pattern (for builds whose server
    /// renders routes on request).
    pub fn server_routes(&self) -> Vec<ServerRoute> {
        let mut out: Vec<ServerRoute> =
            self.exact.keys().filter_map(|k| route_pattern(k)).collect();
        for (segs, _) in &self.dynamic {
            if let Some(r) = route_pattern(&format!("/{}", segs.join("/"))) {
                out.push(r);
            }
        }
        out.sort_by(|a, b| a.route.cmp(&b.route));
        out
    }
}

/// A route with `[x]` / `[...x]` / `[[...x]]` segments as a path regex
/// (trailing slash optional, case-sensitive like the frameworks).
pub fn route_pattern(route: &str) -> Option<ServerRoute> {
    let mut re = String::from("^");
    let segs: Vec<&str> = route.split('/').filter(|s| !s.is_empty()).collect();
    for seg in &segs {
        if seg.starts_with("[[...") || seg.starts_with("[...") {
            re.push_str("(?:/.*)?");
        } else if seg.starts_with('[') {
            re.push_str("/[^/]+");
        } else {
            re.push('/');
            re.push_str(&regex::escape(seg));
        }
    }
    if segs.is_empty() {
        re.push('/');
    } else {
        re.push_str("/?");
    }
    re.push('$');
    Some(ServerRoute {
        pattern: Regex::new(&re).ok()?,
        route: if route.is_empty() {
            "/".into()
        } else {
            route.into()
        },
    })
}

fn dynamic_match(pattern: &[String], parts: &[&str]) -> bool {
    match pattern.first() {
        None => parts.is_empty(),
        Some(seg) if seg.starts_with("[...") => {
            (0..=parts.len()).any(|k| dynamic_match(&pattern[1..], &parts[k..]))
        }
        Some(seg) if seg.starts_with('[') => {
            !parts.is_empty() && dynamic_match(&pattern[1..], &parts[1..])
        }
        Some(seg) => {
            parts.first().is_some_and(|p| p.eq_ignore_ascii_case(seg))
                && dynamic_match(&pattern[1..], &parts[1..])
        }
    }
}

/// Files under `root/dir` with one of `exts`, as (path relative to root,
/// path relative to dir without extension, as segments). Sorted.
pub fn walk_sources(root: &Path, dir: &str, exts: &[&str]) -> Vec<(String, Vec<String>)> {
    let base = root.join(dir);
    if !base.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(&base)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            name != "node_modules" && !(e.depth() > 0 && name.starts_with('.'))
        })
        .flatten()
    {
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !entry.file_type().is_file() || !exts.contains(&ext) {
            continue;
        }
        let rel_to_root = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let rel = path.strip_prefix(&base).unwrap_or(path).with_extension("");
        let segments = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        out.push((rel_to_root, segments));
    }
    out
}

/// github-slugger's per-segment slug (Astro content collections, Docusaurus ids).
pub fn slugify(segment: &str) -> String {
    segment
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

/// Front matter of a Markdown file: `---` YAML or `+++` TOML, as raw text.
pub fn front_matter(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim_start_matches('\u{feff}');
    for fence in ["---", "+++"] {
        if let Some(rest) = text.strip_prefix(fence) {
            let end = rest.find(&format!("\n{fence}"))?;
            return Some(rest[..end].to_string());
        }
    }
    None
}

/// A top-level scalar of front matter (`key: v` or `key = "v"`), unquoted.
pub fn front_matter_value(front: &str, key: &str) -> Option<String> {
    front.lines().find_map(|l| {
        let rest = l.strip_prefix(key)?;
        let rest = rest.trim_start();
        let rest = rest.strip_prefix(':').or_else(|| rest.strip_prefix('='))?;
        let v = rest.trim().trim_matches(|c| c == '"' || c == '\'');
        (!v.is_empty()).then(|| v.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_exact_then_most_specific_dynamic() {
        let mut idx = SourceIndex::default();
        let segs = |s: &str| -> Vec<String> {
            s.split('/')
                .filter(|x| !x.is_empty())
                .map(String::from)
                .collect()
        };
        idx.add(&segs("about"), "about.vue");
        idx.add(&segs("blog/[...rest]"), "blog/[...rest].vue");
        idx.add(&segs("blog/[slug]"), "blog/[slug].vue");
        idx.add(&segs(""), "index.vue");
        assert_eq!(idx.lookup("/").as_deref(), Some("index.vue"));
        assert_eq!(idx.lookup("/about").as_deref(), Some("about.vue"));
        assert_eq!(idx.lookup("/blog/x").as_deref(), Some("blog/[slug].vue"));
        assert_eq!(
            idx.lookup("/blog/x/y").as_deref(),
            Some("blog/[...rest].vue")
        );
        let routes = idx.server_routes();
        let hit = |p: &str| {
            routes
                .iter()
                .find(|r| r.pattern.is_match(p))
                .map(|r| r.route.clone())
        };
        assert_eq!(hit("/blog/a/b").as_deref(), Some("/blog/[...rest]"));
        assert_eq!(hit("/about/").as_deref(), Some("/about"));
        assert_eq!(hit("/").as_deref(), Some("/"));
        assert_eq!(hit("/nope"), None);
    }

    #[test]
    fn reads_front_matter() {
        let fm = "title: x\nslug: \"/a/b\"\n";
        assert_eq!(front_matter_value(fm, "slug").as_deref(), Some("/a/b"));
        assert_eq!(
            front_matter_value("url = '/x/'", "url").as_deref(),
            Some("/x/")
        );
        assert_eq!(front_matter_value(fm, "id"), None);
    }
}
