//! JSON-per-line formats and CloudFront's W3C files.
//!
//! - Caddy (`http.log.access`): `ts` (epoch seconds float, or a string when
//!   `time_format` is set), `request.{remote_ip,client_ip,host,method,uri,
//!   headers.User-Agent[],headers.Referer[]}`, `status`, `size`.
//! - Cloudflare Logpush `http_requests`: `ClientIP`, `ClientRequestHost`,
//!   `ClientRequestMethod`, `ClientRequestURI` (path + query) or
//!   `ClientRequestPath`, `ClientRequestUserAgent`, `ClientRequestReferer`,
//!   `EdgeResponseStatus`, `EdgeResponseBytes`, `CacheCacheStatus`,
//!   `EdgeStartTimestamp` (unixnano / unix / RFC 3339 variants).
//! - Vercel drains: lines with a `proxy` object — `proxy.{timestamp (ms),
//!   method, host, path (with query), userAgent[], referer, statusCode,
//!   clientIp, vercelCache}`; one request may log several lines, so
//!   `requestId` identifies it.
//! - CloudFront standard logs: v1 W3C (`#Fields:` header, tab-separated,
//!   UTC `date` + `time`, percent-encoded values) and v2 JSON with the same
//!   field names.

use percent_encoding::percent_decode_str;
use serde_json::Value;

use super::text::{parse_epoch, parse_iso};
use crate::hit::{present, split_target, Hit};

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    match cur {
        Value::String(s) => Some(s.as_str()),
        Value::Array(a) => a.first()?.as_str(),
        _ => None,
    }
}

