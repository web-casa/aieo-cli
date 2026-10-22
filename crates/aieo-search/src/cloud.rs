//! aieo.ee: list the projects an API key reaches, upload a bundle.
//!
//! `POST {endpoint}/api/projects/{project}/cli/bundles` takes the gzip
//! bundle as is (`Content-Type: application/gzip`), authenticated with an
//! organisation API key that has write scope. The same commit and kinds
//! uploaded twice are recognised (`duplicate`).

use anyhow::{bail, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::creds::AieoKey;
use crate::http::{snippet, Http};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub domain: String,
}

fn base(endpoint: &str) -> &str {
    endpoint.trim_end_matches('/')
}

fn error_of(status: u16, body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(v) => {
            let mut msg = v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("request failed")
                .to_string();
            if let Some(code) = v.get("code").and_then(Value::as_str) {
                msg = format!("{msg} ({code})");
            }
            if let Some(issues) = v.get("issues").and_then(Value::as_array) {
                let list: Vec<String> = issues
                    .iter()
                    .take(5)
                    .map(|i| {
                        i.as_str()
                            .map(String::from)
                            .unwrap_or_else(|| i.to_string())
                    })
                    .collect();
                if !list.is_empty() {
                    msg = format!("{msg}: {}", list.join("; "));
                }
            }
            format!("HTTP {status}: {msg}")
        }
        Err(_) => format!("HTTP {status}: {}", snippet(body)),
    }
}

/// Projects of the key's organisation (verifies the key, too).
pub fn projects(http: &dyn Http, key: &AieoKey) -> Result<Vec<Project>> {
    let auth = format!("Bearer {}", key.api_key);
    let r = http.get(
        &format!("{}/api/projects", base(&key.endpoint)),
        &[("Authorization", &auth)],
    )?;
    if !r.ok() {
        bail!("aieo.ee {}", error_of(r.status, &r.body));
    }
    Ok(serde_json::from_str(&r.body)?)
}

/// The project whose domain is the site's host (`www.` ignored).
pub fn project_for_host<'a>(projects: &'a [Project], host: &str) -> Option<&'a Project> {
    let bare = |h: &str| h.trim().trim_start_matches("www.").to_ascii_lowercase();
    projects.iter().find(|p| bare(&p.domain) == bare(host))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pushed {
    pub run_id: String,
    pub duplicate: bool,
    pub inside_view: Option<String>,
}

pub fn push(
    http: &dyn Http,
    key: &AieoKey,
    project: &str,
    gzip: Vec<u8>,
    allow_origin_mismatch: bool,
) -> Result<Pushed> {
    let auth = format!("Bearer {}", key.api_key);
    let mut url = format!(
        "{}/api/projects/{}/cli/bundles",
        base(&key.endpoint),
        url::form_urlencoded::byte_serialize(project.as_bytes()).collect::<String>()
    );
    if allow_origin_mismatch {
        url.push_str("?allowOriginMismatch=1");
    }
    let r = http.send(
        "POST",
        &url,
        &[("Authorization", &auth)],
        Some(("application/gzip", gzip)),
    )?;
    if !r.ok() {
        bail!("aieo.ee {}", error_of(r.status, &r.body));
    }
    let v = r.json()?;
    Ok(Pushed {
        run_id: v
            .get("runId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        duplicate: v.get("duplicate").and_then(Value::as_bool) == Some(true),
        inside_view: v
            .get("insideViewPath")
            .and_then(Value::as_str)
            .map(|p| format!("{}{p}", base(&key.endpoint))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::fake::Fake;

    fn key() -> AieoKey {
        AieoKey {
            api_key: "aieo_x".into(),
            endpoint: "https://aieo.test/".into(),
        }
    }

    #[test]
    fn pushes_and_reports_errors() {
        let ok = Fake::new(&[(
            "POST",
            "https://aieo.test/api/projects/p1/cli/bundles",
            201,
            r#"{"runId":"r1","duplicate":false,"insideViewPath":"/dashboard/inside-view"}"#,
        )]);
        let p = push(&ok, &key(), "p1", vec![1, 2], false).unwrap();
        assert_eq!(p.run_id, "r1");
        assert_eq!(
            p.inside_view.as_deref(),
            Some("https://aieo.test/dashboard/inside-view")
        );
        let bad = Fake::new(&[(
            "POST",
            "https://aieo.test/api/projects/p1/cli/bundles",
            422,
            r#"{"error":"site origin does not match the project","code":"origin_mismatch"}"#,
        )]);
        let e = push(&bad, &key(), "p1", vec![], false)
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("HTTP 422") && e.contains("origin_mismatch"),
            "{e}"
        );
    }

    #[test]
    fn finds_the_project_by_host() {
        let list = Fake::new(&[(
            "GET",
            "https://aieo.test/api/projects",
            200,
            r#"[{"id":"a","name":"A","domain":"example.com","orgId":"o"},{"id":"b","name":"B","domain":"other.com"}]"#,
        )]);
        let ps = projects(&list, &key()).unwrap();
        assert_eq!(
            project_for_host(&ps, "www.example.com").map(|p| p.id.as_str()),
            Some("a")
        );
        assert!(project_for_host(&ps, "nope.com").is_none());
    }
}
