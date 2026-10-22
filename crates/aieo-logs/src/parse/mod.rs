//! Log formats: detection from the first lines, and a per-file line parser.

pub mod json;
pub mod text;

use serde_json::Value;

pub use json::Parsed;
use text::CustomFormat;

#[derive(Debug, Clone)]
pub enum Format {
    /// Nginx / Apache `combined`, `common`, `vhost_combined`
    Combined,
    /// `nginx:<log_format>` or `apache:<LogFormat>`
    Custom(CustomFormat, String),
    Caddy,
    Cloudflare,
    Vercel,
    /// CloudFront standard logs, v1 W3C (tab-separated with `#Fields:`)
    CloudFront,
    /// CloudFront standard logging v2, JSON output
    CloudFrontJson,
}

impl Format {
    pub fn name(&self) -> String {
        match self {
            Format::Combined => "combined".into(),
            Format::Custom(_, spec) => spec.clone(),
            Format::Caddy => "caddy".into(),
            Format::Cloudflare => "cloudflare".into(),
            Format::Vercel => "vercel".into(),
            Format::CloudFront => "cloudfront".into(),
            Format::CloudFrontJson => "cloudfront-json".into(),
        }
    }

    /// `--format` value: a format name, or `nginx:…` / `apache:…`.
    pub fn from_arg(s: &str) -> Result<Option<Format>, String> {
        Ok(Some(match s {
            "auto" => return Ok(None),
            "combined" | "nginx" | "apache" => Format::Combined,
            "caddy" => Format::Caddy,
            "cloudflare" => Format::Cloudflare,
            "vercel" => Format::Vercel,
            "cloudfront" => Format::CloudFront,
            "cloudfront-json" => Format::CloudFrontJson,
            custom if custom.starts_with("nginx:") || custom.starts_with("apache:") => {
                Format::Custom(CustomFormat::compile(custom)?, custom.to_string())
            }
            other => return Err(format!("unknown log format {other:?}")),
        }))
    }

    fn of_json(v: &Value) -> Option<Format> {
        if v.get("request").and_then(|r| r.get("uri")).is_some() {
            Some(Format::Caddy)
        } else if v.get("ClientRequestURI").is_some()
            || v.get("ClientRequestPath").is_some()
            || v.get("EdgeStartTimestamp").is_some()
        {
            Some(Format::Cloudflare)
        } else if v.get("proxy").is_some() || v.get("deploymentId").is_some() {
            Some(Format::Vercel)
        } else if v.get("cs-uri-stem").is_some() || v.get("sc-status").is_some() {
            Some(Format::CloudFrontJson)
        } else {
            None
        }
    }

    /// The format of a file from its first lines.
    pub fn detect<'a>(lines: impl IntoIterator<Item = &'a str>) -> Option<Format> {
        let mut vercel_seen = false;
        for line in lines.into_iter().map(str::trim).filter(|l| !l.is_empty()) {
            if line.starts_with("#Version") || line.starts_with("#Fields") {
                return Some(Format::CloudFront);
            }
            if line.starts_with('{') {
                if let Ok(v) = serde_json::from_str::<Value>(line) {
                    match Format::of_json(&v) {
                        // a build or runtime line: keep looking for a request line
                        Some(Format::Vercel) if v.get("proxy").is_none() => vercel_seen = true,
                        Some(f) => return Some(f),
                        None => {}
                    }
                }
                continue;
            }
            if text::parse_combined(line).is_some() {
                return Some(Format::Combined);
            }
        }
        vercel_seen.then_some(Format::Vercel)
    }
}

/// What one line held.
#[derive(Debug)]
pub enum Line {
    Hit(Box<Parsed>),
    /// a header, comment or non-request entry (Vercel build logs, …)
    Ignored,
    /// a line the format cannot read
    Malformed,
}

/// Parses the lines of one file (CloudFront needs its `#Fields` header).
pub struct LineParser {
    format: Format,
    fields: Vec<String>,
}

impl LineParser {
    pub fn new(format: Format) -> LineParser {
        LineParser {
            format,
            fields: Vec::new(),
        }
    }

    pub fn format(&self) -> &Format {
        &self.format
    }

