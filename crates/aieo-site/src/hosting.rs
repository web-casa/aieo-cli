//! Host rules that decide what a request gets besides the files: redirects,
//! rewrites and response headers. Three sources are read:
//!
//! - `_headers` / `_redirects` in the build (Cloudflare Pages / Netlify
//!   format): `*` (splat) and `:name` (one segment); a rule may name an
//!   absolute URL, which then only applies on that host. Unforced redirects
//!   are shadowed by an existing file.
//! - `vercel.json` (`redirects`, `rewrites`, `headers`, `trailingSlash`,
//!   `cleanUrls`) and Next.js's `routes-manifest.json`: path-to-regexp
//!   sources (`:name`, `:name*`, `:name+`, `:name?`, `(regex)`). Redirects
//!   run before the filesystem there; rewrites after it unless Next puts
//!   them `beforeFiles`.
//!
//! Rules with `has` / `missing` conditions (or `_redirects` conditions such
//! as `Country=`) depend on the request and are left out.

use regex::Regex;
use serde_json::Value;
use url::Url;

/// Where a rule was written, for findings that point at configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSource {
    /// file relative to the project or build root
    pub file: String,
    /// 1-based line when known
    pub line: Option<usize>,
}

#[derive(Debug, Clone)]
enum Pattern {
    Glob {
        /// None = any host (path-only rule); Some = host pattern (may hold placeholders)
        host: Option<String>,
        path: String,
    },
    /// path-to-regexp source compiled to a regex; group names by index
    /// (None = unnamed group, referenced as `$n`)
    Route {
        re: Regex,
        names: Vec<Option<String>>,
    },
    Never,
}