fn num_at(v: &Value, path: &[&str]) -> Option<f64> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    match cur {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn time_at(v: &Value, path: &[&str]) -> Option<time::OffsetDateTime> {
    num_at(v, path)
        .and_then(parse_epoch)
        .or_else(|| str_at(v, path).and_then(parse_iso))
}

/// A parsed line and, when the format has one, a request id for de-duplication.
pub type Parsed = (Hit, Option<String>);

pub fn caddy(v: &Value) -> Option<Parsed> {
    let (path, query) = split_target(str_at(v, &["request", "uri"])?)?;
    let ip = str_at(v, &["request", "client_ip"])
        .filter(|s| !s.is_empty())
        .or_else(|| str_at(v, &["request", "remote_ip"]))
        .and_then(|s| s.parse().ok());
    Some((
        Hit {
            ts: time_at(v, &["ts"])?,
            host: str_at(v, &["request", "host"]).and_then(present),
            method: str_at(v, &["request", "method"]).map(String::from),
            path,
            query,
            status: num_at(v, &["status"])? as u16,
            bytes: num_at(v, &["size"]).map(|b| b as u64),
            ip,
            ua: str_at(v, &["request", "headers", "User-Agent"])
                .unwrap_or_default()
                .to_string(),
            referrer: str_at(v, &["request", "headers", "Referer"]).and_then(present),
            cache: None,
        },
        None,
    ))
}

pub fn cloudflare(v: &Value) -> Option<Parsed> {
    let (path, query) = match str_at(v, &["ClientRequestURI"]) {
        Some(uri) => split_target(uri)?,
        None => split_target(str_at(v, &["ClientRequestPath"])?)?,
    };
    Some((
        Hit {
            ts: time_at(v, &["EdgeStartTimestamp"])?,
            host: str_at(v, &["ClientRequestHost"]).and_then(present),
            method: str_at(v, &["ClientRequestMethod"]).map(String::from),
            path,
            query,
            status: num_at(v, &["EdgeResponseStatus"])? as u16,
            bytes: num_at(v, &["EdgeResponseBytes"]).map(|b| b as u64),
            ip: str_at(v, &["ClientIP"]).and_then(|s| s.parse().ok()),
            ua: str_at(v, &["ClientRequestUserAgent"])
                .unwrap_or_default()
                .to_string(),
            referrer: str_at(v, &["ClientRequestReferer"]).and_then(present),
            cache: str_at(v, &["CacheCacheStatus"]).and_then(present),
        },
        str_at(v, &["RayID"]).map(String::from),
    ))
}

pub fn vercel(v: &Value) -> Option<Parsed> {
    let proxy = v.get("proxy")?;
    let status = num_at(proxy, &["statusCode"]).filter(|s| *s >= 100.0)?;
    let (path, query) = split_target(str_at(proxy, &["path"])?)?;
    Some((
        Hit {
            ts: time_at(proxy, &["timestamp"]).or_else(|| time_at(v, &["timestamp"]))?,
            host: str_at(proxy, &["host"])
                .or_else(|| str_at(v, &["host"]))
                .and_then(present),
            method: str_at(proxy, &["method"]).map(String::from),
            path,
            query,
            status: status as u16,
            bytes: num_at(proxy, &["responseByteSize"]).map(|b| b as u64),
            ip: str_at(proxy, &["clientIp"]).and_then(|s| s.parse().ok()),
            ua: str_at(proxy, &["userAgent"])
                .unwrap_or_default()
                .to_string(),
            referrer: str_at(proxy, &["referer"]).and_then(present),
            cache: str_at(proxy, &["vercelCache"]).and_then(present),
        },
        str_at(v, &["requestId"])
            .or_else(|| str_at(proxy, &["vercelId"]))
            .map(String::from),
    ))
}

fn decode(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

/// A CloudFront record as field name → value (v1 row or v2 JSON object).
pub fn cloudfront(get: &dyn Fn(&str) -> Option<String>) -> Option<Parsed> {
    let field = |k: &str| get(k).and_then(|s| present(&s));
    let status: u16 = field("sc-status")?.parse().ok()?;
    if status == 0 {
        return None; // the viewer closed the connection before a response
    }
    let ts = match field("timestamp(ms)").and_then(|ms| ms.parse::<f64>().ok()) {
        Some(ms) => parse_epoch(ms)?,
        None => parse_iso(&format!("{}T{}Z", field("date")?, field("time")?))?,
    };
    let stem = field("cs-uri-stem")?;
    let path = split_target(&stem)?.0;
    let ip = field("x-forwarded-for")
        .and_then(|x| x.split(',').next().and_then(|s| s.trim().parse().ok()))
        .or_else(|| field("c-ip").and_then(|s| s.parse().ok()));
    Some((
        Hit {
            ts,
            host: field("x-host-header").or_else(|| field("cs(Host)")),
            method: field("cs-method"),
            path,
            query: field("cs-uri-query"),
            status,
            bytes: field("sc-bytes").and_then(|b| b.parse().ok()),
            ip,
            ua: field("cs(User-Agent)")
                .map(|u| decode(&u))
                .unwrap_or_default(),
            referrer: field("cs(Referer)").map(|r| decode(&r)),
            cache: field("x-edge-result-type"),
        },
        field("x-edge-request-id"),
    ))
}

pub fn cloudfront_json(v: &Value) -> Option<Parsed> {
    cloudfront(&|k| {
        str_at(v, &[k])
            .map(String::from)
            .or_else(|| num_at(v, &[k]).map(|n| n.to_string()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn j(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn caddy_lines() {
        let (h, _) = caddy(&j(r#"{"level":"info","ts":1646861401.5241024,"logger":"http.log.access","msg":"handled request","request":{"remote_ip":"127.0.0.1","remote_port":"41342","client_ip":"203.0.113.9","proto":"HTTP/2.0","method":"GET","host":"localhost","uri":"/a?b=1","headers":{"User-Agent":["curl/7.82.0"],"Referer":["https://perplexity.ai/"]}},"bytes_read":0,"duration":0.0009,"size":10900,"status":200}"#)).unwrap();
        assert_eq!(
            (h.path.as_str(), h.query.as_deref(), h.status),
            ("/a", Some("b=1"), 200)
        );
        assert_eq!(h.ip, Some("203.0.113.9".parse().unwrap()));
        assert_eq!(h.referrer.as_deref(), Some("https://perplexity.ai/"));
        assert_eq!(h.ts.unix_timestamp(), 1_646_861_401);
    }

    #[test]
    fn cloudflare_timestamps() {
        for ts in [
            "1506702504433000200",
            "\"2017-09-29T16:28:24Z\"",
            "1506702504",
        ] {
            let (h, id) = cloudflare(&j(&format!(r#"{{"ClientIP":"89.163.242.206","ClientRequestHost":"e.com","ClientRequestMethod":"GET","ClientRequestURI":"/x?y","ClientRequestUserAgent":"GPTBot/1.1","EdgeResponseStatus":403,"EdgeResponseBytes":10,"EdgeStartTimestamp":{ts},"RayID":"r1","CacheCacheStatus":"dynamic"}}"#))).unwrap();
            assert_eq!(h.ts.unix_timestamp(), 1_506_702_504, "{ts}");
            assert_eq!(
                (h.status, h.path.as_str(), id.as_deref()),
                (403, "/x", Some("r1"))
            );
        }
    }

    #[test]
    fn vercel_lines() {
        let line = r#"{"id": "1573817250283254651097202070","deploymentId": "dpl_x","source": "lambda","host": "my-app-abc123.vercel.app","timestamp": 1573817250283,"requestId": "643af4e3","statusCode": 200,"path": "/api/users","proxy": {"timestamp": 1573817250172,"method": "GET","host": "my-app.vercel.app","path": "/api/users?page=1","userAgent": ["Mozilla/5.0..."],"referer": "https://my-app.vercel.app","region": "sfo1","statusCode": 200,"clientIp": "120.75.16.101","scheme": "https","vercelCache": "MISS"}}"#;
        let (h, id) = vercel(&j(line)).unwrap();
        assert_eq!(
            (h.path.as_str(), h.host.as_deref(), id.as_deref()),
            ("/api/users", Some("my-app.vercel.app"), Some("643af4e3"))
        );
        assert_eq!(h.ts.unix_timestamp(), 1_573_817_250);
        assert!(vercel(&j(r#"{"source":"build","message":"x"}"#)).is_none());
        assert!(vercel(&j(
            r#"{"proxy":{"statusCode":-1,"path":"/","timestamp":1}}"#
        ))
        .is_none());
    }
}
