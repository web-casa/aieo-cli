//! Literal values of a JavaScript / TypeScript framework config, read
//! without executing it.
//!
//! The config object is the one passed to `defineConfig(…)` /
//! `defineNuxtConfig(…)`, written after `export default` or
//! `module.exports =`, or bound to a `…config` variable
//! (`const nextConfig: NextConfig = {…}`). Its keys are recorded with
//! dotted paths for nested objects (`app.baseURL`, `sitemap.hostname`).
//! String, boolean and number literals are kept; anything computed is
//! recorded as present with no value, so callers can tell "not set" from
//! "set, but not knowable statically".

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValue {
    /// the literal (`true` / `false` / numbers as written); None = computed
    pub value: Option<String>,
    /// 1-based line of the key
    pub line: usize,
}

/// A framework config file and what could be read from it.
#[derive(Debug, Clone, Default)]
pub struct ConfigFile {
    /// path relative to the project root
    pub file: String,
    pub values: HashMap<String, ConfigValue>,
}

impl ConfigFile {
    /// The value for `key`, or for its lower-case form: Hugo keys are case
    /// insensitive and stored lower-case, while findings name them as
    /// written in the docs (`baseURL`).
    fn value(&self, key: &str) -> Option<&ConfigValue> {
        self.values
            .get(key)
            .or_else(|| self.values.get(&key.to_ascii_lowercase()))
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.value(key)?.value.as_deref()
    }

    pub fn has(&self, key: &str) -> bool {
        self.value(key).is_some()
    }

    pub fn line(&self, key: &str) -> Option<usize> {
        self.value(key).map(|v| v.line)
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Punct(char),
}

/// Tokens of a JS/TS source with their lines: identifiers (numbers
/// included), string literals without interpolation, punctuation.
/// Comments are skipped.
fn tokenize(src: &str) -> Vec<(Tok, usize)> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    let mut line = 1;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                if chars[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
        } else if c == '"' || c == '\'' || c == '`' {
            let quote = c;
            let start_line = line;
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
                if chars[i] == '\n' {
                    line += 1;
                }
                s.push(chars[i]);
                i += 1;
            }
            i += 1;
            toks.push((
                if interpolated {
                    Tok::Punct('?')
                } else {
                    Tok::Str(s)
                },
                start_line,
            ));
        } else if c.is_alphanumeric()
            || c == '_'
            || c == '$'
            || c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)
        {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric()
                    || chars[i] == '_'
                    || chars[i] == '$'
                    || chars[i] == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit))
            {
                i += 1;
            }
            toks.push((Tok::Ident(chars[start..i].iter().collect()), line));
        } else {
            toks.push((Tok::Punct(c), line));
            i += 1;
        }
    }
    toks
}

fn is_literal(t: &Tok) -> Option<String> {
    match t {
        Tok::Str(s) => Some(s.clone()),
        Tok::Ident(w) if w == "true" || w == "false" => Some(w.clone()),
        Tok::Ident(w) if w.chars().next().is_some_and(|c| c.is_ascii_digit()) => Some(w.clone()),
        _ => None,
    }
}

