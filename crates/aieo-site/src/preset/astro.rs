//! Astro and Starlight: `astro.config.*` (`site`, `base`, `trailingSlash`,
//! `outDir`); pages from `src/pages/**`, Starlight docs from
//! `src/content/docs/**` (github-slugger slugs, front-matter `slug`).

use std::path::Path;

use super::jsconfig::read_config;
use super::routes::{front_matter, front_matter_value, slugify, walk_sources};
use super::{clean_dir, first_file, has_dependency, PresetKind, Project, TrailingSlash};

const CONFIGS: [&str; 5] = [
    "astro.config.mjs",
    "astro.config.ts",
    "astro.config.js",
    "astro.config.mts",
    "astro.config.cjs",
];

pub(super) fn detect(root: &Path) -> bool {
    first_file(root, &CONFIGS).is_some() || has_dependency(root, &["astro"])
}

pub(super) fn configure(p: &mut Project) {
    p.config = read_config(&p.root, &CONFIGS);
    p.take_site("site");
    p.take_base("base");
    match p.config.as_ref().and_then(|c| c.get("trailingSlash")) {
        Some("always") => p.set_trailing_slash(TrailingSlash::Always, "trailingSlash"),
        Some("never") => p.set_trailing_slash(TrailingSlash::Never, "trailingSlash"),
        _ => {}
    }
    if let Some(out) = p.config.as_ref().and_then(|c| c.get("outDir")) {
        p.default_dist = clean_dir(out);
    }
    if p.kind == PresetKind::Starlight {
        for (file, segments) in walk_sources(&p.root, "src/content/docs", &["md", "mdx", "mdoc"]) {
            let mut segments = segments;
            if segments.last().is_some_and(|s| s == "index") {
                segments.pop();
            }
            let slug = front_matter(&p.root.join(&file))
                .and_then(|f| front_matter_value(&f, "slug"))
                .map(|s| s.trim_matches('/').to_string())
                .unwrap_or_else(|| {
                    segments
                        .iter()
                        .map(|s| slugify(s))
                        .collect::<Vec<_>>()
                        .join("/")
                });
            let parts: Vec<String> = slug
                .split('/')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            p.sources.add(&parts, &file);
        }
    }
    for (file, mut segments) in walk_sources(&p.root, "src/pages", &["astro", "md", "mdx", "html"])
    {
        if segments.last().is_some_and(|s| s == "index") {
            segments.pop();
        }
        if segments.iter().any(|s| s.starts_with('_')) {
            continue; // Astro ignores _-prefixed files
        }
        p.sources.add(&segments, &file);
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::write;
    use super::super::*;

    #[test]
    fn maps_routes_to_sources() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "package.json",
            r#"{"dependencies":{"astro":"5","@astrojs/starlight":"0.36"}}"#,
        );
        write(
            root,
            "astro.config.mjs",
            "export default defineConfig({\n  site: 'https://e.com',\n  trailingSlash: 'always',\n})",
        );
        write(root, "src/content/docs/index.mdx", "# home");
        write(root, "src/content/docs/Guides/Getting Started.md", "# x");
        write(
            root,
            "src/content/docs/zh/guides/install.md",
            "---\ntitle: x\n---\n",
        );
        write(
            root,
            "src/content/docs/custom.md",
            "---\nslug: elsewhere/page\n---\n",
        );
        write(root, "src/pages/blog/[...slug].astro", "");
        write(root, "src/pages/about.astro", "");
        write(root, "src/pages/_draft.astro", "");
        let p = detect(root, None);
        assert_eq!(p.kind, PresetKind::Starlight);
        assert_eq!(p.site.as_deref(), Some("https://e.com"));
        assert_eq!(p.trailing_slash, Some(TrailingSlash::Always));
        assert_eq!(
            p.config_ref("trailingSlash"),
            Some(("astro.config.mjs".to_string(), 3))
        );
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
        assert_eq!(p.source_for("/_draft"), None);
    }
}
