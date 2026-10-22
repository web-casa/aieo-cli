//! Framework presets: where the build lands, which origin, base path and
//! trailing-slash policy the framework config declares, what the
//! deployment adds (redirects, headers, server-rendered routes) and which
//! source file a page came from.
//!
//! Framework configs are JavaScript; they are never executed. Only literal
//! values are read (`site: "https://…"`); anything computed stays unknown
//! and must come from `--site` / `aieo.toml`.

mod astro;
mod docs;
mod hugo;
pub mod jsconfig;
mod next;
mod nuxt;
pub mod routes;
mod spa;

use std::path::{Path, PathBuf};

use url::Url;

use crate::hosting::HostConfig;
use crate::site::SiteOptions;
pub use jsconfig::{config_values, ConfigFile, ConfigValue};
pub use routes::{route_key, SourceIndex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetKind {
    Static,
    Astro,
    Starlight,
    Next,
    Nuxt,
    Vue,
    Tanstack,
    Hugo,
    Vitepress,
    Docusaurus,
}

pub const PRESETS: [PresetKind; 10] = [
    PresetKind::Static,
    PresetKind::Astro,
    PresetKind::Starlight,
    PresetKind::Next,
    PresetKind::Nuxt,
    PresetKind::Vue,
    PresetKind::Tanstack,
    PresetKind::Hugo,
    PresetKind::Vitepress,
    PresetKind::Docusaurus,
];

impl PresetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PresetKind::Static => "static",
            PresetKind::Astro => "astro",
            PresetKind::Starlight => "starlight",
            PresetKind::Next => "next",
            PresetKind::Nuxt => "nuxt",
            PresetKind::Vue => "vue",
            PresetKind::Tanstack => "tanstack",
            PresetKind::Hugo => "hugo",
            PresetKind::Vitepress => "vitepress",
            PresetKind::Docusaurus => "docusaurus",
        }
    }

    pub fn parse(s: &str) -> Option<PresetKind> {
        PRESETS.into_iter().find(|k| k.as_str() == s)
    }
}

/// How the framework's trailing-slash setting shapes page URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrailingSlash {
    /// `/guide/`
    Always,
    /// `/guide`
    Never,
}

/// Who renders the pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rendering {
    /// every page is a file in the build
    Static,
    /// some pages are files, a server renders the rest on request
    Hybrid,
    /// one HTML shell; the browser renders every route
    Spa,
}

impl Rendering {
    pub fn as_str(self) -> &'static str {
        match self {
            Rendering::Static => "static",
            Rendering::Hybrid => "hybrid",
            Rendering::Spa => "spa",
        }
    }
}

/// A config key the build output does not apply, with why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoredSetting {
    pub key: String,
    pub line: usize,
    pub why: String,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub kind: PresetKind,
    /// default build directory for the preset (relative to `root`)
    pub default_dist: String,
    /// the framework config, when one was found
    pub config: Option<ConfigFile>,
    /// production origin and the config key it came from
    pub site: Option<String>,
    pub site_key: Option<String>,
    /// where the origin is declared when that is not the framework config
    /// (Next.js `metadataBase` in `app/layout.tsx`): file and line
    pub site_at: Option<(String, usize)>,
    /// URL prefix the site is served under (`/docs`), never `/`
    pub base: Option<String>,
    pub base_key: Option<String>,
    pub trailing_slash: Option<TrailingSlash>,
    pub trailing_slash_key: Option<String>,
    /// how the framework renders when nothing in the build says otherwise
    pub rendering: Rendering,
    /// config the build does not apply (Next.js `redirects()` with `output: 'export'`)
    pub ignored: Vec<IgnoredSetting>,
    pub notes: Vec<String>,
    sources: SourceIndex,
    /// VitePress: the source directory (relative to the root)
    sources_root: Option<String>,
}

/// What a preset adds to a build directory to serve it like production.
#[derive(Debug, Default)]
pub struct Build {
    pub options: SiteOptions,
    pub rendering: Option<Rendering>,
    pub notes: Vec<String>,
    /// set by presets that add server routes, for the coverage note
    server_label: Option<&'static str>,
    /// the directory is not a build that can be checked (why, and what to run)
    pub problem: Option<String>,
}