/// Index of the `{` that opens the config object, if any.
fn config_object_start(toks: &[(Tok, usize)]) -> Option<usize> {
    let ident = |i: usize| match toks.get(i) {
        Some((Tok::Ident(s), _)) => Some(s.as_str()),
        _ => None,
    };
    let punct = |i: usize, c: char| matches!(toks.get(i), Some((Tok::Punct(p), _)) if *p == c);
    let mut fallback = None;
    for i in 0..toks.len() {
        if !punct(i, '{') || i == 0 {
            continue;
        }
        // defineConfig({ / defineNuxtConfig({
        if punct(i - 1, '(')
            && i >= 2
            && ident(i - 2).is_some_and(|n| n.starts_with("define") && n.ends_with("Config"))
        {
            return Some(i);
        }
        // export default {
        if i >= 2 && ident(i - 1) == Some("default") && ident(i - 2) == Some("export") {
            return Some(i);
        }
        // module.exports = {
        if i >= 4
            && punct(i - 1, '=')
            && ident(i - 2) == Some("exports")
            && punct(i - 3, '.')
            && ident(i - 4) == Some("module")
        {
            return Some(i);
        }
        // const nextConfig = { / const config: Config = {
        if fallback.is_none() && punct(i - 1, '=') {
            let mut j = i - 2;
            // skip a type annotation `: Type`
            while j > 0 && !matches!(toks.get(j), Some((Tok::Punct(':'), _))) && j + 4 > i {
                j -= 1;
            }
            let name_at = if punct(j, ':') {
                j.checked_sub(1)
            } else {
                Some(i - 2)
            };
            if let Some(name) = name_at.and_then(ident) {
                if name.to_ascii_lowercase().ends_with("config")
                    && name_at
                        .and_then(|n| n.checked_sub(1))
                        .and_then(ident)
                        .is_some_and(|k| matches!(k, "const" | "let" | "var"))
                {
                    fallback = Some(i);
                }
            }
        }
    }
    fallback
}

