//! A build directory seen as the static host will serve it.
//!
//! URL paths map to files as Cloudflare Pages / Netlify / most static hosts
//! map them (and as aieo.ee's conformance runner serves fixtures):
//! `index.html` → `/`; `a/index.html` → `/a/` and `/a`; `a.html` → `/a`
//! (winning over `a/index.html`); anything else → its own path. `_headers`
//! and `_redirects` in the build root apply on top.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{bail, Context, Result};
use percent_encoding::percent_decode_str;
use regex::Regex;
use url::Url;

use crate::hosting::{HeaderRules, RedirectRules};

/// Files a static host reads as configuration, never serves.
const HOST_FILES: [&str; 3] = ["_headers", "_redirects", "_routes.json"];

#[derive(Debug, Clone, Default)]
pub struct SiteOptions {
    /// extra response headers per exact URL path (tests; `_headers` covers builds)
    pub header_overrides: HashMap<String, Vec<(String, String)>>,
    /// Astro `base`: URL prefix the build is served under ("" for none)
    pub base_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// a file answers the path
    File(String),
    /// a `_redirects` rule or a meta-refresh page sends the request on
    Redirect {
        to: String,
        status: u16,
        via: String,
    },
    /// nothing answers: the host returns 404
    Missing,
    /// the URL is on another host
    Foreign,
}

/// Outcome of following a URL through redirects inside the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Followed {
    /// the file finally served, if any
    pub file: Option<String>,
    pub hops: Vec<String>,
    /// left the site (final hop on another host)
    pub foreign: bool,
    pub looped: bool,
}

pub struct Site {
    pub root: PathBuf,
    pub origin: Url,
    pub base_path: String,
    files: BTreeSet<String>,
    routes: HashMap<String, String>,
    header_rules: HeaderRules,
    header_overrides: HashMap<String, Vec<(String, String)>>,
    pub redirects: RedirectRules,
    /// meta-refresh redirect pages (Astro `redirects`, many SSGs): file → target
    stubs: HashMap<String, String>,
}

static META_REFRESH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<meta\b[^>]*http-equiv\s*=\s*["']?refresh["']?[^>]*content\s*=\s*["']\s*\d+\s*;\s*url\s*=\s*([^"'>]+)["']"#)
        .unwrap()
});

/// URL paths a static host answers with `rel` (mirror of `routesForFile`
/// in `@aieo/rules/conformance`).
pub fn routes_for_file(rel: &str) -> Vec<String> {
    if rel == "index.html" {
        return vec!["/".into(), "/index.html".into()];
    }
    if let Some(dir) = rel.strip_suffix("/index.html") {
        return vec![format!("/{dir}/"), format!("/{dir}"), format!("/{rel}")];
    }
    if let Some(stem) = rel.strip_suffix(".html") {
        return vec![format!("/{stem}"), format!("/{rel}")];
    }
    vec![format!("/{rel}")]
}

/// The canonical URL path of a page file (`a/index.html` → `/a/`, `a.html` → `/a`).
pub fn page_path(rel: &str) -> String {
    routes_for_file(rel)
        .into_iter()
        .next()
        .unwrap_or_else(|| format!("/{rel}"))
}

