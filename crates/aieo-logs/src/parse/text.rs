//! Line formats: Nginx / Apache `combined` (and the vhost variants), and
//! any custom Nginx `log_format` or Apache `LogFormat` string, compiled to
//! a regex whose groups map to hit fields.

use std::sync::LazyLock;

use regex::Regex;
use time::{Month, OffsetDateTime, PrimitiveDateTime, UtcOffset};

use crate::hit::{present, split_target, Hit};

/// `10/Oct/2000:13:55:36 -0700` (Nginx `$time_local`, Apache `%t`).
pub fn parse_time_local(s: &str) -> Option<OffsetDateTime> {
    let s = s.trim().trim_start_matches('[').trim_end_matches(']');
    let (dt, zone) = s.split_once(' ')?;
    let mut parts = dt.splitn(4, [':', '/']);
    let day: u8 = parts.next()?.parse().ok()?;
    let month = match parts.next()? {
        "Jan" => Month::January,
        "Feb" => Month::February,
        "Mar" => Month::March,
        "Apr" => Month::April,
        "May" => Month::May,
        "Jun" => Month::June,
        "Jul" => Month::July,
        "Aug" => Month::August,
        "Sep" => Month::September,
        "Oct" => Month::October,
        "Nov" => Month::November,
        "Dec" => Month::December,
        _ => return None,
    };
    let year: i32 = parts.next()?.parse().ok()?;
    let mut hms = parts.next()?.split(':');
    let (h, m, sec): (u8, u8, u8) = (
        hms.next()?.parse().ok()?,
        hms.next()?.parse().ok()?,
        hms.next()?.split('.').next()?.parse().ok()?,
    );
    let date = time::Date::from_calendar_date(year, month, day).ok()?;
    let t = time::Time::from_hms(h, m, sec).ok()?;
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let digits = zone.trim_start_matches(['+', '-']);
    if digits.len() != 4 {
        return None;
    }
    let oh: i8 = digits[..2].parse().ok()?;
    let om: i8 = digits[2..].parse().ok()?;
    let offset = UtcOffset::from_hms(sign * oh, sign * om, 0).ok()?;
    Some(
        PrimitiveDateTime::new(date, t)
            .assume_offset(offset)
            .to_offset(UtcOffset::UTC),
    )
}

/// RFC 3339 / ISO 8601 (`2026-10-22T08:00:00Z`, `…+08:00`, fractional seconds).
pub fn parse_iso(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s.trim(), &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|t| t.to_offset(UtcOffset::UTC))
}

/// Unix time in seconds, milliseconds or nanoseconds (by magnitude).
pub fn parse_epoch(v: f64) -> Option<OffsetDateTime> {
    let nanos = if v > 1e17 {
        v as i128
    } else if v > 1e11 {
        (v * 1e6) as i128
    } else {
        (v * 1e9) as i128
    };
    OffsetDateTime::from_unix_timestamp_nanos(nanos).ok()
}

/// A quoted field: `"…"` with `\"` escapes (Apache) or `\x22` (Nginx).
const QUOTED: &str = r#""((?:[^"\\]|\\.)*)""#;

static COMBINED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"^(?:(?P<vhost>[^\s\[]+) )?(?P<ip>[0-9A-Fa-f:.]+|-) \S+ (?:\S+|"[^"]*") \[(?P<time>[^\]]+)\] {q} (?P<status>\d{{3}}) (?P<bytes>\d+|-)(?: {r} {u})?"#,
        q = QUOTED.replace("((", "(?P<req>("),
        r = QUOTED.replace("((", "(?P<ref>("),
        u = QUOTED.replace("((", "(?P<ua>("),
    ))
    .expect("combined regex")
});

