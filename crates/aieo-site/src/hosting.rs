//! Static-host rule files shipped in a build: `_headers` and `_redirects`
//! (Cloudflare Pages / Netlify format). Patterns: `*` (splat, any run of
//! characters) and `:name` (one path segment); a rule may name an absolute
//! URL, which then only applies on that host.

use url::Url;

#[derive(Debug, Clone)]
struct Pattern {
    /// None = any host (path-only rule); Some = host pattern (may hold placeholders)
    host: Option<String>,
    path: String,
}

impl Pattern {
    fn parse(raw: &str) -> Option<Pattern> {
        if raw.starts_with('/') {
            return Some(Pattern {
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
        Some(Pattern {
            host: Some(host.to_ascii_lowercase()),
            path: path.to_string(),
        })
    }

    fn matches(&self, host: &str, path: &str) -> Option<Captures> {
        if let Some(h) = &self.host {
            glob_match(h, &host.to_ascii_lowercase(), '.')?;
        }
        glob_match(&self.path, path, '/')
    }
}

#[derive(Debug, Default, Clone)]
pub struct Captures {
    pub splat: Option<String>,
    pub named: Vec<(String, String)>,
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

#[derive(Debug, Clone)]
struct HeaderRule {
    pattern: Pattern,
    set: Vec<(String, String)>,
    remove: Vec<String>,
}

/// Parsed `_headers`.
#[derive(Debug, Clone, Default)]
pub struct HeaderRules {
    rules: Vec<HeaderRule>,
}

impl HeaderRules {
    pub fn parse(text: &str) -> HeaderRules {
        let mut rules: Vec<HeaderRule> = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let indented = line.starts_with(' ') || line.starts_with('\t');
            if !indented {
                match Pattern::parse(trimmed) {
                    Some(pattern) => rules.push(HeaderRule {
                        pattern,
                        set: Vec::new(),
                        remove: Vec::new(),
                    }),
                    None => rules.push(HeaderRule {
                        // unparseable selector: keep a rule that never matches
                        pattern: Pattern {
                            host: Some("\u{0}".into()),
                            path: String::new(),
                        },
                        set: Vec::new(),
                        remove: Vec::new(),
                    }),
                }
                continue;
            }
            let Some(rule) = rules.last_mut() else {
                continue;
            };
            if let Some(name) = trimmed.strip_prefix('!') {
                rule.remove.push(name.trim().to_ascii_lowercase());
            } else if let Some((name, value)) = trimmed.split_once(':') {
                rule.set
                    .push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
            }
        }
        HeaderRules { rules }
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
    pub force: bool,
}

/// Where a `_redirects` rule sends a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectTarget {
    pub location: String,
    pub status: u16,
    pub from: String,
}

/// Parsed `_redirects`.
#[derive(Debug, Clone, Default)]
pub struct RedirectRules {
    rules: Vec<Redirect>,
}

impl RedirectRules {
    pub fn parse(text: &str) -> RedirectRules {
        let mut rules = Vec::new();
        for line in text.lines() {
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
            let Some(pattern) = Pattern::parse(parts[0]) else {
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
            });
        }
        RedirectRules { rules }
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// First rule matching the request. `file_exists`: whether a file
    /// already answers the path — unforced rules only apply when none does
    /// (shadowing, as Netlify and Cloudflare Pages behave).
    pub fn lookup(&self, host: &str, path: &str, file_exists: bool) -> Option<RedirectTarget> {
        for rule in &self.rules {
            if file_exists && !rule.force && rule.status != 200 {
                continue;
            }
            let Some(caps) = rule.pattern.matches(host, path) else {
                continue;
            };
            let mut location = rule.to.clone();
            if let Some(splat) = &caps.splat {
                location = location.replace(":splat", splat);
            }
            for (name, value) in &caps.named {
                location = location.replace(&format!(":{name}"), value);
            }
            return Some(RedirectTarget {
                location,
                status: rule.status,
                from: rule.from.clone(),
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
            if start.from.contains('*') || start.from.contains(':') || start.status == 200 {
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
    }

    #[test]
    fn finds_redirect_loops() {
        let base = Url::parse("https://e.com/").unwrap();
        let r = RedirectRules::parse("/a /b\n/b /a\n/c /d\n");
        let loops = r.loops(&base);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0], vec!["/a", "/b", "/a"]);
    }
}
