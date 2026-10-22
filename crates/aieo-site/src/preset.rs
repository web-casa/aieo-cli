//! Framework presets: where the build lands, which origin and base path the
//! framework config declares, and which source file a page came from.
//!
//! Framework configs are JavaScript; they are never executed. Only literal
//! values are read (`site: "https://…"`); anything computed stays unknown
//! and must come from `--site` / `aieo.toml`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetKind {
    Static,
    Astro,
    Starlight,
}

impl PresetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PresetKind::Static => "static",
            PresetKind::Astro => "astro",
            PresetKind::Starlight => "starlight",
        }
    }

    pub fn parse(s: &str) -> Option<PresetKind> {
        match s {
            "static" => Some(PresetKind::Static),
            "astro" => Some(PresetKind::Astro),
            "starlight" => Some(PresetKind::Starlight),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub kind: PresetKind,
    /// default build directory for the preset (relative to `root`)
    pub default_dist: String,
    pub config_file: Option<PathBuf>,
    /// literal values from the framework config
    pub site: Option<String>,
    pub base: Option<String>,
    pub trailing_slash: Option<String>,
    sources: SourceIndex,
}

const ASTRO_CONFIGS: [&str; 5] = [
    "astro.config.mjs",
    "astro.config.ts",
    "astro.config.js",
    "astro.config.mts",
    "astro.config.cjs",
];

fn has_dependency(root: &Path, name: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join("package.json")) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    ["dependencies", "devDependencies"]
        .iter()
        .any(|k| json.get(k).and_then(|d| d.get(name)).is_some())
}

/// Detect the framework of the project at `root` (or use `forced`).
pub fn detect(root: &Path, forced: Option<PresetKind>) -> Project {
    let config_file = ASTRO_CONFIGS
        .iter()
        .map(|f| root.join(f))
        .find(|p| p.is_file());
    let detected = if config_file.is_some() || has_dependency(root, "astro") {
        if has_dependency(root, "@astrojs/starlight") {
            PresetKind::Starlight
        } else {
            PresetKind::Astro
        }
    } else {
        PresetKind::Static
    };
    let kind = forced.unwrap_or(detected);
    let mut project = Project {
        root: root.to_path_buf(),
        kind,
        default_dist: "dist".into(),
        config_file: None,
        site: None,
        base: None,
        trailing_slash: None,
        sources: SourceIndex::default(),
    };
    if matches!(kind, PresetKind::Astro | PresetKind::Starlight) {
        if let Some(cfg) = &config_file {
            if let Ok(text) = std::fs::read_to_string(cfg) {
                let values = literal_config_values(&text);
                project.site = values.get("site").cloned();
                project.base = values.get("base").cloned().filter(|b| b != "/");
                project.trailing_slash = values.get("trailingSlash").cloned();
                if let Some(out) = values.get("outDir") {
                    project.default_dist = out
                        .trim_start_matches("./")
                        .trim_end_matches('/')
                        .to_string();
                }
            }
        }
        project.config_file = config_file;
        project.sources = SourceIndex::build(root, kind);
    }
    project
}

impl Project {
    /// The source file (relative to the project root) a page URL path was built from.
    pub fn source_for(&self, url_path: &str) -> Option<String> {
        let path = match self.base.as_deref() {
            Some(base) => url_path
                .strip_prefix(base.trim_end_matches('/'))
                .unwrap_or(url_path),
            None => url_path,
        };
        self.sources.lookup(&route_key(path))
    }
}

/// Normalised route: `/A/b/index.html` → `/a/b`, `/` → `/`.
pub fn route_key(path: &str) -> String {
    let mut p = path.trim_end_matches(".html").to_string();
    if let Some(stripped) = p.strip_suffix("/index") {
        p = stripped.to_string();
    }
    let p = p.trim_end_matches('/').to_lowercase();
    if p.is_empty() {
        "/".into()
    } else if p.starts_with('/') {
        p
    } else {
        format!("/{p}")
    }
}

