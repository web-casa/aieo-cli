//! Next.js. `output: 'export'` builds a static site into `out/`; any other
//! build leaves a server build in `.next/`, which is served here as
//! production would serve it:
//!
//! - prerendered pages from `.next/server/app/**.html` (App Router) and
//!   `.next/server/pages/**.html` (Pages Router), static metadata routes
//!   (`robots.txt.body`), `public/**` at the root;
//! - redirects, rewrites and headers from `.next/routes-manifest.json`,
//!   Next's own trailing-slash redirect included;
//! - every other route of `app-path-routes-manifest.json` /
//!   `server/pages-manifest.json` as a server-rendered route, except
//!   dynamic routes whose unknown params 404 (`dynamicParams = false`).
//!
//! Sources: `app/**/page.*` and `pages/**` (or under `src/`), with route
//! groups `(x)`, parallel `@slots` and private `_folders` handled.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::jsconfig::read_config;
use super::routes::{route_pattern, walk_sources};
use super::{first_file, has_dependency, Build, IgnoredSetting, Project, Rendering, TrailingSlash};
use crate::hosting::HostConfig;
use crate::site::walk_files;

const CONFIGS: [&str; 5] = [
    "next.config.js",
    "next.config.mjs",
    "next.config.ts",
    "next.config.cjs",
    "next.config.mts",
];

pub(super) fn detect(root: &Path) -> bool {
    first_file(root, &CONFIGS).is_some() || has_dependency(root, &["next"])
}

pub(super) fn configure(p: &mut Project) {
    p.config = read_config(&p.root, &CONFIGS);
    p.take_base("basePath");
    // Next redirects `/a/` → `/a` unless `trailingSlash: true` (then the reverse)
    match p.config.as_ref().and_then(|c| c.bool("trailingSlash")) {
        Some(true) => p.set_trailing_slash(TrailingSlash::Always, "trailingSlash"),
        Some(false) => p.set_trailing_slash(TrailingSlash::Never, "trailingSlash"),
        None if p.config.as_ref().is_some_and(|c| !c.has("trailingSlash")) => {
            p.set_trailing_slash(TrailingSlash::Never, "trailingSlash (default false)")
        }
        None => {}
    }
    let output = p
        .config
        .as_ref()
        .and_then(|c| c.get("output"))
        .map(String::from);
    let dist_dir = p
        .config
        .as_ref()
        .and_then(|c| c.get("distDir"))
        .map(super::clean_dir)
        .unwrap_or_else(|| ".next".into());
    let export = output.as_deref() == Some("export")
        || (output.is_none()
            && p.root.join("out").is_dir()
            && !p
                .root
                .join(&dist_dir)
                .join("routes-manifest.json")
                .is_file());
    if export {
        p.default_dist = "out".into();
        p.rendering = Rendering::Static;
        if let Some(c) = &p.config {
            for key in ["redirects", "rewrites", "headers"] {
                if let Some(line) = c.line(key) {
                    p.ignored.push(IgnoredSetting {
                        key: key.into(),
                        line,
                        why: "static export (`output: 'export'`) does not apply it; configure it on the host instead".into(),
                    });
                }
            }
        }
    } else {
        p.default_dist = dist_dir;
        p.rendering = Rendering::Hybrid;
    }
    if let Some((site, file, line)) = metadata_base(&p.root) {
        p.site = Some(site);
        p.site_key = Some("metadataBase".into());
        p.site_at = Some((file, line));
    }
    index_sources(p);
}