/// Keys of the config object, dotted for nested objects.
pub fn config_values(src: &str) -> HashMap<String, ConfigValue> {
    let toks = tokenize(src);
    let mut out = HashMap::new();
    let Some(start) = config_object_start(&toks) else {
        return out;
    };
    // one entry per open bracket: Some(path) for an object we read keys of
    let mut stack: Vec<Option<String>> = vec![Some(String::new())];
    let mut pending_key: Option<String> = None;
    let mut i = start + 1;
    while i < toks.len() && !stack.is_empty() {
        let (tok, line) = &toks[i];
        let in_object = stack.last().is_some_and(Option::is_some);
        match tok {
            Tok::Punct('{') => {
                let path = pending_key.take().and_then(|k| {
                    let parent = stack.last()?.as_ref()?;
                    Some(if parent.is_empty() {
                        k
                    } else {
                        format!("{parent}.{k}")
                    })
                });
                stack.push(path);
            }
            Tok::Punct('[') | Tok::Punct('(') => {
                pending_key = None;
                stack.push(None);
            }
            Tok::Punct('}') | Tok::Punct(']') | Tok::Punct(')') => {
                pending_key = None;
                stack.pop();
            }
            Tok::Ident(k) | Tok::Str(k) if in_object => {
                let parent = stack.last().cloned().flatten().unwrap_or_default();
                let full = |k: &str| {
                    if parent.is_empty() {
                        k.to_string()
                    } else {
                        format!("{parent}.{k}")
                    }
                };
                let prev_opens = matches!(toks.get(i - 1), Some((Tok::Punct('{' | ','), _)))
                    || matches!(toks.get(i - 1), Some((Tok::Ident(w), _)) if w == "async");
                match toks.get(i + 1) {
                    Some((Tok::Punct(':'), _)) if prev_opens => {
                        let value = toks.get(i + 2).map(|t| &t.0);
                        let ends = matches!(
                            toks.get(i + 3).map(|t| &t.0),
                            Some(Tok::Punct(',' | '}')) | None
                        );
                        match value {
                            Some(Tok::Punct('{')) => {
                                out.entry(full(k)).or_insert(ConfigValue {
                                    value: None,
                                    line: *line,
                                });
                                pending_key = Some(k.clone());
                                i += 2;
                                continue;
                            }
                            Some(v) => {
                                let literal = if ends { is_literal(v) } else { None };
                                out.entry(full(k)).or_insert(ConfigValue {
                                    value: literal,
                                    line: *line,
                                });
                            }
                            None => {}
                        }
                        i += 2;
                        continue;
                    }
                    // method shorthand: `async redirects() {…}`
                    Some((Tok::Punct('('), _)) if prev_opens && matches!(tok, Tok::Ident(_)) => {
                        out.entry(full(k)).or_insert(ConfigValue {
                            value: None,
                            line: *line,
                        });
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Read a config file under `root` (first of `names` that exists).
pub fn read_config(root: &std::path::Path, names: &[&str]) -> Option<ConfigFile> {
    names.iter().find_map(|name| {
        let text = std::fs::read_to_string(root.join(name)).ok()?;
        Some(ConfigFile {
            file: name.to_string(),
            values: config_values(&text),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(src: &str, key: &str) -> Option<String> {
        config_values(src).get(key).and_then(|v| v.value.clone())
    }

    #[test]
    fn reads_astro_config() {
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
        let c = config_values(src);
        assert_eq!(v(src, "site").as_deref(), Some("https://ubuntu.fan"));
        assert_eq!(c["site"].line, 6);
        assert_eq!(v(src, "base").as_deref(), Some("/docs"));
        assert_eq!(v(src, "trailingSlash").as_deref(), Some("always"));
        assert!(c.contains_key("outDir"), "present");
        assert_eq!(c["outDir"].value, None, "computed values stay unknown");
        assert!(
            !c.contains_key("title"),
            "values inside calls are not config keys"
        );
        assert_eq!(
            v(
                "export default defineConfig({ site: `https://${host}` })",
                "site"
            ),
            None
        );
    }

    #[test]
    fn reads_next_config_variable_and_methods() {
        let src = r#"
import type { NextConfig } from "next";
const nextConfig: NextConfig = {
  output: "export",
  trailingSlash: true,
  basePath: '/app',
  images: { unoptimized: true },
  async redirects() {
    return [{ source: "/old", destination: "/new", permanent: true }];
  },
};
export default nextConfig;
"#;
        let c = config_values(src);
        assert_eq!(v(src, "output").as_deref(), Some("export"));
        assert_eq!(v(src, "trailingSlash").as_deref(), Some("true"));
        assert_eq!(v(src, "images.unoptimized").as_deref(), Some("true"));
        assert_eq!(c["redirects"].line, 8);
        assert!(
            !c.contains_key("source"),
            "values inside methods are not config keys"
        );
        let cjs =
            "/** @type */\nmodule.exports = {\n  basePath: '/b',\n  headers: async () => [],\n}";
        assert_eq!(v(cjs, "basePath").as_deref(), Some("/b"));
        assert!(config_values(cjs).contains_key("headers"));
    }

    #[test]
    fn reads_nested_keys() {
        let src = r#"export default defineNuxtConfig({
  app: { baseURL: '/blog/', head: { htmlAttrs: { lang: 'en' } } },
  ssr: false,
  routeRules: { '/old-page': { redirect: '/about' }, '/guide/**': { prerender: true } },
  sitemap: { hostname: "https://example.com" },
  port: 3000,
})"#;
        assert_eq!(v(src, "app.baseURL").as_deref(), Some("/blog/"));
        assert_eq!(v(src, "app.head.htmlAttrs.lang").as_deref(), Some("en"));
        assert_eq!(v(src, "ssr").as_deref(), Some("false"));
        assert_eq!(
            v(src, "routeRules./old-page.redirect").as_deref(),
            Some("/about")
        );
        assert_eq!(
            v(src, "routeRules./guide/**.prerender").as_deref(),
            Some("true")
        );
        assert_eq!(
            v(src, "sitemap.hostname").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(v(src, "port").as_deref(), Some("3000"));
    }

    #[test]
    fn reads_docusaurus_config() {
        let src = r#"import type {Config} from '@docusaurus/types';
const config: Config = {
  title: 'Example',
  url: 'https://example.com',
  baseUrl: '/',
  trailingSlash: false,
  presets: [['classic', { docs: { routeBasePath: 'docs' } }]],
};
export default config;"#;
        assert_eq!(v(src, "url").as_deref(), Some("https://example.com"));
        assert_eq!(v(src, "trailingSlash").as_deref(), Some("false"));
        assert!(!config_values(src).contains_key("docs.routeBasePath"));
    }
}