impl Site {
    pub fn load(root: &Path, origin: Url, options: SiteOptions) -> Result<Site> {
        if !root.is_dir() {
            bail!("build directory {} does not exist", root.display());
        }
        let mut files = BTreeSet::new();
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .sort_by_file_name()
        {
            let entry = entry.with_context(|| format!("reading {}", root.display()))?;
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(root)
                .expect("walkdir stays under root");
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            if rel.ends_with(".DS_Store") {
                continue;
            }
            files.insert(rel);
        }

        let base_path = options.base_path.trim_end_matches('/').to_string();
        let mut ordered: Vec<&String> = files
            .iter()
            .filter(|f| !HOST_FILES.contains(&f.as_str()))
            .collect();
        // `a.html` claims `/a` before `a/index.html` does
        ordered.sort_by_key(|f| f.ends_with("/index.html"));
        let mut routes = HashMap::new();
        for rel in ordered {
            for route in routes_for_file(rel) {
                let full = if route == "/" && !base_path.is_empty() {
                    routes
                        .entry(base_path.clone())
                        .or_insert_with(|| rel.clone());
                    format!("{base_path}/")
                } else {
                    format!("{base_path}{route}")
                };
                routes.entry(full).or_insert_with(|| rel.clone());
            }
        }

        let read = |name: &str| std::fs::read_to_string(root.join(name)).ok();
        let header_rules = read("_headers")
            .map(|t| HeaderRules::parse(&t))
            .unwrap_or_default();
        let redirects = read("_redirects")
            .map(|t| RedirectRules::parse(&t))
            .unwrap_or_default();

        let mut stubs = HashMap::new();
        for rel in files.iter().filter(|f| f.ends_with(".html")) {
            let path = root.join(rel);
            if std::fs::metadata(&path)
                .map(|m| m.len())
                .unwrap_or(u64::MAX)
                > 4096
            {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Some(c) = META_REFRESH.captures(&text) {
                    stubs.insert(rel.clone(), c[1].trim().to_string());
                }
            }
        }

        Ok(Site {
            root: root.to_path_buf(),
            origin,
            base_path,
            files,
            routes,
            header_rules,
            header_overrides: options.header_overrides,
            redirects,
            stubs,
        })
    }

    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(String::as_str)
    }

    pub fn has_file(&self, rel: &str) -> bool {
        self.files.contains(rel)
    }

    pub fn file_size(&self, rel: &str) -> Option<u64> {
        std::fs::metadata(self.root.join(rel)).ok().map(|m| m.len())
    }

    pub fn read_bytes(&self, rel: &str) -> Option<Vec<u8>> {
        std::fs::read(self.root.join(rel)).ok()
    }

    pub fn host(&self) -> &str {
        self.origin.host_str().unwrap_or_default()
    }

    /// `scheme://host[:port]`
    pub fn origin_str(&self) -> String {
        self.origin.origin().ascii_serialization()
    }

    /// Absolute URL of a path on this site.
    pub fn url_for(&self, path: &str) -> String {
        format!("{}{}", self.origin_str(), path)
    }

    /// The URL path of a page file, base path included.
    pub fn page_url_path(&self, rel: &str) -> String {
        let p = page_path(rel);
        if self.base_path.is_empty() {
            p
        } else {
            format!("{}{}", self.base_path, p)
        }
    }

    pub fn redirect_stub(&self, rel: &str) -> Option<&str> {
        self.stubs.get(rel).map(String::as_str)
    }

    /// Exact route lookup — what aieo.ee's fixture server answers.
    pub fn resolve_exact(&self, path: &str) -> Option<&str> {
        self.routes.get(path).map(String::as_str)
    }

    /// Same host as the site (scheme and `www.` count: http/https and a
    /// different port are the same site; www and apex are not).
    pub fn is_own_url(&self, url: &Url) -> bool {
        url.host_str() == self.origin.host_str()
            && url.port_or_known_default().is_some_and(|p| {
                p == self.origin.port_or_known_default().unwrap_or(p) || matches!(p, 80 | 443)
            })
    }

    /// How the static host answers `path` (percent-decoded first); a
    /// meta-refresh page counts as a redirect.
    pub fn resolve_path(&self, path: &str) -> Resolution {
        self.resolve_path_with(path, true)
    }

    fn resolve_path_with(&self, path: &str, meta_refresh: bool) -> Resolution {
        let decoded = percent_decode_str(path).decode_utf8_lossy().into_owned();
        let file = self
            .routes
            .get(&decoded)
            .or_else(|| self.routes.get(path))
            // `/a/` when only `a.html` exists: hosts redirect to `/a`
            .or_else(|| {
                decoded
                    .strip_suffix('/')
                    .filter(|p| !p.is_empty())
                    .and_then(|p| self.routes.get(p))
            });
        if let Some(t) = self.redirects.lookup(self.host(), &decoded, file.is_some()) {
            if t.status == 200 {
                // a rewrite: the target's content answers under this path
                if let Ok(u) = self.origin.join(&t.location) {
                    if self.is_own_url(&u) {
                        if let Some(f) = self.routes.get(u.path()) {
                            return Resolution::File(f.clone());
                        }
                    }
                }
                return Resolution::Missing;
            }
            if !(400..500).contains(&t.status) {
                return Resolution::Redirect {
                    to: t.location,
                    status: t.status,
                    via: format!("_redirects: {}", t.from),
                };
            }
            return Resolution::Missing;
        }
        match file {
            Some(f) => match self.stubs.get(f).filter(|_| meta_refresh) {
                Some(target) => Resolution::Redirect {
                    to: target.clone(),
                    status: 200,
                    via: format!("meta refresh in {f}"),
                },
                None => Resolution::File(f.clone()),
            },
            None => Resolution::Missing,
        }
    }

    pub fn resolve_url(&self, url: &Url) -> Resolution {
        if !self.is_own_url(url) {
            return Resolution::Foreign;
        }
        self.resolve_path(url.path())
    }

    /// Follow a URL through `_redirects` rules and meta-refresh pages (what
    /// a browser — and a crawler that renders — ends up on).
    pub fn follow(&self, url: &Url) -> Followed {
        self.follow_with(url, true)
    }

    /// Follow HTTP redirects only (what a plain HTTP client ends up on).
    pub fn follow_http(&self, url: &Url) -> Followed {
        self.follow_with(url, false)
    }

    fn follow_with(&self, url: &Url, meta_refresh: bool) -> Followed {
        let mut current = url.clone();
        let mut hops = Vec::new();
        for _ in 0..8 {
            let resolution = if self.is_own_url(&current) {
                self.resolve_path_with(current.path(), meta_refresh)
            } else {
                Resolution::Foreign
            };
            match resolution {
                Resolution::File(f) => {
                    return Followed {
                        file: Some(f),
                        hops,
                        foreign: false,
                        looped: false,
                    }
                }
                Resolution::Missing => {
                    return Followed {
                        file: None,
                        hops,
                        foreign: false,
                        looped: false,
                    }
                }
                Resolution::Foreign => {
                    return Followed {
                        file: None,
                        hops,
                        foreign: true,
                        looped: false,
                    }
                }
                Resolution::Redirect { to, .. } => {
                    let Ok(next) = current.join(&to) else {
                        return Followed {
                            file: None,
                            hops,
                            foreign: false,
                            looped: false,
                        };
                    };
                    if hops.contains(&next.to_string()) || next == *url {
                        return Followed {
                            file: None,
                            hops,
                            foreign: false,
                            looped: true,
                        };
                    }
                    hops.push(next.to_string());
                    current = next;
                }
            }
        }
        Followed {
            file: None,
            hops,
            foreign: false,
            looped: true,
        }
    }

    /// Response headers for a path (names lower-cased; repeated names kept).
    pub fn headers_for(&self, path: &str) -> Vec<(String, String)> {
        let mut out = self.header_rules.for_request(self.host(), path);
        if let Some(extra) = self.header_overrides.get(path) {
            out.extend(
                extra
                    .iter()
                    .map(|(k, v)| (k.to_ascii_lowercase(), v.clone())),
            );
        }
        out
    }

    /// One header as Node joins repeats (`a, b`), or None.
    pub fn header(&self, path: &str, name: &str) -> Option<String> {
        let values: Vec<String> = self
            .headers_for(path)
            .into_iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v)
            .collect();
        (!values.is_empty()).then(|| values.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(files: &[(&str, &str)], base: &str) -> (tempfile::TempDir, Site) {
        let dir = tempfile::tempdir().unwrap();
        for (rel, text) in files {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let s = Site::load(
            dir.path(),
            Url::parse("https://example.com").unwrap(),
            SiteOptions {
                base_path: base.into(),
                ..Default::default()
            },
        )
        .unwrap();
        (dir, s)
    }

    #[test]
    fn maps_paths_like_a_static_host() {
        let (_d, s) = site(
            &[
                ("index.html", "<html>"),
                ("a.html", "<html>"),
                ("a/index.html", "<html>"),
                ("b/index.html", "<html>"),
                ("caf\u{e9}/index.html", "<html>"),
                (
                    "old.html",
                    r#"<meta http-equiv="refresh" content="0;url=/b/">"#,
                ),
                ("_redirects", "/gone /b/ 301\n/proxy/* /b/ 200\n"),
                ("_headers", "/b/*\n  X-Robots-Tag: noindex\n"),
            ],
            "",
        );
        assert_eq!(s.resolve_path("/"), Resolution::File("index.html".into()));
        assert_eq!(s.resolve_path("/a"), Resolution::File("a.html".into()));
        assert_eq!(
            s.resolve_path("/a/"),
            Resolution::File("a/index.html".into())
        );
        assert_eq!(
            s.resolve_path("/b"),
            Resolution::File("b/index.html".into())
        );
        assert_eq!(
            s.resolve_path("/caf%C3%A9/"),
            Resolution::File("caf\u{e9}/index.html".into())
        );
        assert!(matches!(
            s.resolve_path("/gone"),
            Resolution::Redirect { status: 301, .. }
        ));
        assert_eq!(
            s.resolve_path("/proxy/x"),
            Resolution::File("b/index.html".into())
        );
        assert_eq!(s.resolve_path("/nope"), Resolution::Missing);
        assert_eq!(s.resolve_path("/_headers"), Resolution::Missing);
        let followed = s.follow(&Url::parse("https://example.com/old").unwrap());
        assert_eq!(followed.file.as_deref(), Some("b/index.html"));
        assert_eq!(s.header("/b/", "x-robots-tag").as_deref(), Some("noindex"));
        assert_eq!(
            s.resolve_url(&Url::parse("https://other.com/").unwrap()),
            Resolution::Foreign
        );
        assert_eq!(s.page_url_path("b/index.html"), "/b/");
    }

    #[test]
    fn applies_the_base_path() {
        let (_d, s) = site(
            &[("index.html", "<html>"), ("guide/index.html", "<html>")],
            "/docs/",
        );
        assert_eq!(
            s.resolve_path("/docs/"),
            Resolution::File("index.html".into())
        );
        assert_eq!(
            s.resolve_path("/docs"),
            Resolution::File("index.html".into())
        );
        assert_eq!(
            s.resolve_path("/docs/guide/"),
            Resolution::File("guide/index.html".into())
        );
        assert_eq!(s.resolve_path("/guide/"), Resolution::Missing);
        assert_eq!(s.page_url_path("guide/index.html"), "/docs/guide/");
    }
}
