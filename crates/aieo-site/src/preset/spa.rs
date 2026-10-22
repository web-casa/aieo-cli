//! Vite apps whose routes live in the client: Vue (vue-router) and
//! TanStack Router, plus TanStack Start, which can prerender pages and keep
//! a server for the rest.
//!
//! Vue routes are code (`createRouter({ routes })`) unless a file-based
//! router uses `src/pages/**`. TanStack's file routes are `src/routes/**`:
//! `index` = the directory, `$param`, `$` (splat), `_pathless` layouts,
//! `(group)` folders, `posts.$id` flat dots, `route` files, `-ignored`.

use std::path::Path;

use super::jsconfig::read_config;
use super::routes::walk_sources;
use super::{clean_dir, first_file, has_dependency, Build, Project, Rendering};

const VITE_CONFIGS: [&str; 6] = [
    "vite.config.ts",
    "vite.config.js",
    "vite.config.mts",
    "vite.config.mjs",
    "app.config.ts",
    "app.config.js",
];

const START: [&str; 3] = [
    "@tanstack/react-start",
    "@tanstack/solid-start",
    "@tanstack/start",
];

pub(super) fn vue_detect(root: &Path) -> bool {
    has_dependency(root, &["vue"]) && first_file(root, &VITE_CONFIGS[..4]).is_some()
}

pub(super) fn tanstack_detect(root: &Path) -> bool {
    has_dependency(root, &START)
        || has_dependency(
            root,
            &[
                "@tanstack/react-router",
                "@tanstack/solid-router",
                "@tanstack/vue-router",
            ],
        ) && first_file(root, &VITE_CONFIGS).is_some()
}

fn vite_config(p: &mut Project) {
    p.config = read_config(&p.root, &VITE_CONFIGS);
    p.take_base("base");
    if let Some(out) = p.config.as_ref().and_then(|c| c.get("build.outDir")) {
        p.default_dist = clean_dir(out);
    }
}

pub(super) fn vue_configure(p: &mut Project) {
    vite_config(p);
    // vite-ssg prerenders every route to HTML; a plain Vite build is a shell
    p.rendering = if has_dependency(&p.root, &["vite-ssg"]) {
        Rendering::Static
    } else {
        Rendering::Spa
    };
    for (file, mut segments) in walk_sources(&p.root, "src/pages", &["vue", "md"]) {
        if segments.last().is_some_and(|s| s == "index") {
            segments.pop();
        }
        let segments: Vec<String> = segments
            .iter()
            .filter(|s| !(s.starts_with('(') && s.ends_with(')')))
            .map(|s| s.replace("[[...", "[...").replace("]]", "]"))
            .collect();
        p.sources.add(&segments, &file);
    }
    if p.sources.is_empty() {
        p.notes.push(
            "routes are defined in code (vue-router); findings point at build files, not sources"
                .into(),
        );
    }
}

/// One TanStack file-route path part → canonical segments (None = no URL segment).
fn tanstack_part(part: &str) -> Option<String> {
    if part.starts_with('_') || (part.starts_with('(') && part.ends_with(')')) {
        return None; // pathless layout / group
    }
    if part == "$" {
        return Some("[...splat]".into());
    }
    if let Some(name) = part.strip_prefix('$') {
        return Some(format!("[{name}]"));
    }
    if part.starts_with("{-$") {
        return Some(format!("[...{}]", part.trim_matches(['{', '}', '-', '$'])));
    }
    // `posts_` opts out of nesting under `posts`; the URL keeps `posts`
    Some(part.trim_end_matches('_').replace("[.]", "."))
}