impl Pattern {
    fn glob(raw: &str) -> Option<Pattern> {
        if raw.starts_with('/') {
            return Some(Pattern::Glob {
                host: None,
                path: raw.to_string(),
            });
        }
        // absolute: scheme://host/path — the host may hold `:placeholder`s,
        // which `Url` would reject, so split by hand
        let rest = raw
            .strip_prefix("https://")
            .or_else(|| raw.strip_prefix("http://"))?;
        let (host, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        Some(Pattern::Glob {
            host: Some(host.to_ascii_lowercase()),
            path: path.to_string(),
        })
    }

    fn route(source: &str) -> Option<Pattern> {
        let (re, names) = route_regex(source, false)?;
        Some(Pattern::Route { re, names })
    }

    fn matches(&self, host: &str, path: &str) -> Option<Captures> {
        match self {
            Pattern::Glob { host: h, path: p } => {
                if let Some(h) = h {
                    glob_match(h, &host.to_ascii_lowercase(), '.')?;
                }
                glob_match(p, path, '/')
            }
            Pattern::Route { re, names } => {
                let c = re.captures(path)?;
                let mut caps = Captures::default();
                for (i, name) in names.iter().enumerate() {
                    let value = c.get(i + 1).map(|m| m.as_str().to_string());
                    match name {
                        Some(n) => caps.named.push((n.clone(), value.unwrap_or_default())),
                        None => caps.positional.push(value.unwrap_or_default()),
                    }
                }
                Some(caps)
            }
            Pattern::Never => None,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Captures {
    pub splat: Option<String>,
    pub named: Vec<(String, String)>,
    /// unnamed path-to-regexp groups, `$1`…
    pub positional: Vec<String>,
}

/// Match `pattern` against `text`: `*` = any run, `:name` = one segment
/// (no `sep`). Returns captures on a full match.
fn glob_match(pattern: &str, text: &str, sep: char) -> Option<Captures> {
    fn go(p: &[char], t: &[char], sep: char, caps: &mut Captures) -> bool {
        if p.is_empty() {
            return t.is_empty();
        }
        match p[0] {
            '*' => {
                for k in (0..=t.len()).rev() {
                    let before = caps.clone();
                    if go(&p[1..], &t[k..], sep, caps) {
                        caps.splat.get_or_insert_with(|| t[..k].iter().collect());
                        return true;
                    }
                    *caps = before;
                }
                false
            }
            ':' if p.len() > 1 && (p[1].is_ascii_alphanumeric() || p[1] == '_') => {
                let name_len = p[1..]
                    .iter()
                    .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
                    .count();
                let name: String = p[1..1 + name_len].iter().collect();
                let seg_max = t.iter().take_while(|c| **c != sep && **c != '/').count();
                for k in (1..=seg_max).rev() {
                    let before = caps.clone();
                    if go(&p[1 + name_len..], &t[k..], sep, caps) {
                        caps.named.push((name, t[..k].iter().collect()));
                        return true;
                    }
                    *caps = before;
                }
                false
            }
            c => !t.is_empty() && t[0] == c && go(&p[1..], &t[1..], sep, caps),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let mut caps = Captures::default();
    go(&p, &t, sep, &mut caps).then_some(caps)
}

/// Compile a path-to-regexp source (the dialect Next.js and Vercel use) to
/// an anchored regex. Unless `strict`, a trailing slash is optional, as
/// with path-to-regexp's default mode.
pub fn route_regex(source: &str, strict: bool) -> Option<(Regex, Vec<Option<String>>)> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::from("^");
    let mut names: Vec<Option<String>> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let (name, group) = if c == ':'
            && chars
                .get(i + 1)
                .is_some_and(|n| n.is_ascii_alphabetic() || *n == '_')
        {
            let start = i + 1;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let name: String = chars[start..i].iter().collect();
            let group = if chars.get(i) == Some(&'(') {
                let (g, next) = take_group(&chars, i)?;
                i = next;
                g
            } else {
                "[^/]+?".to_string()
            };
            (Some(name), group)
        } else if c == '(' {
            let (g, next) = take_group(&chars, i)?;
            i = next;
            (None, g)
        } else if c == '*' {
            i += 1;
            (None, ".*".to_string())
        } else {
            if c == '\\' && i + 1 < chars.len() {
                i += 1;
            }
            out.push_str(&regex::escape(&chars[i].to_string()));
            i += 1;
            continue;
        };
        let modifier = chars
            .get(i)
            .copied()
            .filter(|m| matches!(m, '*' | '+' | '?'));
        if modifier.is_some() {
            i += 1;
        }
        let slash_before = out.ends_with('/');
        match modifier {
            Some('*') if slash_before => {
                out.pop();
                out.push_str(&format!("(?:/((?:{group})(?:/(?:{group}))*))?"));
            }
            Some('+') => out.push_str(&format!("((?:{group})(?:/(?:{group}))*)")),
            Some('?') if slash_before => {
                out.pop();
                out.push_str(&format!("(?:/({group}))?"));
            }
            Some('*') => out.push_str(&format!("((?:{group})(?:/(?:{group}))*)?")),
            Some(_) => out.push_str(&format!("({group})?")),
            None => out.push_str(&format!("({group})")),
        }
        names.push(name);
    }
    if !strict && !source.ends_with('/') {
        out.push_str("(?:/)?");
    }
    out.push('$');
    Some((Regex::new(&out).ok()?, names))
}

/// `(…)` starting at `start` (balanced, escapes honoured) → inner regex, next index.
fn take_group(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut depth = 0;
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let inner: String = chars[start + 1..i].iter().collect();
                    // a leading `?:` / lookaround stays as written; make it non-capturing
                    let inner = inner.strip_prefix("?:").unwrap_or(&inner).to_string();
                    return Some((inner, i + 1));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Fill `:name` / `:name*` / `$n` / `:splat` in a destination.
fn substitute(dest: &str, caps: &Captures) -> String {
    let mut location = dest.to_string();
    if let Some(splat) = &caps.splat {
        location = location.replace(":splat", splat);
    }
    let mut named = caps.named.clone();
    // longer names first so `:slug` does not eat `:slugs`
    named.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
    for (name, value) in &named {
        for suffix in ["*", "+", "?", ""] {
            location = location.replace(&format!(":{name}{suffix}"), value);
        }
    }
    for (i, value) in caps.positional.iter().enumerate().rev() {
        location = location.replace(&format!("${}", i + 1), value);
    }
    location
}

#[derive(Debug, Clone)]
struct HeaderRule {
    pattern: Pattern,
    set: Vec<(String, String)>,
    remove: Vec<String>,
    source: RuleSource,
}

/// A header rule as configured (for findings about the configuration).
#[derive(Debug, Clone)]
pub struct HeaderRuleInfo<'a> {
    pub pattern: &'a str,
    pub set: &'a [(String, String)],
    pub source: &'a RuleSource,
}

/// Response-header rules from `_headers`, `vercel.json` or Next.js.
#[derive(Debug, Clone, Default)]
pub struct HeaderRules {
    rules: Vec<HeaderRule>,
    patterns: Vec<String>,
}

impl HeaderRules {
    pub fn parse(text: &str) -> HeaderRules {
        let mut out = HeaderRules::default();
        for (n, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let indented = line.starts_with(' ') || line.starts_with('\t');
            if !indented {
                // an unparseable selector keeps a rule that never matches
                let pattern = Pattern::glob(trimmed).unwrap_or(Pattern::Never);
                out.push(
                    pattern,
                    trimmed,
                    Vec::new(),
                    RuleSource {
                        file: "_headers".into(),
                        line: Some(n + 1),
                    },
                );
                continue;
            }
            let Some(rule) = out.rules.last_mut() else {
                continue;
            };
            if let Some(name) = trimmed.strip_prefix('!') {
                rule.remove.push(name.trim().to_ascii_lowercase());
            } else if let Some((name, value)) = trimmed.split_once(':') {
                rule.set
                    .push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
            }
        }
        out
    }

    fn push(
        &mut self,
        pattern: Pattern,
        raw: &str,
        set: Vec<(String, String)>,
        source: RuleSource,
    ) {
        self.rules.push(HeaderRule {
            pattern,
            set,
            remove: Vec::new(),
            source,
        });
        self.patterns.push(raw.to_string());
    }

    /// `[{source, headers: [{key, value}], has?, missing?}]` (vercel.json, Next.js).
    fn from_route_list(list: Option<&Value>, file: &str, text: &str) -> (HeaderRules, usize) {
        let mut out = HeaderRules::default();
        let mut skipped = 0;
        for item in list.and_then(Value::as_array).into_iter().flatten() {
            if conditional(item) {
                skipped += 1;
                continue;
            }
            let Some(source) = item.get("source").and_then(Value::as_str) else {
                continue;
            };
            let set = item
                .get("headers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|h| {
                    Some((
                        h.get("key")?.as_str()?.to_ascii_lowercase(),
                        h.get("value")?.as_str()?.to_string(),
                    ))
                })
                .collect();
            let pattern = Pattern::route(source).unwrap_or(Pattern::Never);
            out.push(pattern, source, set, json_source(file, text, source));
        }
        (out, skipped)
    }

    pub fn extend(&mut self, other: HeaderRules) {
        self.rules.extend(other.rules);
        self.patterns.extend(other.patterns);
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The configured rules, in order.
    pub fn rules(&self) -> impl Iterator<Item = HeaderRuleInfo<'_>> {
        self.rules
            .iter()
            .zip(&self.patterns)
            .map(|(r, p)| HeaderRuleInfo {
                pattern: p,
                set: &r.set,
                source: &r.source,
            })
    }

    /// Whether rule `index` (as `rules()` yields them) applies to a request.
    pub fn rule_matches(&self, index: usize, host: &str, path: &str) -> bool {
        self.rules
            .get(index)
            .is_some_and(|r| r.pattern.matches(host, path).is_some())
    }

    /// Headers for a request, in rule order; repeated names are kept apart
    /// (callers join them as an HTTP stack would).
    pub fn for_request(&self, host: &str, path: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for rule in &self.rules {
            if rule.pattern.matches(host, path).is_none() {
                continue;
            }
            for name in &rule.remove {
                out.retain(|(n, _)| n != name);
            }
            out.extend(rule.set.iter().cloned());
        }
        out
    }
}

#[derive(Debug, Clone)]
pub struct Redirect {
    pattern: Pattern,
    pub from: String,
    pub to: String,
    pub status: u16,
    /// applies even when a file answers the path
    pub force: bool,
    /// a single literal path (no placeholders, splats or groups)
    pub fixed: bool,
    /// a framework's own redirect (Next.js trailing-slash handling), not the user's
    pub internal: bool,
    pub source: RuleSource,
}

/// Where a redirect rule sends a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectTarget {
    pub location: String,
    pub status: u16,
    pub from: String,
    pub source: RuleSource,
}

/// Redirect and rewrite rules, in the order the host applies them.
#[derive(Debug, Clone, Default)]
pub struct RedirectRules {
    rules: Vec<Redirect>,
}

fn conditional(item: &Value) -> bool {
    item.get("has").is_some() || item.get("missing").is_some()
}

/// Locate `"needle"` in a JSON config for a finding's line.
fn json_source(file: &str, text: &str, needle: &str) -> RuleSource {
    let quoted = format!("\"{needle}\"");
    RuleSource {
        file: file.to_string(),
        line: text
            .find(&quoted)
            .map(|at| text[..at].matches('\n').count() + 1),
    }
}

impl RedirectRules {
    pub fn parse(text: &str) -> RedirectRules {
        let mut rules = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            // conditions (Country=, Language=, Role=, query params) are not modelled
            if parts.iter().skip(2).any(|p| p.contains('=')) || parts[0].contains('=') {
                continue;
            }
            let Some(pattern) = Pattern::glob(parts[0]) else {
                continue;
            };
            let (status, force) = match parts.get(2) {
                Some(code) => {
                    let force = code.ends_with('!');
                    (code.trim_end_matches('!').parse().unwrap_or(301), force)
                }
                None => (301, false),
            };
            rules.push(Redirect {
                pattern,
                from: parts[0].to_string(),
                to: parts[1].to_string(),
                status,
                force,
                fixed: !parts[0].contains(['*', ':']) && parts[0].starts_with('/'),
                internal: false,
                source: RuleSource {
                    file: "_redirects".into(),
                    line: Some(n + 1),
                },
            });
        }
        RedirectRules { rules }
    }

    /// `[{source, destination, permanent?, statusCode?}]` → rules.
    fn route_list(
        list: Option<&Value>,
        file: &str,
        text: &str,
        rewrite: bool,
        force: bool,
        skipped: &mut usize,
    ) -> Vec<Redirect> {
        let mut rules = Vec::new();
        for item in list.and_then(Value::as_array).into_iter().flatten() {
            if conditional(item) {
                *skipped += 1;
                continue;
            }
            let (Some(source), Some(dest)) = (
                item.get("source").and_then(Value::as_str),
                item.get("destination").and_then(Value::as_str),
            ) else {
                continue;
            };
            let Some(pattern) = Pattern::route(source) else {
                *skipped += 1;
                continue;
            };
            let status = if rewrite {
                200
            } else if let Some(code) = item.get("statusCode").and_then(Value::as_u64) {
                code as u16
            } else if item.get("permanent").and_then(Value::as_bool) == Some(false) {
                307
            } else {
                308
            };
            let internal = item.get("internal").and_then(Value::as_bool) == Some(true);
            rules.push(Redirect {
                pattern,
                from: source.to_string(),
                to: dest.to_string(),
                status,
                force,
                fixed: !source.contains([':', '(', '*']),
                internal,
                source: if internal {
                    RuleSource {
                        file: file.to_string(),
                        line: None,
                    }
                } else {
                    json_source(file, text, source)
                },
            });
        }
        rules
    }

    pub fn extend(&mut self, other: RedirectRules) {
        self.rules.extend(other.rules);
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn rules(&self) -> &[Redirect] {
        &self.rules
    }

    /// First rule matching the request. `file_exists`: whether a file
    /// already answers the path — unforced rules only apply when none does
    /// (shadowing, as Netlify and Cloudflare Pages behave).
    pub fn lookup(&self, host: &str, path: &str, file_exists: bool) -> Option<RedirectTarget> {
        for rule in &self.rules {
            // an existing file wins over unforced rules, rewrites included
            // (Netlify / Cloudflare Pages shadowing; Vercel and Next.js
            // rewrites run after the filesystem)
            if file_exists && !rule.force {
                continue;
            }
            let Some(caps) = rule.pattern.matches(host, path) else {
                continue;
            };
            return Some(RedirectTarget {
                location: substitute(&rule.to, &caps),
                status: rule.status,
                from: rule.from.clone(),
                source: rule.source.clone(),
            });
        }
        None
    }

    /// Redirect sources that point at themselves through other rules
    /// (A → B → A), each reported once.
    pub fn loops(&self, base: &Url) -> Vec<Vec<String>> {
        let host = base.host_str().unwrap_or_default();
        let mut found: Vec<Vec<String>> = Vec::new();
        for start in &self.rules {
            if !start.fixed || start.status == 200 {
                continue;
            }
            let mut chain = vec![start.from.clone()];
            let mut path = start.from.clone();
            for _ in 0..10 {
                let Some(t) = self.lookup(host, &path, false) else {
                    break;
                };
                if t.status == 200 {
                    break;
                }
                let next = match base.join(&t.location) {
                    Ok(u) if u.host_str() == Some(host) => u.path().to_string(),
                    _ => break,
                };
                if chain.contains(&next) {
                    chain.push(next);
                    if !found
                        .iter()
                        .any(|f| f.first() == chain.first() || f.contains(&chain[0]))
                    {
                        found.push(chain.clone());
                    }
                    break;
                }
                chain.push(next.clone());
                path = next;
            }
        }
        found
    }
}

/// Rules a deployment config adds on top of the build.
#[derive(Debug, Clone, Default)]
pub struct HostConfig {
    pub redirects: RedirectRules,
    pub headers: HeaderRules,
    /// rules left out because they depend on request conditions
    pub skipped: usize,
}

impl HostConfig {
    pub fn extend(&mut self, other: HostConfig) {
        self.redirects.extend(other.redirects);
        self.headers.extend(other.headers);
        self.skipped += other.skipped;
    }

    /// `vercel.json` (`file` is how findings name it).
    pub fn vercel(text: &str, file: &str) -> Option<HostConfig> {
        let json: Value = serde_json::from_str(text).ok()?;
        let mut skipped = 0;
        let mut rules = Vec::new();
        // Vercel answers `cleanUrls` and `trailingSlash` before user redirects
        if json.get("cleanUrls").and_then(Value::as_bool) == Some(true) {
            for (from, to) in [
                ("/:path*/index.html", "/:path*"),
                ("/:path+.html", "/:path+"),
            ] {
                rules.push(builtin(from, to, file));
            }
        }
        match json.get("trailingSlash").and_then(Value::as_bool) {
            Some(false) => rules.push(builtin("/:path+/", "/:path+", file)),
            Some(true) => rules.push(builtin(r"/:path((?:[^/]+/)*[^/.]+)", "/:path/", file)),
            None => {}
        }
        rules.extend(RedirectRules::route_list(
            json.get("redirects"),
            file,
            text,
            false,
            true,
            &mut skipped,
        ));
        rules.extend(RedirectRules::route_list(
            json.get("rewrites"),
            file,
            text,
            true,
            false,
            &mut skipped,
        ));
        let (headers, header_skipped) =
            HeaderRules::from_route_list(json.get("headers"), file, text);
        Some(HostConfig {
            redirects: RedirectRules { rules },
            headers,
            skipped: skipped + header_skipped,
        })
    }

    /// Redirects a framework config declares: (path-to-regexp source,
    /// destination, status, where written). They run before any file.
    pub fn redirect_list(list: Vec<(String, String, u16, RuleSource)>) -> HostConfig {
        let mut out = HostConfig::default();
        for (from, to, status, source) in list {
            let Some(pattern) = Pattern::route(&from) else {
                out.skipped += 1;
                continue;
            };
            out.redirects.rules.push(Redirect {
                pattern,
                fixed: !from.contains([':', '(', '*']),
                from,
                to,
                status,
                force: true,
                internal: false,
                source,
            });
        }
        out
    }

    /// `netlify.toml` `[[redirects]]` / `[[headers]]` (same patterns as `_redirects`).
    pub fn netlify(text: &str, file: &str) -> Option<HostConfig> {
        let doc: toml::Table = toml::from_str(text).ok()?;
        let mut out = HostConfig::default();
        let line_of = |needle: &str| json_source(file, text, needle);
        for item in doc
            .get("redirects")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            if item.get("conditions").is_some() || item.get("query").is_some() {
                out.skipped += 1;
                continue;
            }
            let (Some(from), Some(to)) = (
                item.get("from").and_then(toml::Value::as_str),
                item.get("to").and_then(toml::Value::as_str),
            ) else {
                continue;
            };
            let Some(pattern) = Pattern::glob(from) else {
                continue;
            };
            out.redirects.rules.push(Redirect {
                pattern,
                from: from.to_string(),
                to: to.to_string(),
                status: item
                    .get("status")
                    .and_then(toml::Value::as_integer)
                    .map_or(301, |s| s as u16),
                force: item.get("force").and_then(toml::Value::as_bool) == Some(true),
                fixed: !from.contains(['*', ':']) && from.starts_with('/'),
                internal: false,
                source: line_of(from),
            });
        }
        for item in doc
            .get("headers")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(path) = item.get("for").and_then(toml::Value::as_str) else {
                continue;
            };
            let set = item
                .get("values")
                .and_then(toml::Value::as_table)
                .into_iter()
                .flatten()
                .filter_map(|(k, v)| Some((k.to_ascii_lowercase(), v.as_str()?.to_string())))
                .collect();
            let pattern = Pattern::glob(path).unwrap_or(Pattern::Never);
            out.headers.push(pattern, path, set, line_of(path));
        }
        Some(out)
    }

