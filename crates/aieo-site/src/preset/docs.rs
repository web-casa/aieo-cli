//! Documentation generators.
//!
//! VitePress: the project lives where `.vitepress/` is (the root or
//! `docs/`). Config `.vitepress/config.*`: `base`, `cleanUrls`, `srcDir`,
//! `outDir`, `sitemap.hostname`. Every `*.md` under `srcDir` is a page.
//!
//! Docusaurus: `docusaurus.config.*`: `url`, `baseUrl`, `trailingSlash`.
//! Docs under `docs/` (served at `/docs`, number prefixes stripped,
//! front-matter `id` / `slug`), blog posts under `blog/`
//! (`2024-05-01-name.md` → `/blog/2024/05/01/name`), pages under `src/pages/`.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::jsconfig::read_config;
use super::routes::{front_matter, front_matter_value, walk_sources};
use super::{clean_dir, first_file, has_dependency, Project, TrailingSlash};

const VP_CONFIGS: [&str; 4] = ["config.mts", "config.ts", "config.mjs", "config.js"];

/// Directory (relative to the root) holding `.vitepress/`.
fn vitepress_root(root: &Path) -> Option<String> {
    if root.join(".vitepress").is_dir() {
        return Some(String::new());
    }
    if root.join("docs/.vitepress").is_dir() {
        return Some("docs".into());
    }
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter(|e| e.path().join(".vitepress").is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n != "node_modules")
        .min()
}

fn join(dir: &str, rel: &str) -> String {
    let rel = clean_dir(rel);
    match (dir.is_empty(), rel.is_empty() || rel == ".") {
        (true, _) => rel,
        (false, true) => dir.to_string(),
        (false, false) => format!("{dir}/{rel}"),
    }
}

pub(super) fn vitepress_detect(root: &Path) -> bool {
    vitepress_root(root).is_some_and(|d| {
        let names: Vec<String> = VP_CONFIGS
            .iter()
            .map(|n| join(&d, &format!(".vitepress/{n}")))
            .collect();
        names.iter().any(|n| root.join(n).is_file())
    }) || has_dependency(root, &["vitepress"])
}

pub(super) fn vitepress_configure(p: &mut Project) {
    let vp = vitepress_root(&p.root).unwrap_or_default();
    let names: Vec<String> = VP_CONFIGS
        .iter()
        .map(|n| join(&vp, &format!(".vitepress/{n}")))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    p.config = read_config(&p.root, &refs);
    p.take_site("sitemap.hostname");
    p.take_base("base");
    if p.config.as_ref().and_then(|c| c.bool("cleanUrls")) == Some(true) {
        p.set_trailing_slash(TrailingSlash::Never, "cleanUrls");
    }
    let out = p
        .config
        .as_ref()
        .and_then(|c| c.get("outDir"))
        .map(String::from);
    p.default_dist = join(&vp, out.as_deref().unwrap_or(".vitepress/dist"));
    let src = p
        .config
        .as_ref()
        .and_then(|c| c.get("srcDir"))
        .map(String::from);
    let src = join(&vp, src.as_deref().unwrap_or("."));
    p.sources_root = Some(src.clone());
    for (file, mut segments) in walk_sources(&p.root, &src, &["md"]) {
        if segments
            .first()
            .is_some_and(|s| s == "node_modules" || s == "public")
        {
            continue;
        }
        if segments.last().is_some_and(|s| s == "index") {
            segments.pop();
        }
        p.sources.add(&segments, &file);
    }
}

const DOCUSAURUS_CONFIGS: [&str; 4] = [
    "docusaurus.config.js",
    "docusaurus.config.ts",
    "docusaurus.config.mjs",
    "docusaurus.config.mts",
];

pub(super) fn docusaurus_detect(root: &Path) -> bool {
    first_file(root, &DOCUSAURUS_CONFIGS).is_some() || has_dependency(root, &["@docusaurus/core"])
}

static NUMBER_PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+\s*[-_.]+\s*").unwrap());
static BLOG_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{4})[-/](\d{2})[-/](\d{2})[-/]?(.*)$").unwrap());