/// `metadataBase: new URL("https://…")` in the root layout: the origin, the
/// layout file and the line.
fn metadata_base(root: &Path) -> Option<(String, String, usize)> {
    let re =
        regex::Regex::new(r#"metadataBase\s*:\s*new\s+URL\(\s*["'`](https?://[^"'`$]+)["'`]\s*\)"#)
            .ok()?;
    for dir in ["app", "src/app"] {
        for ext in ["tsx", "jsx", "ts", "js"] {
            let Ok(text) = std::fs::read_to_string(root.join(dir).join(format!("layout.{ext}")))
            else {
                continue;
            };
            if let Some(c) = re.captures(&text) {
                let line = text[..c.get(0)?.start()].matches('\n').count() + 1;
                return Some((c[1].to_string(), format!("{dir}/layout.{ext}"), line));
            }
        }
    }
    None
}

/// `[[...x]]` → `[...x]`; route groups and slots disappear.
fn app_segments(dirs: &[String]) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for seg in dirs {
        if seg.starts_with('@') || seg.starts_with('_') || seg.starts_with("(.") {
            return None; // parallel slot, private folder, intercepting route
        }
        if seg.starts_with('(') && seg.ends_with(')') {
            continue;
        }
        out.push(seg.replace("[[...", "[...").replace("]]", "]"));
    }
    Some(out)
}