    /// Next.js `.next/routes-manifest.json`: redirects (with Next's own
    /// trailing-slash rule marked internal), headers and rewrites.
    pub fn next_manifest(text: &str, file: &str) -> Option<HostConfig> {
        let json: Value = serde_json::from_str(text).ok()?;
        let mut skipped = 0;
        let mut rules =
            RedirectRules::route_list(json.get("redirects"), file, text, false, true, &mut skipped);
        let rewrites = json.get("rewrites");
        if let Some(list) = rewrites.filter(|r| r.is_array()) {
            rules.extend(RedirectRules::route_list(
                Some(list),
                file,
                text,
                true,
                false,
                &mut skipped,
            ));
        } else if let Some(obj) = rewrites {
            for (key, force) in [
                ("beforeFiles", true),
                ("afterFiles", false),
                ("fallback", false),
            ] {
                rules.extend(RedirectRules::route_list(
                    obj.get(key),
                    file,
                    text,
                    true,
                    force,
                    &mut skipped,
                ));
            }
        }
        let (headers, header_skipped) =
            HeaderRules::from_route_list(json.get("headers"), file, text);
        Some(HostConfig {
            redirects: RedirectRules { rules },
            headers,
            skipped: skipped + header_skipped,
        })
    }
}

fn builtin(from: &str, to: &str, file: &str) -> Redirect {
    let pattern = route_regex(from, true)
        .map(|(re, names)| Pattern::Route { re, names })
        .unwrap_or(Pattern::Never);
    Redirect {
        pattern,
        from: from.to_string(),
        to: to.to_string(),
        status: 308,
        force: true,
        fixed: false,
        internal: true,
        source: RuleSource {
            file: file.to_string(),
            line: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_match_paths_hosts_and_detach() {
        let h = HeaderRules::parse(
            "# comment\n/*\n  X-Frame-Options: DENY\n/docs/*\n  X-Robots-Tag: noindex\n  ! X-Frame-Options\nhttps://:project.pages.dev/*\n  X-Robots-Tag: noindex\n",
        );
        assert_eq!(
            h.for_request("example.com", "/"),
            vec![("x-frame-options".into(), "DENY".into())]
        );
        assert_eq!(
            h.for_request("example.com", "/docs/a/"),
            vec![("x-robots-tag".into(), "noindex".into())]
        );
        // the preview-domain rule never applies to production
        assert_eq!(
            h.for_request("my-site.pages.dev", "/x").last().unwrap().1,
            "noindex"
        );
        assert_eq!(h.for_request("example.com", "/x").len(), 1);
        let infos: Vec<_> = h.rules().collect();
        assert_eq!(infos[1].pattern, "/docs/*");
        assert_eq!(infos[1].source.line, Some(4));
    }

    #[test]
    fn redirects_splats_placeholders_and_shadowing() {
        let r = RedirectRules::parse("/old/* /new/:splat 301\n/blog/:year/:slug /posts/:slug\n/forced /x 302!\n/spa/* /index.html 200\n/geo /x 302 Country=us\n");
        assert_eq!(r.len(), 4);
        assert_eq!(
            r.lookup("e.com", "/old/a/b", false).unwrap().location,
            "/new/a/b"
        );
        assert_eq!(
            r.lookup("e.com", "/blog/2024/hello", false)
                .unwrap()
                .location,
            "/posts/hello"
        );
        assert!(r.lookup("e.com", "/blog/2024/hello", true).is_none());
        assert_eq!(r.lookup("e.com", "/forced", true).unwrap().status, 302);
        assert_eq!(r.lookup("e.com", "/spa/route", false).unwrap().status, 200);
        assert!(
            r.lookup("e.com", "/spa/route", true).is_none(),
            "files shadow rewrites"
        );
        assert_eq!(r.rules()[2].source.line, Some(3));
    }

    #[test]
    fn finds_redirect_loops() {
        let base = Url::parse("https://e.com/").unwrap();
        let r = RedirectRules::parse("/a /b\n/b /a\n/c /d\n");
        let loops = r.loops(&base);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0], vec!["/a", "/b", "/a"]);
    }

    #[test]
    fn compiles_path_to_regexp_sources() {
        let m = |src: &str, path: &str| {
            Pattern::route(src)
                .unwrap()
                .matches("e.com", path)
                .map(|c| (c.named, c.positional))
        };
        assert!(m("/old-page", "/old-page").is_some());
        assert!(
            m("/old-page", "/old-page/").is_some(),
            "optional trailing slash"
        );
        assert!(m("/old-page", "/old-page/x").is_none());
        let (named, _) = m("/blog/:slug", "/blog/hello").unwrap();
        assert_eq!(named, vec![("slug".into(), "hello".into())]);
        assert!(m("/blog/:slug", "/blog/a/b").is_none());
        let (named, _) = m("/docs/:path*", "/docs").unwrap();
        assert_eq!(named[0].1, "");
        let (named, _) = m("/docs/:path*", "/docs/a/b").unwrap();
        assert_eq!(named[0].1, "a/b");
        assert!(m("/docs/:path+", "/docs").is_none());
        let (named, _) = m(r"/post/:id(\d+)", "/post/42").unwrap();
        assert_eq!(named[0].1, "42");
        assert!(m(r"/post/:id(\d+)", "/post/x").is_none());
        let (_, pos) = m("/legacy/(.*)", "/legacy/a/b").unwrap();
        assert_eq!(pos, vec!["a/b".to_string()]);
    }

    #[test]
    fn reads_vercel_json() {
        let text = r#"{
  "cleanUrls": true,
  "trailingSlash": false,
  "redirects": [
    { "source": "/old/:slug", "destination": "/new/:slug", "permanent": true },
    { "source": "/tmp", "destination": "/x", "permanent": false },
    { "source": "/geo", "destination": "/x", "has": [{ "type": "header", "key": "x" }] }
  ],
  "rewrites": [{ "source": "/app/(.*)", "destination": "/index.html" }],
  "headers": [{ "source": "/(.*)", "headers": [{ "key": "X-Robots-Tag", "value": "noindex" }] }]
}"#;
        let c = HostConfig::vercel(text, "vercel.json").unwrap();
        assert_eq!(c.skipped, 1);
        let r = &c.redirects;
        let t = r.lookup("e.com", "/old/a", true).unwrap();
        assert_eq!((t.location.as_str(), t.status), ("/new/a", 308));
        assert_eq!(t.source.line, Some(5));
        assert_eq!(r.lookup("e.com", "/tmp", false).unwrap().status, 307);
        assert_eq!(
            r.lookup("e.com", "/about.html", true).unwrap().location,
            "/about"
        );
        assert_eq!(
            r.lookup("e.com", "/guide/", true).unwrap().location,
            "/guide"
        );
        let slash = HostConfig::vercel(r#"{"trailingSlash": true}"#, "vercel.json").unwrap();
        let r2 = &slash.redirects;
        assert_eq!(
            r2.lookup("e.com", "/guide", true).unwrap().location,
            "/guide/"
        );
        assert!(r2.lookup("e.com", "/guide/", true).is_none());
        assert!(r2.lookup("e.com", "/logo.png", true).is_none());
        assert!(
            r.lookup("e.com", "/app/x", true).is_none(),
            "rewrites yield to files"
        );
        assert_eq!(r.lookup("e.com", "/app/x", false).unwrap().status, 200);
        assert_eq!(
            c.headers.for_request("e.com", "/anything"),
            vec![("x-robots-tag".into(), "noindex".into())]
        );
    }

    #[test]
    fn reads_netlify_toml() {
        let text = "[[redirects]]\nfrom = \"/old\"\nto = \"/new\"\nstatus = 302\n\n[[redirects]]\nfrom = \"/geo\"\nto = \"/x\"\nconditions = {Country = [\"US\"]}\n\n[[headers]]\nfor = \"/drafts/*\"\n[headers.values]\nX-Robots-Tag = \"noindex\"\n";
        let c = HostConfig::netlify(text, "netlify.toml").unwrap();
        assert_eq!(c.skipped, 1);
        let t = c.redirects.lookup("e.com", "/old", false).unwrap();
        assert_eq!((t.status, t.source.line), (302, Some(2)));
        assert_eq!(
            c.headers.for_request("e.com", "/drafts/a"),
            vec![("x-robots-tag".into(), "noindex".into())]
        );
    }

    #[test]
    fn reads_next_routes_manifest() {
        let text = r#"{"version":3,"pages404":true,"basePath":"",
 "redirects":[{"source":"/:path+/","destination":"/:path+","internal":true,"statusCode":308,"regex":"^(?:/((?:[^/]+?)(?:/(?:[^/]+?))*))/$"},
  {"source":"/old-page","destination":"/about","statusCode":308,"regex":"^(?!/_next)/old-page(?:/)?$"}],
 "headers":[],
 "rewrites":{"beforeFiles":[],"afterFiles":[{"source":"/api-docs/:p*","destination":"/docs/:p*"}],"fallback":[]}}"#;
        let c = HostConfig::next_manifest(text, ".next/routes-manifest.json").unwrap();
        let r = &c.redirects;
        assert!(r.rules()[0].internal);
        assert_eq!(
            r.lookup("e.com", "/about/", true).unwrap().location,
            "/about"
        );
        assert!(r.lookup("e.com", "/", true).is_none());
        assert_eq!(
            r.lookup("e.com", "/old-page", false).unwrap().location,
            "/about"
        );
        assert_eq!(
            r.lookup("e.com", "/api-docs/a/b", false).unwrap().location,
            "/docs/a/b"
        );
    }
}