fn unescape(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                match u8::from_str_radix(&hex, 16) {
                    Ok(b) => out.push(b as char),
                    Err(_) => out.push_str(&format!("\\x{hex}")),
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// `GET /path HTTP/1.1` → method, path, query.
fn request_line(req: &str) -> Option<(String, String, Option<String>)> {
    let mut parts = req.split(' ');
    let method = parts.next()?.to_string();
    let target = parts.next()?;
    let (path, query) = split_target(target)?;
    Some((method, path, query))
}

/// One line of `combined` / `vhost_combined` / `common`.
pub fn parse_combined(line: &str) -> Option<Hit> {
    let c = COMBINED.captures(line)?;
    let (method, path, query) = request_line(&unescape(&c["req"]))?;
    Some(Hit {
        ts: parse_time_local(&c["time"])?,
        host: c
            .name("vhost")
            .map(|v| v.as_str().split(':').next().unwrap_or_default().to_string()),
        method: Some(method),
        path,
        query,
        status: c["status"].parse().ok()?,
        bytes: c["bytes"].parse().ok(),
        ip: c["ip"].parse().ok(),
        ua: c
            .name("ua")
            .map(|u| unescape(u.as_str()))
            .unwrap_or_default(),
        referrer: c.name("ref").and_then(|r| present(&unescape(r.as_str()))),
        cache: None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Ip,
    ForwardedFor,
    TimeLocal,
    TimeIso,
    Msec,
    Request,
    Method,
    Uri,
    Path,
    Query,
    Status,
    Bytes,
    Referrer,
    Ua,
    Host,
    Cache,
    Ignore,
}

/// A custom format compiled to a regex.
#[derive(Debug, Clone)]
pub struct CustomFormat {
    re: Regex,
    fields: Vec<Field>,
}

fn nginx_field(var: &str) -> Field {
    match var {
        "remote_addr" | "realip_remote_addr" | "binary_remote_addr" => Field::Ip,
        "http_x_forwarded_for" | "proxy_add_x_forwarded_for" | "http_cf_connecting_ip" => {
            Field::ForwardedFor
        }
        "time_local" => Field::TimeLocal,
        "time_iso8601" => Field::TimeIso,
        "msec" => Field::Msec,
        "request" => Field::Request,
        "request_method" => Field::Method,
        "request_uri" => Field::Uri,
        "uri" | "document_uri" => Field::Path,
        "args" | "query_string" => Field::Query,
        "status" => Field::Status,
        "body_bytes_sent" | "bytes_sent" => Field::Bytes,
        "http_referer" => Field::Referrer,
        "http_user_agent" => Field::Ua,
        "host" | "http_host" | "server_name" => Field::Host,
        "upstream_cache_status" => Field::Cache,
        _ => Field::Ignore,
    }
}

fn apache_field(directive: &str, arg: Option<&str>) -> Field {
    match (directive, arg.map(str::to_ascii_lowercase).as_deref()) {
        ("h" | "a", _) => Field::Ip,
        ("i", Some("x-forwarded-for")) | ("i", Some("cf-connecting-ip")) => Field::ForwardedFor,
        ("t", None) => Field::TimeLocal,
        ("r", _) => Field::Request,
        ("m", _) => Field::Method,
        ("U", _) => Field::Path,
        ("q", _) => Field::Query,
        ("s", _) => Field::Status,
        ("b" | "B" | "O", _) => Field::Bytes,
        ("i", Some("referer")) => Field::Referrer,
        ("i", Some("user-agent")) => Field::Ua,
        ("v" | "V", _) | ("i", Some("host")) => Field::Host,
        _ => Field::Ignore,
    }
}

impl CustomFormat {
    /// `nginx:` + an Nginx `log_format` string (`$var`), or `apache:` + an
    /// Apache `LogFormat` string (`%h`, `%>s`, `%{User-Agent}i`).
    pub fn compile(spec: &str) -> Result<CustomFormat, String> {
        let (kind, fmt) = spec
            .split_once(':')
            .ok_or_else(|| format!("custom format {spec:?}: start with nginx: or apache:"))?;
        let chars: Vec<char> = fmt.chars().collect();
        let mut re = String::from("^");
        let mut fields = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let field = match (kind, c) {
                ("nginx", '$') => {
                    let braced = chars.get(i + 1) == Some(&'{');
                    let start = if braced { i + 2 } else { i + 1 };
                    let mut j = start;
                    while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                        j += 1;
                    }
                    if j == start {
                        re.push_str(&regex::escape("$"));
                        i += 1;
                        continue;
                    }
                    let var: String = chars[start..j].iter().collect();
                    i = if braced { j + 1 } else { j };
                    nginx_field(&var)
                }
                ("apache", '%') => {
                    let mut j = i + 1;
                    let mut arg = None;
                    if chars.get(j) == Some(&'{') {
                        let end = chars[j..].iter().position(|c| *c == '}').map(|p| j + p);
                        let Some(end) = end else {
                            return Err("unclosed %{".into());
                        };
                        arg = Some(chars[j + 1..end].iter().collect::<String>());
                        j = end + 1;
                    }
                    while chars.get(j).is_some_and(|c| matches!(c, '<' | '>')) {
                        j += 1;
                    }
                    let Some(d) = chars.get(j) else {
                        return Err("format ends after %".into());
                    };
                    if *d == '%' {
                        re.push('%');
                        i = j + 1;
                        continue;
                    }
                    i = j + 1;
                    apache_field(&d.to_string(), arg.as_deref())
                }
                ("nginx" | "apache", _) => {
                    re.push_str(&regex::escape(&c.to_string()));
                    i += 1;
                    continue;
                }
                _ => {
                    return Err(format!(
                        "custom format kind {kind:?}: use nginx: or apache:"
                    ))
                }
            };
            // quoted context → anything but an unescaped quote; brackets → up to ]
            let quoted = re.ends_with('"');
            let bracketed = re.ends_with("\\[");
            re.push_str(if quoted {
                r#"((?:[^"\\]|\\.)*)"#
            } else if field == Field::TimeLocal && kind == "apache" && !bracketed {
                // Apache's %t prints its own brackets
                r"(\[[^\]]*\])"
            } else if bracketed {
                r"([^\]]*)"
            } else {
                r"(\S*)"
            });
            fields.push(field);
        }
        let need = [Field::Status];
        for f in need {
            if !fields.contains(&f) {
                return Err("the format has no status field ($status / %>s)".into());
            }
        }
        if !fields
            .iter()
            .any(|f| matches!(f, Field::Request | Field::Uri | Field::Path))
        {
            return Err("the format has no request or URI field".into());
        }
        if !fields
            .iter()
            .any(|f| matches!(f, Field::TimeLocal | Field::TimeIso | Field::Msec))
        {
            return Err("the format has no time field".into());
        }
        let re = Regex::new(&re).map_err(|e| e.to_string())?;
        Ok(CustomFormat { re, fields })
    }

    pub fn parse(&self, line: &str) -> Option<Hit> {
        let c = self.re.captures(line)?;
        let mut hit = Hit {
            ts: OffsetDateTime::UNIX_EPOCH,
            host: None,
            method: None,
            path: String::new(),
            query: None,
            status: 0,
            bytes: None,
            ip: None,
            ua: String::new(),
            referrer: None,
            cache: None,
        };
        let mut have_time = false;
        let mut have_path = false;
        let mut forwarded = None;
        for (i, f) in self.fields.iter().enumerate() {
            let v = unescape(c.get(i + 1).map_or("", |m| m.as_str()));
            match f {
                Field::Ip => hit.ip = v.parse().ok(),
                Field::ForwardedFor => {
                    forwarded = v.split(',').next().and_then(|s| s.trim().parse().ok())
                }
                Field::TimeLocal => {
                    hit.ts = parse_time_local(&v)?;
                    have_time = true;
                }
                Field::TimeIso => {
                    hit.ts = parse_iso(&v)?;
                    have_time = true;
                }
                Field::Msec => {
                    hit.ts = parse_epoch(v.parse().ok()?)?;
                    have_time = true;
                }
                Field::Request => {
                    let (m, p, q) = request_line(&v)?;
                    hit.method = Some(m);
                    hit.path = p;
                    hit.query = q;
                    have_path = true;
                }
                Field::Method => hit.method = present(&v),
                Field::Uri => {
                    let (p, q) = split_target(&v)?;
                    hit.path = p;
                    hit.query = q;
                    have_path = true;
                }
                Field::Path if !have_path => {
                    hit.path = split_target(&v)?.0;
                    have_path = true;
                }
                Field::Query => {
                    hit.query = present(&v).map(|q| q.trim_start_matches('?').to_string())
                }
                Field::Status => hit.status = v.parse().ok()?,
                Field::Bytes => hit.bytes = v.parse().ok(),
                Field::Referrer => hit.referrer = present(&v),
                Field::Ua => hit.ua = if v == "-" { String::new() } else { v },
                Field::Host => {
                    hit.host =
                        present(&v).map(|h| h.split(':').next().unwrap_or_default().to_string())
                }
                Field::Cache => hit.cache = present(&v),
                Field::Path | Field::Ignore => {}
            }
        }
        if hit.ip.is_none() {
            hit.ip = forwarded;
        }
        (have_time && have_path).then_some(hit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NGINX: &str = r#"66.249.66.1 - - [22/Oct/2026:08:15:03 +0800] "GET /docs/a%20b/?x=1 HTTP/1.1" 200 5123 "-" "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)""#;

    #[test]
    fn parses_combined_lines() {
        let h = parse_combined(NGINX).unwrap();
        assert_eq!(h.path, "/docs/a b/");
        assert_eq!(h.query.as_deref(), Some("x=1"));
        assert_eq!(h.status, 200);
        assert_eq!(h.ip, Some("66.249.66.1".parse().unwrap()));
        assert!(h.ua.contains("Googlebot"));
        assert_eq!(h.referrer, None);
        assert_eq!(h.ts.unix_timestamp(), 1_792_628_103);
        let vhost = format!("example.com:443 {NGINX}");
        assert_eq!(
            parse_combined(&vhost).unwrap().host.as_deref(),
            Some("example.com")
        );
        let escaped = r#"1.2.3.4 - - [22/Oct/2026:08:15:03 +0000] "GET / HTTP/1.1" 304 - "https://chatgpt.com/" "Mozilla/5.0 \"quoted\"""#;
        let h = parse_combined(escaped).unwrap();
        assert_eq!(h.ua, r#"Mozilla/5.0 "quoted""#);
        assert_eq!(h.referrer.as_deref(), Some("https://chatgpt.com/"));
        assert_eq!(h.bytes, None);
        let common = r#"1.2.3.4 - frank [22/Oct/2026:08:15:03 +0000] "GET /x HTTP/1.0" 200 10"#;
        assert_eq!(parse_combined(common).unwrap().ua, "");
        assert!(parse_combined(r#"1.2.3.4 - - [bad] "\x16\x03" 400 0 "-" "-""#).is_none());
    }

    #[test]
    fn compiles_custom_formats() {
        let f = CustomFormat::compile(
            r#"nginx:$host $remote_addr [$time_iso8601] "$request" $status $body_bytes_sent "$http_referer" "$http_user_agent" $upstream_cache_status"#,
        )
        .unwrap();
        let h = f
            .parse(r#"docs.example 20.171.207.5 [2026-10-22T08:15:03+00:00] "GET /a HTTP/2.0" 200 99 "-" "GPTBot/1.1" HIT"#)
            .unwrap();
        assert_eq!(h.host.as_deref(), Some("docs.example"));
        assert_eq!(h.cache.as_deref(), Some("HIT"));
        assert_eq!(h.ua, "GPTBot/1.1");
        let a = CustomFormat::compile(
            r#"apache:%v %h %l %u %t "%r" %>s %b "%{Referer}i" "%{User-Agent}i""#,
        )
        .unwrap();
        let h = a
            .parse(r#"e.com 1.2.3.4 - - [22/Oct/2026:08:15:03 +0000] "GET /p HTTP/1.1" 404 0 "-" "curl/8""#)
            .unwrap();
        assert_eq!(
            (h.host.as_deref(), h.status, h.path.as_str()),
            (Some("e.com"), 404, "/p")
        );
        assert!(CustomFormat::compile("nginx:$remote_addr $status").is_err());
        assert!(CustomFormat::compile("$remote_addr").is_err());
    }
}
