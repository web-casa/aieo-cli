//! IndexNow: tell participating search engines (Bing, Yandex, Seznam,
//! Naver, …) which URLs changed. One engine shares submissions with the
//! others; Google does not take part.
//!
//! The key (8–128 of `a-zA-Z0-9-`) is public: the site serves it as
//! `/<key>.txt` (or at `keyLocation`), and every submitted URL must be on
//! that host. A request carries at most 10,000 URLs.

use anyhow::{bail, Result};
use serde_json::json;
use url::Url;

use crate::http::Http;

pub const DEFAULT_ENDPOINT: &str = "https://api.indexnow.org/indexnow";
pub const MAX_URLS: usize = 10_000;

pub fn valid_key(key: &str) -> bool {
    (8..=128).contains(&key.len()) && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A new random key (32 hex characters).
pub fn keygen() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Where the key file is served: `keyLocation` when set, else `/<key>.txt`.
pub fn key_url(origin: &Url, key: &str, key_location: Option<&str>) -> Result<Url> {
    Ok(match key_location {
        Some(loc) => origin.join(loc)?,
        None => origin.join(&format!("/{key}.txt"))?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyFile {
    /// served and holds the key
    Valid,
    /// served, different content
    Wrong(String),
    /// not served (status)
    Missing(u16),
}

/// Check the live key file.
pub fn verify(http: &dyn Http, url: &Url, key: &str) -> Result<KeyFile> {
    let r = http.get(url.as_str(), &[])?;
    if !r.ok() {
        return Ok(KeyFile::Missing(r.status));
    }
    let body = r.body.trim_start_matches('\u{feff}').trim();
    Ok(if body == key {
        KeyFile::Valid
    } else {
        KeyFile::Wrong(body.chars().take(80).collect())
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub endpoint: String,
    pub host: String,
    pub key: String,
    pub key_location: Option<String>,
    pub batches: Vec<Vec<String>>,
    /// URLs left out (another host, not http(s))
    pub rejected: Vec<String>,
}

impl Plan {
    pub fn urls(&self) -> usize {
        self.batches.iter().map(Vec::len).sum()
    }
}

/// Deduplicate, keep URLs on the site's host, split into requests.
pub fn plan(
    endpoint: &str,
    origin: &Url,
    key: &str,
    key_location: Option<&str>,
    urls: impl IntoIterator<Item = String>,
) -> Result<Plan> {
    if !valid_key(key) {
        bail!("IndexNow key must be 8–128 characters of a-z, A-Z, 0-9 and '-'");
    }
    let host = origin.host_str().unwrap_or_default().to_ascii_lowercase();
    let mut seen = std::collections::BTreeSet::new();
    let mut keep = Vec::new();
    let mut rejected = Vec::new();
    for raw in urls {
        match Url::parse(&raw) {
            Ok(u)
                if matches!(u.scheme(), "http" | "https")
                    && u.host_str().map(str::to_ascii_lowercase).as_deref()
                        == Some(host.as_str()) =>
            {
                if seen.insert(u.to_string()) {
                    keep.push(u.to_string());
                }
            }
            _ => rejected.push(raw),
        }
    }
    let key_location = key_location
        .map(|l| origin.join(l).map(|u| u.to_string()))
        .transpose()?;
    Ok(Plan {
        endpoint: endpoint.to_string(),
        host,
        key: key.to_string(),
        key_location,
        batches: keep.chunks(MAX_URLS).map(<[String]>::to_vec).collect(),
        rejected,
    })
}

/// What an IndexNow status code means.
pub fn meaning(status: u16) -> &'static str {
    match status {
        200 => "accepted",
        202 => "received; the key is still being validated",
        400 => "bad request (invalid format)",
        403 => "key not valid (key file missing or different)",
        422 => "URLs do not belong to the host, or the key does not match the schema",
        429 => "too many requests (possible spam)",
        _ => "unexpected response",
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sent {
    pub urls: usize,
    pub status: u16,
}

/// Send every batch; stops at the first failure.
pub fn submit(http: &dyn Http, plan: &Plan) -> Result<Vec<Sent>> {
    let mut out = Vec::new();
    for batch in &plan.batches {
        let mut body = json!({ "host": plan.host, "key": plan.key, "urlList": batch });
        if let Some(loc) = &plan.key_location {
            body["keyLocation"] = json!(loc);
        }
        let r = http.post_json(&plan.endpoint, &[], &body)?;
        out.push(Sent {
            urls: batch.len(),
            status: r.status,
        });
        if !matches!(r.status, 200 | 202) {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::fake::Fake;

    #[test]
    fn keys_plans_and_submissions() {
        assert!(valid_key("abcd-1234"));
        assert!(!valid_key("short"));
        assert!(!valid_key("has_underscore_123"));
        let k = keygen().unwrap();
        assert!(valid_key(&k) && k.len() == 32);
        let origin = Url::parse("https://example.com").unwrap();
        assert_eq!(
            key_url(&origin, &k, None).unwrap().path(),
            format!("/{k}.txt")
        );
        let urls = (0..10_002)
            .map(|i| format!("https://example.com/p/{i}"))
            .chain([
                "https://example.com/p/0".to_string(),
                "https://other.com/x".to_string(),
                "mailto:a@b".to_string(),
            ]);
        let p = plan(DEFAULT_ENDPOINT, &origin, &k, None, urls).unwrap();
        assert_eq!(
            (p.urls(), p.batches.len(), p.rejected.len()),
            (10_002, 2, 2)
        );

        let http = Fake::new(&[
            (
                "GET",
                "https://example.com/",
                200,
                &format!("\u{feff}{k}\n"),
            ),
            ("POST", DEFAULT_ENDPOINT, 202, ""),
        ]);
        assert_eq!(
            verify(&http, &key_url(&origin, &k, None).unwrap(), &k).unwrap(),
            KeyFile::Valid
        );
        let sent = submit(&http, &p).unwrap();
        assert_eq!(sent.iter().map(|s| s.urls).sum::<usize>(), 10_002);
        let body: serde_json::Value =
            serde_json::from_str(&http.bodies(DEFAULT_ENDPOINT)[1]).unwrap();
        assert_eq!(body["host"], "example.com");
        assert_eq!(body["urlList"].as_array().unwrap().len(), 2);

        let forbidden = Fake::new(&[("POST", DEFAULT_ENDPOINT, 403, "")]);
        let sent = submit(&forbidden, &p).unwrap();
        assert_eq!(sent.len(), 1, "stops after a refused batch");
        assert!(meaning(sent[0].status).contains("key not valid"));
    }
}