/// github-slugger's per-segment slug (what Astro content collections use).
fn slugify(segment: &str) -> String {
    segment
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

#[derive(Debug, Clone, Default)]
struct SourceIndex {
    exact: HashMap<String, String>,
    /// dynamic routes: segments (`[x]` one, `[...x]` any number) → file
    dynamic: Vec<(Vec<String>, String)>,
}

impl SourceIndex {
    fn build(root: &Path, kind: PresetKind) -> SourceIndex {
        let mut index = SourceIndex::default();
        if kind == PresetKind::Starlight {
            index.add_tree(root, "src/content/docs", &["md", "mdx", "mdoc"], true);
        }
        index.add_tree(root, "src/pages", &["astro", "md", "mdx", "html"], false);
        index
    }

    fn add_tree(&mut self, root: &Path, dir: &str, exts: &[&str], content: bool) {
        let base = root.join(dir);
        if !base.is_dir() {
            return;
        }
        for entry in walkdir::WalkDir::new(&base)
            .sort_by_file_name()
            .into_iter()
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
            let mut segments: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect();
            if segments.last().is_some_and(|s| s == "index") {
                segments.pop();
            }
            if segments.iter().any(|s| s.starts_with('_')) {
                continue; // Astro ignores _-prefixed files
            }
            if content {
                let slug = frontmatter_slug(path).unwrap_or_else(|| {
                    segments
                        .iter()
                        .map(|s| slugify(s))
                        .collect::<Vec<_>>()
                        .join("/")
                });
                self.exact
                    .entry(route_key(&format!("/{slug}")))
                    .or_insert(rel_to_root);
            } else if segments.iter().any(|s| s.starts_with('[')) {
                self.dynamic.push((segments, rel_to_root));
            } else {
                self.exact
                    .entry(route_key(&format!("/{}", segments.join("/"))))
                    .or_insert(rel_to_root);
            }
        }
    }

    fn lookup(&self, key: &str) -> Option<String> {
        if let Some(f) = self.exact.get(key) {
            return Some(f.clone());
        }
        let parts: Vec<&str> = key.split('/').filter(|s| !s.is_empty()).collect();
        self.dynamic
            .iter()
            .find(|(segs, _)| dynamic_match(segs, &parts))
            .map(|(_, f)| f.clone())
    }
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

fn frontmatter_slug(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let rest = text.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    rest[..end].lines().find_map(|l| {
        let v = l
            .trim()
            .strip_prefix("slug:")?
            .trim()
            .trim_matches(|c| c == '"' || c == '\'');
        Some(v.trim_matches('/').to_string())
    })
}

#[derive(Debug, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Punct(char),
}

/// Tokens of a JS/TS source: identifiers, string literals without
/// interpolation, punctuation. Comments are skipped.
fn tokenize(src: &str) -> Vec<Tok> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c == '"' || c == '\'' || c == '`' {
            let quote = c;
            let mut s = String::new();
            let mut interpolated = false;
            i += 1;
            while i < chars.len() && chars[i] != quote {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    s.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                if quote == '`' && chars[i] == '$' && chars.get(i + 1) == Some(&'{') {
                    interpolated = true;
                }
                s.push(chars[i]);
                i += 1;
            }
            i += 1;
            toks.push(if interpolated {
                Tok::Punct('?')
            } else {
                Tok::Str(s)
            });
        } else if c.is_alphanumeric() || c == '_' || c == '$' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
            {
                i += 1;
            }
            toks.push(Tok::Ident(chars[start..i].iter().collect()));
        } else {
            toks.push(Tok::Punct(c));
            i += 1;
        }
    }
    toks
}