pub(crate) fn package_json(root: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(root.join("package.json")).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn has_dependency(root: &Path, names: &[&str]) -> bool {
    let Some(json) = package_json(root) else {
        return false;
    };
    ["dependencies", "devDependencies"].iter().any(|k| {
        json.get(k)
            .is_some_and(|d| names.iter().any(|n| d.get(n).is_some()))
    })
}

pub(crate) fn first_file(root: &Path, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find(|n| root.join(n).is_file())
        .map(|n| n.to_string())
}

/// `./dist/` → `dist`
pub(crate) fn clean_dir(dir: &str) -> String {
    dir.trim_start_matches("./")
        .trim_end_matches('/')
        .to_string()
}

/// `/docs/` → Some("/docs"); `/` or empty → None.
pub(crate) fn clean_base(base: &str) -> Option<String> {
    let b = base.trim().trim_end_matches('/');
    if b.is_empty() || b == "." {
        return None;
    }
    Some(if b.starts_with('/') {
        b.to_string()
    } else {
        format!("/{b}")
    })
}

fn detect_kind(root: &Path) -> PresetKind {
    if astro::detect(root) {
        if has_dependency(root, &["@astrojs/starlight"]) {
            PresetKind::Starlight
        } else {
            PresetKind::Astro
        }
    } else if next::detect(root) {
        PresetKind::Next
    } else if nuxt::detect(root) {
        PresetKind::Nuxt
    } else if docs::docusaurus_detect(root) {
        PresetKind::Docusaurus
    } else if docs::vitepress_detect(root) {
        PresetKind::Vitepress
    } else if spa::tanstack_detect(root) {
        PresetKind::Tanstack
    } else if hugo::detect(root) {
        PresetKind::Hugo
    } else if spa::vue_detect(root) {
        PresetKind::Vue
    } else {
        PresetKind::Static
    }
}

/// Detect the framework of the project at `root` (or use `forced`).
pub fn detect(root: &Path, forced: Option<PresetKind>) -> Project {
    let kind = forced.unwrap_or_else(|| detect_kind(root));
    let mut project = Project {
        root: root.to_path_buf(),
        kind,
        default_dist: "dist".into(),
        config: None,
        site: None,
        site_key: None,
        site_at: None,
        base: None,
        base_key: None,
        trailing_slash: None,
        trailing_slash_key: None,
        rendering: Rendering::Static,
        ignored: Vec::new(),
        notes: Vec::new(),
        sources: SourceIndex::default(),
        sources_root: None,
    };
    match kind {
        PresetKind::Static => {}
        PresetKind::Astro | PresetKind::Starlight => astro::configure(&mut project),
        PresetKind::Next => next::configure(&mut project),
        PresetKind::Nuxt => nuxt::configure(&mut project),
        PresetKind::Vue => spa::vue_configure(&mut project),
        PresetKind::Tanstack => spa::tanstack_configure(&mut project),
        PresetKind::Hugo => hugo::configure(&mut project),
        PresetKind::Vitepress => docs::vitepress_configure(&mut project),
        PresetKind::Docusaurus => docs::docusaurus_configure(&mut project),
    }
    project
}

impl Project {
    /// The source file (relative to the project root) a page URL path was built from.
    pub fn source_for(&self, url_path: &str) -> Option<String> {
        let path = match self.base.as_deref() {
            Some(base) => url_path.strip_prefix(base).unwrap_or(url_path),
            None => url_path,
        };
        self.sources.lookup(&route_key(path))
    }

    /// Read a config literal and remember which key it came from.
    pub(crate) fn take_site(&mut self, key: &str) {
        if self.site.is_some() {
            return;
        }
        if let Some(v) = self.config.as_ref().and_then(|c| c.get(key)) {
            if Url::parse(v).is_ok_and(|u| u.host_str().is_some()) {
                self.site = Some(v.to_string());
                self.site_key = Some(key.to_string());
            }
        }
    }

    pub(crate) fn take_base(&mut self, key: &str) {
        if self.base_key.is_some() {
            return;
        }
        if let Some(v) = self.config.as_ref().and_then(|c| c.get(key)) {
            self.base = clean_base(v);
            self.base_key = Some(key.to_string());
        }
    }

    pub(crate) fn set_trailing_slash(&mut self, policy: TrailingSlash, key: &str) {
        self.trailing_slash = Some(policy);
        self.trailing_slash_key = Some(key.to_string());
    }

    /// Directory (relative to the root) whose files the framework copies
    /// to the build unchanged — where a file like an IndexNow key goes.
    /// None for a plain build directory (no source to put it in).
    pub fn public_dir(&self) -> Option<String> {
        Some(match self.kind {
            PresetKind::Static => return None,
            PresetKind::Hugo | PresetKind::Docusaurus => "static".into(),
            PresetKind::Vitepress => {
                // `<vitepress root>/<srcDir>/public`
                let src = self.sources_root.clone().unwrap_or_default();
                if src.is_empty() {
                    "public".into()
                } else {
                    format!("{src}/public")
                }
            }
            _ => "public".into(),
        })
    }

    /// Where a config key was written: `astro.config.mjs:4`.
    pub fn config_ref(&self, key: &str) -> Option<(String, usize)> {
        let c = self.config.as_ref()?;
        Some((c.file.clone(), c.line(key)?))
    }

    /// Site options for serving `dist` as production would: deployment
    /// config (`vercel.json`, `netlify.toml`), the framework's own routing
    /// (Next.js manifests, server routes) and the base path.
    pub fn build(&self, dist: &Path) -> Build {
        let mut build = Build::default();
        build.options.base_path = self.base.clone().unwrap_or_default();
        let mut host = HostConfig::default();
        if let Ok(text) = std::fs::read_to_string(self.root.join("vercel.json")) {
            match HostConfig::vercel(&text, "vercel.json") {
                Some(c) => host.extend(c),
                None => build
                    .notes
                    .push("vercel.json is not valid JSON; its rules were not applied".into()),
            }
        }
        if let Ok(text) = std::fs::read_to_string(self.root.join("netlify.toml")) {
            match HostConfig::netlify(&text, "netlify.toml") {
                Some(c) => host.extend(c),
                None => build
                    .notes
                    .push("netlify.toml could not be parsed; its rules were not applied".into()),
            }
        }
        match self.kind {
            PresetKind::Next => next::build(self, dist, &mut build, &mut host),
            PresetKind::Nuxt => nuxt::build(self, dist, &mut build, &mut host),
            PresetKind::Tanstack => spa::tanstack_build(self, dist, &mut build),
            _ => {}
        }
        if host.skipped > 0 {
            build.notes.push(format!(
                "{} redirect/header rule(s) with request conditions (has/missing, Country=…) were not applied",
                host.skipped
            ));
        }
        build.options.host = host;
        if let Some(label) = build.server_label {
            build.notes.push(server_note(label, dist, &build.options));
        }
        build
    }
}

/// How many server routes have no HTML in the build (render only on request).
fn server_note(label: &str, dist: &Path, options: &SiteOptions) -> String {
    let files: Vec<String> = match &options.layout {
        Some(layout) => layout.iter().map(|(rel, _)| rel.clone()).collect(),
        None => crate::site::walk_files(dist).unwrap_or_default(),
    };
    let html: std::collections::HashSet<String> = files
        .into_iter()
        .filter(|f| f.ends_with(".html"))
        .map(|f| route_key(&format!("/{f}")))
        .collect();
    let routes = &options.server_routes;
    let dynamic = routes.iter().filter(|r| r.route.contains('[')).count();
    let unrendered = routes
        .iter()
        .filter(|r| !r.route.contains('[') && !html.contains(&route_key(&r.route)))
        .count();
    if dynamic + unrendered == 0 {
        format!("{label}: every route is prerendered")
    } else {
        format!(
            "{label}: {unrendered} route(s) and {dynamic} dynamic route(s) render only on request; links to them count as working, their HTML is not checked"
        )
    }
}

/// Production origin the build itself declares: the homepage canonical, a
/// robots.txt `Sitemap:` line, or the first sitemap entry.
/// `layout`: the build's published files when they are not `dist` as is
/// (see [`Build`]).
pub fn infer_origin(
    dist: &Path,
    layout: Option<&[(String, PathBuf)]>,
) -> Option<(Url, &'static str)> {
    let read = |name: &str| match layout {
        Some(files) => files
            .iter()
            .find(|(rel, _)| rel == name)
            .and_then(|(_, path)| std::fs::read_to_string(path).ok()),
        None => std::fs::read_to_string(dist.join(name)).ok(),
    };
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
pub(crate) mod testing {
    use std::path::Path;

    pub fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}