fn index_sources(p: &mut Project) {
    for base in ["app", "src/app"] {
        for (file, segments) in
            walk_sources(&p.root, base, &["tsx", "ts", "jsx", "js", "mdx", "md"])
        {
            if segments.last().map(String::as_str) != Some("page") {
                continue;
            }
            if let Some(route) = app_segments(&segments[..segments.len() - 1]) {
                p.sources.add(&route, &file);
            }
        }
    }
    for base in ["pages", "src/pages"] {
        for (file, mut segments) in
            walk_sources(&p.root, base, &["tsx", "ts", "jsx", "js", "mdx", "md"])
        {
            if segments.first().is_some_and(|s| s == "api")
                || segments.iter().any(|s| s.starts_with('_'))
            {
                continue;
            }
            if segments.last().is_some_and(|s| s == "index") {
                segments.pop();
            }
            let segments: Vec<String> = segments
                .iter()
                .map(|s| s.replace("[[...", "[...").replace("]]", "]"))
                .collect();
            p.sources.add(&segments, &file);
        }
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Published path for a file of `.next/server/app`: `about.html` → itself,
/// `robots.txt.body` → `robots.txt`, `_not-found.html` → `404.html`.
fn app_output(rel: &str) -> Option<String> {
    if let Some(stem) = rel.strip_suffix(".html") {
        return match stem {
            "_not-found" => Some("404.html".into()),
            s if s.starts_with('_') || s.ends_with("/_not-found") => None,
            _ => Some(rel.to_string()),
        };
    }
    let published = rel.strip_suffix(".body")?;
    // only static metadata files (robots.txt, sitemap.xml, manifest, llms.txt …)
    published.contains('.').then(|| published.to_string())
}

pub(super) fn build(p: &Project, dist: &Path, build: &mut Build, host: &mut HostConfig) {
    let manifest = dist.join("routes-manifest.json");
    if dist.join("server").is_dir() && !dist.join("BUILD_ID").is_file() {
        build.problem = Some(format!(
            "{} holds a development build (no BUILD_ID); run `next build` first",
            dist.display()
        ));
        return;
    }
    if !manifest.is_file() {
        // a static export (or something else): served as is
        build.rendering = Some(Rendering::Static);
        return;
    }
    build.rendering = Some(Rendering::Hybrid);
    let rel = |path: &Path| {
        path.strip_prefix(&p.root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().into_owned())
    };
    if let Ok(text) = std::fs::read_to_string(&manifest) {
        if let Some(c) = HostConfig::next_manifest(&text, &rel(&manifest)) {
            host.extend(c);
        }
    }

    let mut layout: Vec<(String, PathBuf)> = Vec::new();
    let public = p.root.join("public");
    if public.is_dir() {
        for f in walk_files(&public).unwrap_or_default() {
            let path = public.join(&f);
            layout.push((f, path));
        }
    }
    let app = dist.join("server/app");
    for f in walk_files(&app).unwrap_or_default() {
        if let Some(published) = app_output(&f) {
            let path = app.join(&f);
            layout.push((published, path));
        }
    }
    let pages = dist.join("server/pages");
    for f in walk_files(&pages).unwrap_or_default() {
        if f.ends_with(".html") && !f.starts_with('_') && f != "500.html" {
            let path = pages.join(&f);
            layout.push((f, path));
        }
    }
    let statics = dist.join("static");
    for f in walk_files(&statics).unwrap_or_default() {
        let path = statics.join(&f);
        layout.push((format!("_next/static/{f}"), path));
    }
    // a file in public/ wins over a generated one with the same path, as in Next
    let mut seen = std::collections::HashSet::new();
    layout.retain(|(rel, _)| seen.insert(rel.clone()));
    build.options.layout = Some(layout);

    // routes the server renders on request
    let mut routes: Vec<String> = Vec::new();
    if let Some(Value::Object(m)) = read_json(&dist.join("app-path-routes-manifest.json")) {
        routes.extend(m.values().filter_map(Value::as_str).map(String::from));
    }
    if let Some(Value::Object(m)) = read_json(&dist.join("server/pages-manifest.json")) {
        routes.extend(m.keys().cloned());
    }
    let closed: Vec<String> = read_json(&dist.join("prerender-manifest.json"))
        .and_then(|v| v.get("dynamicRoutes").cloned())
        .and_then(|v| match v {
            Value::Object(m) => Some(
                m.into_iter()
                    .filter(|(_, r)| r.get("fallback") == Some(&Value::Bool(false)))
                    .map(|(k, _)| k)
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default();
    routes.sort();
    routes.dedup();
    let mut server = Vec::new();
    for route in routes {
        let internal = route
            .split('/')
            .any(|s| s.starts_with('_') && !s.starts_with("_next"))
            || matches!(route.as_str(), "/404" | "/500");
        if internal || closed.contains(&route) {
            continue;
        }
        if let Some(r) = route_pattern(&route) {
            server.push(r);
        }
    }
    build.options.server_routes = server;
    build.server_label = Some("Next.js server build");
    if dist.join("server/middleware.js").is_file() || dist.join("server/proxy.js").is_file() {
        build.notes.push(
            "Next.js middleware / proxy runs on every request; its redirects and rewrites (locale redirects at /, for one) are not modelled".into(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::write;
    use super::super::*;

    #[test]
    fn export_config_and_sources() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "package.json",
            r#"{"dependencies":{"next":"16.0.0"}}"#,
        );
        write(
            root,
            "next.config.ts",
            "const nextConfig = {\n  output: 'export',\n  async redirects() { return [] },\n};\nexport default nextConfig;\n",
        );
        write(
            root,
            "app/layout.tsx",
            "export const metadata = { metadataBase: new URL('https://example.com') };",
        );
        write(root, "app/page.tsx", "");
        write(root, "app/(marketing)/about/page.tsx", "");
        write(root, "app/blog/[slug]/page.tsx", "");
        write(root, "app/docs/[[...path]]/page.mdx", "");
        write(root, "app/@modal/login/page.tsx", "");
        write(root, "app/_lib/page.tsx", "");
        let p = detect(root, None);
        assert_eq!(p.kind, PresetKind::Next);
        assert_eq!(p.default_dist, "out");
        assert_eq!(p.site.as_deref(), Some("https://example.com"));
        assert_eq!(p.trailing_slash, Some(TrailingSlash::Never));
        assert_eq!(p.ignored.len(), 1);
        assert_eq!(
            (p.ignored[0].key.as_str(), p.ignored[0].line),
            ("redirects", 3)
        );
        assert_eq!(p.source_for("/").as_deref(), Some("app/page.tsx"));
        assert_eq!(
            p.source_for("/about").as_deref(),
            Some("app/(marketing)/about/page.tsx")
        );
        assert_eq!(
            p.source_for("/blog/x").as_deref(),
            Some("app/blog/[slug]/page.tsx")
        );
        assert_eq!(
            p.source_for("/docs").as_deref(),
            Some("app/docs/[[...path]]/page.mdx")
        );
        assert_eq!(p.source_for("/login"), None);
    }

    #[test]
    fn serves_a_server_build() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "package.json",
            r#"{"dependencies":{"next":"16.0.0"}}"#,
        );
        write(
            root,
            "next.config.mjs",
            "export default { trailingSlash: false }",
        );
        write(root, "public/robots.txt", "User-agent: *\nAllow: /\n");
        write(root, ".next/BUILD_ID", "abc");
        write(
            root,
            ".next/server/app/index.html",
            "<html><title>Home</title></html>",
        );
        write(
            root,
            ".next/server/app/about.html",
            "<html><title>About</title></html>",
        );
        write(
            root,
            ".next/server/app/blog/hello-world.html",
            "<html></html>",
        );
        write(root, ".next/server/app/_not-found.html", "<html>404</html>");
        write(
            root,
            ".next/server/app/sitemap.xml.body",
            "<urlset></urlset>",
        );
        write(root, ".next/server/app/about.rsc", "");
        write(root, ".next/static/chunks/main.js", "");
        write(
            root,
            ".next/routes-manifest.json",
            r#"{"redirects":[{"source":"/:path+/","destination":"/:path+","internal":true,"statusCode":308},{"source":"/old-page","destination":"/about","statusCode":308}],"headers":[],"rewrites":[]}"#,
        );
        write(
            root,
            ".next/app-path-routes-manifest.json",
            r#"{"/page":"/","/about/page":"/about","/blog/[slug]/page":"/blog/[slug]","/search/page":"/search","/_not-found/page":"/_not-found","/docs/[slug]/page":"/docs/[slug]"}"#,
        );
        write(
            root,
            ".next/prerender-manifest.json",
            r#"{"routes":{},"dynamicRoutes":{"/docs/[slug]":{"fallback":false},"/blog/[slug]":{"fallback":null}}}"#,
        );
        let p = detect(root, None);
        assert_eq!(p.default_dist, ".next");
        let dist = root.join(".next");
        let b = p.build(&dist);
        assert_eq!(b.problem, None);
        assert_eq!(b.rendering, Some(Rendering::Hybrid));
        let site = crate::Site::load(
            &dist,
            url::Url::parse("https://example.com").unwrap(),
            b.options,
        )
        .unwrap();
        use crate::Resolution;
        assert_eq!(
            site.resolve_path("/"),
            Resolution::File("index.html".into())
        );
        assert_eq!(
            site.resolve_path("/about"),
            Resolution::File("about.html".into())
        );
        assert_eq!(
            site.resolve_path("/robots.txt"),
            Resolution::File("robots.txt".into())
        );
        assert_eq!(
            site.resolve_path("/sitemap.xml"),
            Resolution::File("sitemap.xml".into())
        );
        assert!(site.has_file("404.html"));
        assert!(site.has_file("_next/static/chunks/main.js"));
        assert!(!site.has_file("about.rsc"));
        assert_eq!(
            site.resolve_path("/search"),
            Resolution::Server("/search".into())
        );
        assert_eq!(
            site.resolve_path("/blog/other"),
            Resolution::Server("/blog/[slug]".into())
        );
        assert_eq!(
            site.resolve_path("/docs/other"),
            Resolution::Missing,
            "dynamicParams = false"
        );
        assert_eq!(site.resolve_path("/nope"), Resolution::Missing);
        assert!(matches!(
            site.resolve_path("/about/"),
            Resolution::Redirect { status: 308, .. }
        ));
        assert!(matches!(
            site.resolve_path("/old-page"),
            Resolution::Redirect { status: 308, .. }
        ));
    }

    #[test]
    fn rejects_a_development_build() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "package.json",
            r#"{"dependencies":{"next":"16.0.0"}}"#,
        );
        write(root, ".next/server/app-paths-manifest.json", "{}");
        write(root, ".next/routes-manifest.json", "{}");
        let p = detect(root, None);
        let b = p.build(&root.join(".next"));
        assert!(b.problem.unwrap().contains("run `next build` first"));
    }
}