fn split(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

pub(super) fn docusaurus_configure(p: &mut Project) {
    p.config = read_config(&p.root, &DOCUSAURUS_CONFIGS);
    p.take_site("url");
    p.take_base("baseUrl");
    match p.config.as_ref().and_then(|c| c.bool("trailingSlash")) {
        Some(true) => p.set_trailing_slash(TrailingSlash::Always, "trailingSlash"),
        Some(false) => p.set_trailing_slash(TrailingSlash::Never, "trailingSlash"),
        None => {}
    }
    p.default_dist = "build".into();

    for (file, segments) in walk_sources(&p.root, "docs", &["md", "mdx"]) {
        let front = front_matter(&p.root.join(&file)).unwrap_or_default();
        let mut dirs: Vec<String> = segments[..segments.len() - 1]
            .iter()
            .map(|s| NUMBER_PREFIX.replace(s, "").into_owned())
            .collect();
        let name = NUMBER_PREFIX
            .replace(segments.last().expect("a file"), "")
            .into_owned();
        let route = if let Some(slug) = front_matter_value(&front, "slug") {
            if slug.starts_with('/') {
                split(&slug)
            } else {
                dirs.extend(split(&slug));
                dirs
            }
        } else {
            let is_index = name.eq_ignore_ascii_case("index")
                || name.eq_ignore_ascii_case("readme")
                || dirs.last().is_some_and(|d| d.eq_ignore_ascii_case(&name));
            if !is_index {
                dirs.push(front_matter_value(&front, "id").unwrap_or(name));
            }
            dirs
        };
        let mut full = vec!["docs".to_string()];
        full.extend(route);
        p.sources.add(&full, &file);
    }

    for (file, segments) in walk_sources(&p.root, "blog", &["md", "mdx"]) {
        let joined = segments.join("/");
        let joined = joined.strip_suffix("/index").unwrap_or(&joined);
        let front = front_matter(&p.root.join(&file)).unwrap_or_default();
        let mut route = vec!["blog".to_string()];
        if let Some(slug) = front_matter_value(&front, "slug") {
            route.extend(split(&slug));
        } else if let Some(c) = BLOG_DATE.captures(joined) {
            route.extend([c[1].to_string(), c[2].to_string(), c[3].to_string()]);
            route.extend(split(&c[4]));
        } else {
            route.extend(split(joined));
        }
        p.sources.add(&route, &file);
    }

    for (file, mut segments) in walk_sources(
        &p.root,
        "src/pages",
        &["js", "jsx", "ts", "tsx", "md", "mdx"],
    ) {
        if segments.iter().any(|s| s.starts_with('_')) {
            continue;
        }
        if segments.last().is_some_and(|s| s == "index") {
            segments.pop();
        }
        p.sources.add(&segments, &file);
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::write;
    use super::super::*;

    #[test]
    fn vitepress_under_docs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "package.json",
            r#"{"devDependencies":{"vitepress":"1.6.4"}}"#,
        );
        write(
            root,
            "docs/.vitepress/config.mts",
            "export default defineConfig({\n  cleanUrls: true,\n  sitemap: { hostname: 'https://example.com' },\n})",
        );
        write(root, "docs/index.md", "");
        write(root, "docs/guide/getting-started.md", "");
        let p = detect(root, None);
        assert_eq!(p.kind, PresetKind::Vitepress);
        assert_eq!(p.default_dist, "docs/.vitepress/dist");
        assert_eq!(p.site.as_deref(), Some("https://example.com"));
        assert_eq!(p.trailing_slash, Some(TrailingSlash::Never));
        assert_eq!(
            p.config_ref("cleanUrls"),
            Some(("docs/.vitepress/config.mts".into(), 2))
        );
        assert_eq!(p.source_for("/").as_deref(), Some("docs/index.md"));
        assert_eq!(p.public_dir().as_deref(), Some("docs/public"));
        assert_eq!(
            p.source_for("/guide/getting-started.html").as_deref(),
            Some("docs/guide/getting-started.md")
        );
    }

    #[test]
    fn docusaurus_docs_blog_pages() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "package.json",
            r#"{"dependencies":{"@docusaurus/core":"3.10.2"}}"#,
        );
        write(
            root,
            "docusaurus.config.ts",
            "const config: Config = {\n  url: 'https://example.com',\n  baseUrl: '/',\n  trailingSlash: false,\n};\nexport default config;",
        );
        write(root, "docs/intro.md", "");
        write(root, "docs/guide/01-getting-started.md", "");
        write(
            root,
            "docs/guide/02-config.md",
            "---\nid: configuration\n---\n",
        );
        write(root, "docs/ref/index.md", "");
        write(root, "docs/other.md", "---\nslug: /elsewhere\n---\n");
        write(root, "blog/2024-05-01-hello-world.md", "");
        write(
            root,
            "blog/2024-06-01-second/index.md",
            "---\nslug: second-post\n---\n",
        );
        write(root, "src/pages/about.md", "");
        write(root, "src/pages/index.tsx", "");
        let p = detect(root, None);
        assert_eq!(p.kind, PresetKind::Docusaurus);
        assert_eq!(p.base, None);
        assert_eq!(p.trailing_slash, Some(TrailingSlash::Never));
        let s = |u: &str| p.source_for(u);
        assert_eq!(s("/docs/intro").as_deref(), Some("docs/intro.md"));
        assert_eq!(
            s("/docs/guide/getting-started").as_deref(),
            Some("docs/guide/01-getting-started.md")
        );
        assert_eq!(
            s("/docs/guide/configuration").as_deref(),
            Some("docs/guide/02-config.md")
        );
        assert_eq!(s("/docs/ref").as_deref(), Some("docs/ref/index.md"));
        assert_eq!(s("/docs/elsewhere").as_deref(), Some("docs/other.md"));
        assert_eq!(
            s("/blog/2024/05/01/hello-world").as_deref(),
            Some("blog/2024-05-01-hello-world.md")
        );
        assert_eq!(
            s("/blog/second-post").as_deref(),
            Some("blog/2024-06-01-second/index.md")
        );
        assert_eq!(s("/about").as_deref(), Some("src/pages/about.md"));
        assert_eq!(s("/").as_deref(), Some("src/pages/index.tsx"));
    }
}
