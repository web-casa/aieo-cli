//! Hugo. Config: `hugo.{toml,yaml,json}` (or the older `config.*`, or
//! `config/_default/`): `baseURL` (origin and base path), `publishDir`,
//! `uglyURLs`, `defaultContentLanguage[InSubdir]`, `contentDir`.
//! Sources: `content/**` — `_index.md` is a section, `index.md` a leaf
//! bundle, `about.zh.md` a translation; front-matter `url` / `slug` win.
//! Aliases are meta-refresh pages in the build, already read as redirects.

use std::collections::HashMap;
use std::path::Path;

use url::Url;

use super::jsconfig::{ConfigFile, ConfigValue};
use super::routes::{front_matter, front_matter_value, walk_sources};
use super::{clean_dir, Project, TrailingSlash};

const CONFIGS: [&str; 10] = [
    "hugo.toml",
    "hugo.yaml",
    "hugo.yml",
    "hugo.json",
    "config.toml",
    "config.yaml",
    "config.yml",
    "config.json",
    "config/_default/hugo.toml",
    "config/_default/config.toml",
];

/// Top-level scalar settings, keys lower-cased (Hugo's are case-insensitive).
fn read(root: &Path) -> Option<ConfigFile> {
    for name in CONFIGS {
        let Ok(text) = std::fs::read_to_string(root.join(name)) else {
            continue;
        };
        let line_of = |key: &str| {
            text.lines()
                .position(|l| {
                    let l = l.trim_start().trim_start_matches('"').to_ascii_lowercase();
                    l.starts_with(key)
                        && l[key.len()..]
                            .trim_start_matches('"')
                            .trim_start()
                            .starts_with([':', '='])
                })
                .map_or(1, |i| i + 1)
        };
        let mut values = HashMap::new();
        let mut put = |key: String, value: Option<String>| {
            let line = line_of(&key);
            values.insert(key, ConfigValue { value, line });
        };
        if name.ends_with(".toml") {
            let Ok(table) = toml::from_str::<toml::Table>(&text) else {
                continue;
            };
            for (k, v) in table {
                let value = match v {
                    toml::Value::String(s) => Some(s),
                    toml::Value::Boolean(b) => Some(b.to_string()),
                    toml::Value::Integer(i) => Some(i.to_string()),
                    _ => None,
                };
                put(k.to_ascii_lowercase(), value);
            }
        } else if name.ends_with(".json") {
            let Ok(serde_json::Value::Object(m)) = serde_json::from_str(&text) else {
                continue;
            };
            for (k, v) in m {
                let value = match v {
                    serde_json::Value::String(s) => Some(s),
                    serde_json::Value::Bool(b) => Some(b.to_string()),
                    serde_json::Value::Number(n) => Some(n.to_string()),
                    _ => None,
                };
                put(k.to_ascii_lowercase(), value);
            }
        } else {
            // YAML: top-level `key: scalar` lines are enough here
            for l in text.lines() {
                if l.starts_with([' ', '\t', '#', '-']) {
                    continue;
                }
                let Some((k, v)) = l.split_once(':') else {
                    continue;
                };
                let v = v
                    .split(" #")
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches(['"', '\'']);
                put(
                    k.trim().to_ascii_lowercase(),
                    (!v.is_empty()).then(|| v.to_string()),
                );
            }
        }
        return Some(ConfigFile {
            file: name.to_string(),
            values,
        });
    }
    None
}

pub(super) fn detect(root: &Path) -> bool {
    let Some(config) = read(root) else {
        return false;
    };
    config.file.starts_with("hugo.")
        || (config.has("baseurl")
            && ["content", "layouts", "themes", "archetypes"]
                .iter()
                .any(|d| root.join(d).is_dir()))
}

