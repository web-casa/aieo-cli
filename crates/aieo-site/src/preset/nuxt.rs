//! Nuxt. `nuxt generate` prerenders into `.output/public`; `nuxt build`
//! leaves a Nitro server next to it (`.output/server`), which renders every
//! page route the build did not prerender. `ssr: false` is a client-rendered
//! SPA. Config: `app.baseURL`, `site.url` (nuxt-site-config), `routeRules`
//! redirects. Sources: `app/pages/**` (Nuxt 4) or `pages/**`.

use std::path::Path;

use super::jsconfig::read_config;
use super::routes::walk_sources;
use super::{first_file, has_dependency, Build, Project, Rendering, TrailingSlash};
use crate::hosting::{HostConfig, RuleSource};

const CONFIGS: [&str; 3] = ["nuxt.config.ts", "nuxt.config.js", "nuxt.config.mjs"];

pub(super) fn detect(root: &Path) -> bool {
    first_file(root, &CONFIGS).is_some() || has_dependency(root, &["nuxt", "nuxt3"])
}

/// `[slug]` stays, `[...slug]` stays, `[[slug]]` (optional) and mixed
/// segments (`post-[id]`) become catch-all / one-segment params.
fn segment(seg: &str) -> String {
    if seg.starts_with("[[") {
        format!("[...{}]", seg.trim_matches(['[', ']']))
    } else if seg.contains('[') && !seg.starts_with('[') {
        "[param]".into()
    } else {
        seg.to_string()
    }
}

pub(super) fn configure(p: &mut Project) {
    p.config = read_config(&p.root, &CONFIGS);
    p.take_site("site.url");
    p.take_site("runtimeConfig.public.siteUrl");
    p.take_base("app.baseURL");
    match p.config.as_ref().and_then(|c| c.bool("site.trailingSlash")) {
        Some(true) => p.set_trailing_slash(TrailingSlash::Always, "site.trailingSlash"),
        Some(false) => p.set_trailing_slash(TrailingSlash::Never, "site.trailingSlash"),
        None => {}
    }
    p.default_dist = ".output/public".into();
    p.rendering = if p.config.as_ref().and_then(|c| c.bool("ssr")) == Some(false) {
        Rendering::Spa
    } else {
        Rendering::Static
    };
    let dir = if p.root.join("app/pages").is_dir() {
        "app/pages"
    } else {
        "pages"
    };
    for (file, mut segments) in walk_sources(&p.root, dir, &["vue", "tsx", "jsx"]) {
        if segments.last().is_some_and(|s| s == "index") {
            segments.pop();
        }
        let segments: Vec<String> = segments.iter().map(|s| segment(s)).collect();
        p.sources.add(&segments, &file);
    }
}

/// `routeRules: { '/old': { redirect: '/new' } }` (or `{ redirect: { to, statusCode } }`).
fn route_rule_redirects(p: &Project) -> HostConfig {
    let Some(config) = &p.config else {
        return HostConfig::default();
    };
    let mut keys: Vec<(&String, &super::ConfigValue)> = config
        .values
        .iter()
        .filter(|(k, _)| k.starts_with("routeRules."))
        .collect();
    keys.sort_by_key(|(_, v)| v.line);
    let mut list = Vec::new();
    for (key, value) in keys {
        let rest = &key["routeRules.".len()..];
        let (pattern, to) = if let Some(pat) = rest.strip_suffix(".redirect.to") {
            (pat, value.value.clone())
        } else if let Some(pat) = rest.strip_suffix(".redirect") {
            (pat, value.value.clone())
        } else {
            continue;
        };
        let Some(to) = to else { continue };
        let status = config
            .get(&format!("routeRules.{pattern}.redirect.statusCode"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(307);
        // radix3 wildcards → path-to-regexp
        let source = pattern.replace("/**", "/:path*").replace("/*", "/:segment");
        list.push((
            source,
            to.replace("/**", "/:path*"),
            status,
            RuleSource {
                file: config.file.clone(),
                line: Some(value.line),
            },
        ));
    }
    HostConfig::redirect_list(list)
}

pub(super) fn build(p: &Project, dist: &Path, build: &mut Build, host: &mut HostConfig) {
    let output = dist.parent().unwrap_or(dist);
    let server = output.join("server").is_dir() && dist.ends_with("public");
    if server {
        // Nitro answers routeRules redirects; a static build has meta-refresh pages instead
        host.extend(route_rule_redirects(p));
    }
    if server && p.rendering != Rendering::Spa {
        build.rendering = Some(Rendering::Hybrid);
        build.options.server_routes = p.sources.server_routes();
        build.server_label = Some("Nuxt server build");
    } else {
        build.rendering = Some(p.rendering);
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::write;
    use super::super::*;

    #[test]
    fn nuxt_config_sources_and_server_routes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "package.json", r#"{"dependencies":{"nuxt":"4.1.0"}}"#);
        write(
            root,
            "nuxt.config.ts",
            "export default defineNuxtConfig({\n  site: { url: 'https://example.com' },\n  routeRules: {\n    '/old-page': { redirect: '/about' },\n    '/about': { prerender: true },\n  },\n})\n",
        );
        write(root, "app/pages/index.vue", "");
        write(root, "app/pages/about.vue", "");
        write(root, "app/pages/blog/[slug].vue", "");
        write(root, "app/pages/docs/[[path]].vue", "");
        write(root, ".output/public/about/index.html", "<html></html>");
        write(root, ".output/public/index.html", "<html></html>");
        write(root, ".output/server/index.mjs", "");
        let p = detect(root, None);
        assert_eq!(p.kind, PresetKind::Nuxt);
        assert_eq!(p.site.as_deref(), Some("https://example.com"));
        assert_eq!(p.default_dist, ".output/public");
        assert_eq!(
            p.source_for("/blog/x").as_deref(),
            Some("app/pages/blog/[slug].vue")
        );
        assert_eq!(
            p.source_for("/docs/a/b").as_deref(),
            Some("app/pages/docs/[[path]].vue")
        );
        let dist = root.join(".output/public");
        let b = p.build(&dist);
        assert_eq!(b.rendering, Some(Rendering::Hybrid));
        let site = crate::Site::load(
            &dist,
            url::Url::parse("https://example.com").unwrap(),
            b.options,
        )
        .unwrap();
        use crate::Resolution;
        assert_eq!(
            site.resolve_path("/about"),
            Resolution::File("about/index.html".into())
        );
        assert_eq!(
            site.resolve_path("/blog/x"),
            Resolution::Server("/blog/[slug]".into())
        );
        assert_eq!(site.resolve_path("/nope"), Resolution::Missing);
        match site.resolve_path("/old-page") {
            Resolution::Redirect { to, status, via } => {
                assert_eq!((to.as_str(), status), ("/about", 307));
                assert_eq!(via, "nuxt.config.ts:4: /old-page");
            }
            other => panic!("{other:?}"),
        }
    }
}
