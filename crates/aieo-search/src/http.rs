//! A small HTTP seam: real requests through ureq, scripted ones in tests.

use std::time::Duration;

use anyhow::{anyhow, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn json(&self) -> Result<serde_json::Value> {
        serde_json::from_str(&self.body).map_err(|e| {
            anyhow!(
                "expected JSON (HTTP {}): {e}: {}",
                self.status,
                snippet(&self.body)
            )
        })
    }
}

pub fn snippet(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 300 {
        format!("{}…", s.chars().take(300).collect::<String>())
    } else {
        s.to_string()
    }
}

pub trait Http: Send + Sync {
    /// `body`: content type and bytes. Non-2xx statuses are returned, not errors.
    fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<(&str, Vec<u8>)>,
    ) -> Result<Response>;

    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<Response> {
        self.send("GET", url, headers, None)
    }

    fn post_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &serde_json::Value,
    ) -> Result<Response> {
        self.send(
            "POST",
            url,
            headers,
            Some(("application/json; charset=utf-8", serde_json::to_vec(body)?)),
        )
    }

    fn post_form(&self, url: &str, fields: &[(&str, &str)]) -> Result<Response> {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields)
            .finish();
        self.send(
            "POST",
            url,
            &[],
            Some(("application/x-www-form-urlencoded", body.into_bytes())),
        )
    }
}

pub struct Ureq {
    agent: ureq::Agent,
}

impl Default for Ureq {
    fn default() -> Self {
        Ureq {
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(60)))
                .http_status_as_error(false)
                .build()
                .new_agent(),
        }
    }
}

impl Http for Ureq {
    fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<(&str, Vec<u8>)>,
    ) -> Result<Response> {
        let ua = concat!("aieo-cli/", env!("CARGO_PKG_VERSION"));
        let mut resp = match (method, body) {
            ("GET", _) => {
                let mut r = self.agent.get(url).header("User-Agent", ua);
                for (k, v) in headers {
                    r = r.header(*k, *v);
                }
                r.call()
            }
            (m, body) => {
                let mut r = match m {
                    "PUT" => self.agent.put(url),
                    _ => self.agent.post(url),
                }
                .header("User-Agent", ua);
                for (k, v) in headers {
                    r = r.header(*k, *v);
                }
                match body {
                    Some((ct, bytes)) => r.header("Content-Type", ct).send(&bytes[..]),
                    None => r.send_empty(),
                }
            }
        }
        .map_err(|e| anyhow!("{method} {url}: {e}"))?;
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .with_config()
            .limit(256 * 1024 * 1024)
            .read_to_string()
            .map_err(|e| anyhow!("{method} {url}: reading the response: {e}"))?;
        Ok(Response { status, body })
    }
}

#[cfg(test)]
pub mod fake {
    //! Scripted responses keyed by method + URL prefix; records requests.

    use std::sync::Mutex;

    use super::*;

    pub struct Fake {
        pub routes: Vec<(String, String, u16, String)>,
        pub seen: Mutex<Vec<(String, String, Option<String>)>>,
    }

    impl Fake {
        pub fn new(routes: &[(&str, &str, u16, &str)]) -> Fake {
            Fake {
                routes: routes
                    .iter()
                    .map(|(m, u, s, b)| (m.to_string(), u.to_string(), *s, b.to_string()))
                    .collect(),
                seen: Mutex::new(Vec::new()),
            }
        }

        pub fn bodies(&self, url_prefix: &str) -> Vec<String> {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, u, _)| u.starts_with(url_prefix))
                .filter_map(|(_, _, b)| b.clone())
                .collect()
        }
    }

    impl Http for Fake {
        fn send(
            &self,
            method: &str,
            url: &str,
            _headers: &[(&str, &str)],
            body: Option<(&str, Vec<u8>)>,
        ) -> Result<Response> {
            self.seen.lock().unwrap().push((
                method.to_string(),
                url.to_string(),
                body.map(|(_, b)| String::from_utf8_lossy(&b).into_owned()),
            ));
            let (_, _, status, body) = self
                .routes
                .iter()
                .find(|(m, u, _, _)| m == method && url.starts_with(u.as_str()))
                .ok_or_else(|| anyhow!("unscripted {method} {url}"))?;
            Ok(Response {
                status: *status,
                body: body.clone(),
            })
        }
    }
}
