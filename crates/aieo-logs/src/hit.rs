//! One request as any log format describes it.

use std::net::IpAddr;

use percent_encoding::percent_decode_str;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// request time, UTC
    pub ts: OffsetDateTime,
    pub host: Option<String>,
    pub method: Option<String>,
    /// URL path, percent-decoded, without the query
    pub path: String,
    pub query: Option<String>,
    pub status: u16,
    pub bytes: Option<u64>,
    pub ip: Option<IpAddr>,
    pub ua: String,
    pub referrer: Option<String>,
    /// CDN cache status when the format has one (HIT, MISS, …)
    pub cache: Option<String>,
}

/// Split a request target (`/a/b?x=1`, or an absolute URL from a proxy log)
/// into a decoded path and the raw query.
pub fn split_target(target: &str) -> Option<(String, Option<String>)> {
    let target = target.trim();
    let rest = if let Some(after) = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))
    {
        &after[after.find('/')?..]
    } else {
        target
    };
    if !rest.starts_with('/') {
        return None;
    }
    let rest = rest.split('#').next().unwrap_or(rest);
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, (!q.is_empty()).then(|| q.to_string())),
        None => (rest, None),
    };
    Some((
        percent_decode_str(path).decode_utf8_lossy().into_owned(),
        query,
    ))
}

/// `"-"` and empty mean "not logged".
pub fn present(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && s != "-").then(|| s.to_string())
}

/// Status class: 2, 3, 4 or 5 (1xx counts as 2, anything else as 5).
pub fn status_class(status: u16) -> usize {
    match status {
        100..=299 => 2,
        300..=399 => 3,
        400..=499 => 4,
        _ => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_targets() {
        assert_eq!(
            split_target("/caf%C3%A9/a?x=1#f"),
            Some(("/café/a".into(), Some("x=1".into())))
        );
        assert_eq!(split_target("https://e.com/p?"), Some(("/p".into(), None)));
        assert_eq!(split_target("*"), None);
        assert_eq!(present(" - "), None);
        assert_eq!(status_class(404), 4);
    }
}