/// `key: "literal"` pairs of the object passed to `defineConfig(...)` (or
/// `export default {...}`), top level only.
pub fn literal_config_values(src: &str) -> HashMap<String, String> {
    let toks = tokenize(src);
    let mut target_depth: Option<usize> = None;
    let mut depth = 0usize;
    let mut out = HashMap::new();
    for (i, t) in toks.iter().enumerate() {
        match t {
            Tok::Punct('{') => {
                depth += 1;
                if target_depth.is_none() && i >= 2 {
                    let opener = matches!(
                        (&toks[i - 2], &toks[i - 1]),
                        (Tok::Ident(n), Tok::Punct('(')) if n == "defineConfig"
                    ) || matches!((&toks[i - 2], &toks[i - 1]), (Tok::Ident(a), Tok::Ident(b)) if a == "export" && b == "default");
                    if opener {
                        target_depth = Some(depth);
                    }
                }
            }
            Tok::Punct('}') => depth = depth.saturating_sub(1),
            Tok::Punct(':') if Some(depth) == target_depth && i >= 1 => {
                let key = match &toks[i - 1] {
                    Tok::Ident(k) | Tok::Str(k) => k.clone(),
                    _ => continue,
                };
                if let Some(Tok::Str(v)) = toks.get(i + 1) {
                    if matches!(toks.get(i + 2), Some(Tok::Punct(',' | '}')) | None) {
                        out.entry(key).or_insert_with(|| v.clone());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Production origin the build itself declares: the homepage canonical, a
/// robots.txt `Sitemap:` line, or the first sitemap entry.
pub fn infer_origin(dist: &Path) -> Option<(Url, &'static str)> {
    let read = |name: &str| std::fs::read_to_string(dist.join(name)).ok();
    let origin = |s: &str| {
        Url::parse(s.trim())
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
    };
    if let Some(doc) = read("index.html").and_then(|h| crate::html::Doc::parse(&h)) {
        if let Some(u) = doc.canonical_href().as_deref().and_then(origin) {
            return Some((u, "homepage canonical"));
        }
    }
    if let Some(robots) = read("robots.txt") {
        if let Some(u) = crate::robots::parse_robots(&robots)
            .sitemaps
            .iter()
            .find_map(|s| origin(s))
        {
            return Some((u, "robots.txt Sitemap line"));
        }
    }
    for name in [
        "sitemap-index.xml",
        "sitemap.xml",
        "sitemap-0.xml",
        "sitemap_index.xml",
    ] {
        if let Some(doc) = read(name).and_then(|t| crate::sitemap::parse_sitemap(&t)) {
            if let Some(u) = doc.locs.iter().find_map(|l| origin(l)) {
                return Some((u, "sitemap entry"));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_literal_astro_config_values() {
        let src = r#"
            // site: "https://commented.example"
            import { defineConfig } from 'astro/config';
            import starlight from '@astrojs/starlight';
            export default defineConfig({
              site: 'https://ubuntu.fan',
              base: "/docs",
              trailingSlash: `always`,
              integrations: [starlight({ title: 'X', site: 'https://nested.example' })],
              outDir: process.env.OUT ?? './dist',
            });
        "#;
        let v = literal_config_values(src);
        assert_eq!(
            v.get("site").map(String::as_str),
            Some("https://ubuntu.fan")
        );
        assert_eq!(v.get("base").map(String::as_str), Some("/docs"));
        assert_eq!(v.get("trailingSlash").map(String::as_str), Some("always"));
        assert!(!v.contains_key("outDir"), "computed values stay unknown");
        assert!(
            !literal_config_values("export default defineConfig({ site: `https://${host}` })")
                .contains_key("site")
        );
    }

    #[test]
    fn maps_routes_to_sources() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let write = |rel: &str, text: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        write(
            "package.json",
            r#"{"dependencies":{"astro":"5","@astrojs/starlight":"0.36"}}"#,
        );
        write(
            "astro.config.mjs",
            "export default defineConfig({ site: 'https://e.com' })",
        );
        write("src/content/docs/index.mdx", "# home");
        write("src/content/docs/Guides/Getting Started.md", "# x");
        write(
            "src/content/docs/zh/guides/install.md",
            "---\ntitle: x\n---\n",
        );
        write(
            "src/content/docs/custom.md",
            "---\nslug: elsewhere/page\n---\n",
        );
        write("src/pages/blog/[...slug].astro", "");
        write("src/pages/about.astro", "");
        let p = detect(root, None);
        assert_eq!(p.kind, PresetKind::Starlight);
        assert_eq!(p.site.as_deref(), Some("https://e.com"));
        assert_eq!(
            p.source_for("/").as_deref(),
            Some("src/content/docs/index.mdx")
        );
        assert_eq!(
            p.source_for("/guides/getting-started/").as_deref(),
            Some("src/content/docs/Guides/Getting Started.md")
        );
        assert_eq!(
            p.source_for("/zh/guides/install/").as_deref(),
            Some("src/content/docs/zh/guides/install.md")
        );
        assert_eq!(
            p.source_for("/elsewhere/page/").as_deref(),
            Some("src/content/docs/custom.md")
        );
        assert_eq!(
            p.source_for("/about/").as_deref(),
            Some("src/pages/about.astro")
        );
        assert_eq!(
            p.source_for("/blog/2024/post/").as_deref(),
            Some("src/pages/blog/[...slug].astro")
        );
        assert_eq!(p.source_for("/missing/"), None);
    }
}