    pub fn parse(&mut self, line: &str) -> Line {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() {
            return Line::Ignored;
        }
        let hit = match &self.format {
            Format::Combined => text::parse_combined(line).map(|h| (h, None)),
            Format::Custom(f, _) => f.parse(line).map(|h| (h, None)),
            Format::CloudFront => {
                if let Some(rest) = line.strip_prefix("#Fields:") {
                    self.fields = rest.split_whitespace().map(String::from).collect();
                    return Line::Ignored;
                }
                if line.starts_with('#') {
                    return Line::Ignored;
                }
                let values: Vec<&str> = line.split('\t').collect();
                if self.fields.is_empty() || values.len() < 2 {
                    return Line::Malformed;
                }
                let fields = &self.fields;
                json::cloudfront(&|k| {
                    fields
                        .iter()
                        .position(|f| f == k)
                        .and_then(|i| values.get(i))
                        .map(|v| v.to_string())
                })
            }
            json_format => {
                let Ok(v) = serde_json::from_str::<Value>(line) else {
                    return Line::Malformed;
                };
                let parsed = match json_format {
                    Format::Caddy => json::caddy(&v),
                    Format::Cloudflare => json::cloudflare(&v),
                    Format::Vercel => {
                        if v.get("proxy").is_none() {
                            return Line::Ignored;
                        }
                        json::vercel(&v)
                    }
                    Format::CloudFrontJson => json::cloudfront_json(&v),
                    _ => unreachable!(),
                };
                if parsed.is_none() && matches!(json_format, Format::Vercel) {
                    return Line::Ignored; // proxy without a final status (revalidation)
                }
                parsed
            }
        };
        match hit {
            Some(p) => Line::Hit(Box::new(p)),
            // a viewer that hung up before a response (status 000)
            None if matches!(self.format, Format::CloudFront) && line.contains("\t000\t") => {
                Line::Ignored
            }
            None if matches!(self.format, Format::CloudFrontJson)
                && line.contains("\"sc-status\":\"000\"") =>
            {
                Line::Ignored
            }
            None => Line::Malformed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formats() {
        let name = |lines: &[&str]| Format::detect(lines.iter().copied()).map(|f| f.name());
        assert_eq!(
            name(&[r#"1.2.3.4 - - [22/Oct/2026:08:15:03 +0000] "GET / HTTP/1.1" 200 1 "-" "x""#]),
            Some("combined".into())
        );
        assert_eq!(
            name(&["#Version: 1.0", "#Fields: date time"]),
            Some("cloudfront".into())
        );
        assert_eq!(
            name(&[r#"{"level":"info","ts":1.0,"request":{"uri":"/"},"status":200}"#]),
            Some("caddy".into())
        );
        assert_eq!(
            name(&[r#"{"ClientIP":"1.2.3.4","ClientRequestURI":"/","EdgeStartTimestamp":1}"#]),
            Some("cloudflare".into())
        );
        assert_eq!(
            name(&[
                r#"{"source":"build","deploymentId":"d"}"#,
                r#"{"deploymentId":"d","proxy":{}}"#
            ]),
            Some("vercel".into())
        );
        assert_eq!(name(&["hello world"]), None);
    }

    #[test]
    fn reads_cloudfront_w3c() {
        let mut p = LineParser::new(Format::CloudFront);
        assert!(matches!(p.parse("#Version: 1.0"), Line::Ignored));
        assert!(matches!(
            p.parse("#Fields: date time x-edge-location sc-bytes c-ip cs-method cs(Host) cs-uri-stem sc-status cs(Referer) cs(User-Agent) cs-uri-query cs(Cookie) x-edge-result-type x-edge-request-id x-host-header"),
            Line::Ignored
        ));
        let row = "2019-12-04\t21:02:31\tLAX1\t392\t192.0.2.100\tGET\td111111abcdef8.cloudfront.net\t/index.html\t200\t-\tMozilla/5.0%20(compatible;%20GPTBot/1.1)\t-\t-\tHit\tSOX4\twww.example.com";
        let Line::Hit(p1) = p.parse(row) else {
            panic!()
        };
        let (h, id) = *p1;
        assert_eq!(h.ua, "Mozilla/5.0 (compatible; GPTBot/1.1)");
        assert_eq!(h.host.as_deref(), Some("www.example.com"));
        assert_eq!(
            (h.cache.as_deref(), id.as_deref()),
            (Some("Hit"), Some("SOX4"))
        );
        assert_eq!(h.ts.unix_timestamp(), 1_575_493_351);
        let hung_up = row.replace("\t200\t", "\t000\t");
        assert!(matches!(p.parse(&hung_up), Line::Ignored));
    }
}