pub(super) fn tanstack_configure(p: &mut Project) {
    vite_config(p);
    let start = has_dependency(&p.root, &START);
    if start {
        p.default_dist = [".output/public", "dist/client"]
            .into_iter()
            .find(|d| p.root.join(d).is_dir())
            .unwrap_or(".output/public")
            .to_string();
        p.rendering = Rendering::Static;
    } else {
        p.rendering = Rendering::Spa;
    }
    let dir = ["src/routes", "app/routes"]
        .into_iter()
        .find(|d| p.root.join(d).is_dir())
        .unwrap_or("src/routes");
    for (file, segments) in walk_sources(&p.root, dir, &["tsx", "ts", "jsx", "js"]) {
        if segments.iter().any(|s| s.starts_with('-')) {
            continue;
        }
        // a pathless layout file (`_layout.tsx`) renders no page of its own
        let leaf = segments.last().and_then(|s| {
            s.trim_end_matches(".lazy")
                .rsplit('.')
                .next()
                .map(String::from)
        });
        if leaf
            .as_deref()
            .is_some_and(|l| l.starts_with('_') && l != "__root")
        {
            continue;
        }
        let mut parts: Vec<String> = Vec::new();
        let last = segments.len() - 1;
        for (i, seg) in segments.iter().enumerate() {
            let seg = if i == last {
                seg.trim_end_matches(".lazy").to_string()
            } else {
                seg.clone()
            };
            if seg == "__root" {
                parts.clear();
                parts.push("\u{0}".into());
                break;
            }
            for (j, part) in seg.split('.').enumerate() {
                let n = seg.split('.').count();
                if i == last && j == n - 1 && (part == "index" || part == "route") {
                    continue;
                }
                parts.extend(tanstack_part(part));
            }
        }
        if parts.first().is_some_and(|s| s == "\u{0}") {
            continue;
        }
        p.sources.add(&parts, &file);
    }
}

pub(super) fn tanstack_build(p: &Project, dist: &Path, build: &mut Build) {
    let output = dist.parent().unwrap_or(dist);
    let server =
        output.join("server").is_dir() && (dist.ends_with("public") || dist.ends_with("client"));
    if server && p.rendering != Rendering::Spa {
        build.rendering = Some(Rendering::Hybrid);
        build.options.server_routes = p.sources.server_routes();
        build.server_label = Some("TanStack Start server build");
    } else {
        build.rendering = Some(p.rendering);
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::write;
    use super::super::*;

    #[test]
    fn tanstack_file_routes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "package.json",
            r#"{"dependencies":{"@tanstack/react-start":"1.0.0"}}"#,
        );
        write(
            root,
            "vite.config.ts",
            "export default defineConfig({ plugins: [] })",
        );
        for f in [
            "__root.tsx",
            "index.tsx",
            "about.tsx",
            "guide.getting-started.tsx",
            "blog.$slug.tsx",
            "_layout.tsx",
            "_layout/settings.tsx",
            "(marketing)/pricing.lazy.tsx",
            "files/$.tsx",
            "posts/route.tsx",
            "posts/index.tsx",
            "-components/x.tsx",
        ] {
            write(root, &format!("src/routes/{f}"), "");
        }
        let p = detect(root, None);
        assert_eq!(p.kind, PresetKind::Tanstack);
        assert_eq!(p.rendering, Rendering::Static);
        let s = |u: &str| p.source_for(u);
        assert_eq!(s("/").as_deref(), Some("src/routes/index.tsx"));
        assert_eq!(s("/about").as_deref(), Some("src/routes/about.tsx"));
        assert_eq!(
            s("/guide/getting-started").as_deref(),
            Some("src/routes/guide.getting-started.tsx")
        );
        assert_eq!(
            s("/blog/hello").as_deref(),
            Some("src/routes/blog.$slug.tsx")
        );
        assert_eq!(
            s("/settings").as_deref(),
            Some("src/routes/_layout/settings.tsx")
        );
        assert_eq!(
            s("/pricing").as_deref(),
            Some("src/routes/(marketing)/pricing.lazy.tsx")
        );
        assert_eq!(s("/files/a/b").as_deref(), Some("src/routes/files/$.tsx"));
        assert_eq!(s("/posts").as_deref(), Some("src/routes/posts/index.tsx"));
    }

    #[test]
    fn vue_spa() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "package.json",
            r#"{"dependencies":{"vue":"3.5.0"},"devDependencies":{"vite":"7"}}"#,
        );
        write(
            root,
            "vite.config.ts",
            "export default defineConfig({\n  base: '/app/',\n  build: { outDir: 'build' },\n})",
        );
        let p = detect(root, None);
        assert_eq!(p.kind, PresetKind::Vue);
        assert_eq!(p.rendering, Rendering::Spa);
        assert_eq!(p.base.as_deref(), Some("/app"));
        assert_eq!(p.default_dist, "build");
        assert_eq!(p.notes.len(), 1);
    }
}