pub(super) fn configure(p: &mut Project) {
    p.config = read(&p.root);
    let Some(config) = p.config.clone() else {
        return;
    };
    if let Some(base) = config.get("baseurl").and_then(|b| Url::parse(b).ok()) {
        if base.host_str().is_some() {
            p.site = Some(base.origin().ascii_serialization());
            p.site_key = Some("baseURL".into());
            p.base = super::clean_base(base.path());
            p.base_key = Some("baseURL".into());
        }
    }
    p.default_dist = config
        .get("publishdir")
        .map(clean_dir)
        .unwrap_or_else(|| "public".into());
    if config.bool("uglyurls") == Some(true) {
        p.set_trailing_slash(TrailingSlash::Never, "uglyURLs");
    } else {
        p.set_trailing_slash(TrailingSlash::Always, "uglyURLs (default false)");
    }
    let default_lang = config
        .get("defaultcontentlanguage")
        .unwrap_or("en")
        .to_lowercase();
    let lang_in_subdir = config.bool("defaultcontentlanguageinsubdir") == Some(true);
    let content = config.get("contentdir").unwrap_or("content").to_string();
    for (file, mut segments) in walk_sources(
        &p.root,
        &content,
        &["md", "markdown", "html", "adoc", "org"],
    ) {
        let Some(last) = segments.pop() else { continue };
        let (stem, lang) = match last.rsplit_once('.') {
            Some((stem, lang)) if (2..=7).contains(&lang.len()) && !lang.contains(' ') => {
                (stem.to_string(), Some(lang.to_lowercase()))
            }
            _ => (last.clone(), None),
        };
        let front = front_matter(&p.root.join(&file)).unwrap_or_default();
        let mut path: Vec<String> = if let Some(url) = front_matter_value(&front, "url") {
            url.split('/')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        } else {
            let mut path = segments.clone();
            if stem != "_index" && stem != "index" {
                path.push(stem.clone());
            }
            if let Some(slug) = front_matter_value(&front, "slug") {
                path.pop();
                path.push(slug.trim_matches('/').to_string());
            }
            path
        };
        let lang = lang.unwrap_or_else(|| default_lang.clone());
        if lang != default_lang || lang_in_subdir {
            path.insert(0, lang);
        }
        let path: Vec<String> = path.iter().map(|s| s.replace(' ', "-")).collect();
        p.sources.add(&path, &file);
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::write;
    use super::super::*;

    #[test]
    fn hugo_config_and_content() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "hugo.toml",
            "baseURL = 'https://example.com/docs/'\ntitle = 'x'\npublishDir = 'out'\n",
        );
        write(root, "content/_index.md", "+++\ntitle = 'home'\n+++\n");
        write(root, "content/about.md", "---\ntitle: about\n---\n");
        write(root, "content/about.zh.md", "---\ntitle: about\n---\n");
        write(root, "content/guide/_index.md", "");
        write(root, "content/guide/Getting Started.md", "");
        write(root, "content/blog/hello-world/index.md", "");
        write(root, "content/blog/second.md", "---\nslug: renamed\n---\n");
        write(root, "content/legal.md", "---\nurl: /terms/\n---\n");
        let p = detect(root, None);
        assert_eq!(p.kind, PresetKind::Hugo);
        assert_eq!(p.site.as_deref(), Some("https://example.com"));
        assert_eq!(p.base.as_deref(), Some("/docs"));
        assert_eq!(p.default_dist, "out");
        assert_eq!(p.trailing_slash, Some(TrailingSlash::Always));
        assert_eq!(p.config_ref("baseurl"), Some(("hugo.toml".into(), 1)));
        let s = |u: &str| p.source_for(u);
        assert_eq!(s("/docs/").as_deref(), Some("content/_index.md"));
        assert_eq!(s("/docs/about/").as_deref(), Some("content/about.md"));
        assert_eq!(s("/docs/zh/about/").as_deref(), Some("content/about.zh.md"));
        assert_eq!(
            s("/docs/guide/").as_deref(),
            Some("content/guide/_index.md")
        );
        assert_eq!(
            s("/docs/guide/getting-started/").as_deref(),
            Some("content/guide/Getting Started.md")
        );
        assert_eq!(
            s("/docs/blog/hello-world/").as_deref(),
            Some("content/blog/hello-world/index.md")
        );
        assert_eq!(
            s("/docs/blog/renamed/").as_deref(),
            Some("content/blog/second.md")
        );
        assert_eq!(s("/docs/terms/").as_deref(), Some("content/legal.md"));
    }
}
