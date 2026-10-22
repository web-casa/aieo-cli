//! Crawler traffic from Cloudflare's GraphQL Analytics API
//! (`httpRequestsAdaptiveGroups`), for zones without Logpush.
//!
//! The dataset is sampled and aggregated: rows are (hour, user agent, path,
//! host[, status]) with a count, so they are stored with that count as
//! `weight` and without IPs — crawler claims stay `unchecked`. Only catalog
//! crawlers are requested (an `OR` of `userAgent_like` filters). Limits
//! differ per plan; they are read from the zone's `settings` node, and a
//! window that returns a full page is split until it does not.
//!
//! Docs: developers.cloudflare.com/analytics/graphql-api/ and
//! …/ai-crawl-control/reference/graphql-api/.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

use crate::classify::classify;
use crate::hit::split_target;
use crate::store::{Counts, SourceRow, Store, StoredHit};

pub const ENDPOINT: &str = "https://api.cloudflare.com/client/v4/graphql";

/// Sends one GraphQL request body, returns the response body.
pub type Post = Box<dyn Fn(&Value) -> Result<Value>>;

pub fn http_post(endpoint: String, token: String) -> Post {
    Box::new(move |body: &Value| {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(60)))
            .http_status_as_error(false)
            .build()
            .new_agent();
        let mut resp = agent
            .post(&endpoint)
            .header("Authorization", &format!("Bearer {token}"))
            .header(
                "User-Agent",
                concat!("aieo-cli/", env!("CARGO_PKG_VERSION")),
            )
            .header("Content-Type", "application/json")
            .send(serde_json::to_string(body)?.as_bytes())
            .map_err(|e| anyhow!("Cloudflare API: {e}"))?;
        let status = resp.status();
        let text = resp.body_mut().read_to_string()?;
        let v: Value = serde_json::from_str(&text)
            .with_context(|| format!("Cloudflare API answered {status} with non-JSON"))?;
        Ok(v)
    })
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// longest window one query may span, seconds
    pub max_duration: i64,
    pub page_size: u64,
    /// how far back data exists, seconds
    pub not_older_than: i64,
}

impl Default for Limits {
    fn default() -> Self {
        // conservative: one day per query, a month back
        Limits {
            max_duration: 86_400,
            page_size: 5_000,
            not_older_than: 31 * 86_400,
        }
    }
}

fn errors(v: &Value) -> Option<String> {
    let errs = v.get("errors")?.as_array()?;
    if errs.is_empty() {
        return None;
    }
    Some(
        errs.iter()
            .filter_map(|e| e.get("message").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

fn quote(s: &str) -> String {
    serde_json::to_string(s).expect("string")
}

/// The zone's limits for the dataset (defaults when the settings node is not readable).
pub fn limits(post: &Post, zone: &str) -> (Limits, Option<String>) {
    let q = format!(
        "{{ viewer {{ zones(filter: {{zoneTag: {}}}) {{ settings {{ httpRequestsAdaptiveGroups {{ enabled maxDuration maxPageSize notOlderThan }} }} }} }} }}",
        quote(zone)
    );
    let v = match post(&json!({ "query": q })) {
        Ok(v) => v,
        Err(e) => return (Limits::default(), Some(format!("settings: {e}"))),
    };
    if let Some(e) = errors(&v) {
        return (Limits::default(), Some(format!("settings: {e}")));
    }
    let s = &v["data"]["viewer"]["zones"][0]["settings"]["httpRequestsAdaptiveGroups"];
    let num = |k: &str| s.get(k).and_then(Value::as_i64);
    let d = Limits::default();
    (
        Limits {
            max_duration: num("maxDuration")
                .filter(|n| *n > 0)
                .unwrap_or(d.max_duration),
            page_size: num("maxPageSize")
                .filter(|n| *n > 0)
                .map_or(d.page_size, |n| n as u64)
                .min(10_000),
            not_older_than: num("notOlderThan")
                .filter(|n| *n > 0)
                .unwrap_or(d.not_older_than),
        },
        None,
    )
}

/// `OR` of the catalog crawlers' user-agent tokens.
fn crawler_filter() -> String {
    let parts: Vec<String> = aieo_core::spec::spec()
        .crawlers
        .iter()
        .filter_map(|c| c.ua_pattern.as_ref())
        .map(|p| format!("{{userAgent_like: {}}}", quote(&format!("%{p}%"))))
        .collect();
    format!("OR: [{}]", parts.join(", "))
}

fn rfc(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).expect("RFC 3339")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub hour: OffsetDateTime,
    pub ua: String,
    pub path: String,
    pub host: Option<String>,
    /// None when the status could not be grouped on (see `status_class`)
    pub status: Option<u16>,
    /// set when the query filtered on a status class instead
    pub status_class: Option<u16>,
    pub count: u64,
}

fn query(
    zone: &str,
    from: OffsetDateTime,
    to: OffsetDateTime,
    limit: u64,
    with_status: bool,
    class: Option<(u16, u16)>,
) -> String {
    let status_filter = class
        .map(|(lo, hi)| format!(", edgeResponseStatus_geq: {lo}, edgeResponseStatus_lt: {hi}"))
        .unwrap_or_default();
    format!(
        "{{ viewer {{ zones(filter: {{zoneTag: {zone}}}) {{ httpRequestsAdaptiveGroups(filter: {{datetime_geq: {from}, datetime_lt: {to}, requestSource: \"eyeball\"{status_filter}, {or}}}, limit: {limit}, orderBy: [count_DESC]) {{ count dimensions {{ userAgent clientRequestPath clientRequestHTTPHost datetimeHour{status} }} }} }} }} }}",
        zone = quote(zone),
        from = quote(&rfc(from)),
        to = quote(&rfc(to)),
        or = crawler_filter(),
        status = if with_status { " edgeResponseStatus" } else { "" },
    )
}

fn rows_of(v: &Value, class: Option<(u16, u16)>) -> Result<Vec<Row>> {
    let groups = v["data"]["viewer"]["zones"][0]["httpRequestsAdaptiveGroups"]
        .as_array()
        .context("unexpected Cloudflare response: no httpRequestsAdaptiveGroups")?;
    let mut out = Vec::new();
    for g in groups {
        let d = &g["dimensions"];
        let Some(hour) = d["datetimeHour"]
            .as_str()
            .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        else {
            continue;
        };
        out.push(Row {
            hour,
            ua: d["userAgent"].as_str().unwrap_or_default().to_string(),
            path: d["clientRequestPath"].as_str().unwrap_or("/").to_string(),
            host: d["clientRequestHTTPHost"].as_str().map(String::from),
            status: d["edgeResponseStatus"].as_u64().map(|s| s as u16),
            status_class: class.map(|(lo, _)| lo),
            count: g["count"].as_u64().unwrap_or(0),
        });
    }
    Ok(out)
}

/// Status ranges queried when status cannot be grouped on; 403 apart, as it is how WAFs refuse crawlers.
const CLASSES: [(u16, u16); 6] = [
    (200, 300),
    (300, 400),
    (400, 403),
    (403, 404),
    (404, 500),
    (500, 600),
];

/// Rows for `[from, to)`, splitting windows whose page came back full.
pub fn fetch_window(
    post: &Post,
    zone: &str,
    from: OffsetDateTime,
    to: OffsetDateTime,
    limits: &Limits,
    with_status: &mut bool,
    notes: &mut Vec<String>,
) -> Result<Vec<Row>> {
    let mut out = Vec::new();
    let classes: Vec<Option<(u16, u16)>> = if *with_status {
        vec![None]
    } else {
        CLASSES.iter().copied().map(Some).collect()
    };
    for class in classes {
        let v = post(
            &json!({ "query": query(zone, from, to, limits.page_size, *with_status, class) }),
        )?;
        if let Some(e) = errors(&v) {
            if *with_status && e.contains("edgeResponseStatus") {
                // not groupable on this plan / dataset: query per status class
                *with_status = false;
                notes.push(
                    "status is not a groupable dimension here; queried per status class".into(),
                );
                return fetch_window(post, zone, from, to, limits, with_status, notes);
            }
            bail!("Cloudflare API: {e}");
        }
        let rows = rows_of(&v, class)?;
        if rows.len() as u64 >= limits.page_size && to - from > Duration::hours(1) {
            let mid = from + (to - from) / 2;
            out.extend(fetch_window(
                post,
                zone,
                from,
                mid,
                limits,
                with_status,
                notes,
            )?);
            out.extend(fetch_window(
                post,
                zone,
                mid,
                to,
                limits,
                with_status,
                notes,
            )?);
            continue;
        }
        if rows.len() as u64 >= limits.page_size {
            notes.push(format!(
                "{}: a full page in one hour — rows beyond {} are missing",
                rfc(from),
                limits.page_size
            ));
        }
        out.extend(rows);
    }
    Ok(out)
}

fn hex16(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A representative code for a status class row (the class is all we know).
fn status_of(row: &Row) -> u16 {
    row.status.unwrap_or(match row.status_class {
        Some(200) => 200,
        Some(300) => 301,
        Some(400) => 400,
        Some(403) => 403,
        Some(404) => 404,
        _ => 500,
    })
}

#[derive(Debug, Default)]
pub struct PullReport {
    pub from: Option<OffsetDateTime>,
    pub to: Option<OffsetDateTime>,
    pub rows: u64,
    pub requests: u64,
    pub notes: Vec<String>,
}

/// Pull `days` back from now (within what the plan keeps) into the store.
pub fn pull(
    store: &mut Store,
    post: &Post,
    zone: &str,
    days: i64,
    now: OffsetDateTime,
) -> Result<PullReport> {
    let (limits, note) = limits(post, zone);
    let mut report = PullReport::default();
    report.notes.extend(note);
    let end = now
        .replace_minute(0)?
        .replace_second(0)?
        .replace_nanosecond(0)?;
    let oldest = end - Duration::seconds(limits.not_older_than) + Duration::hours(1);
    let mut start = end - Duration::days(days);
    if start < oldest {
        report.notes.push(format!(
            "the plan keeps {} days of analytics; pulling from {}",
            limits.not_older_than / 86_400,
            rfc(oldest)
        ));
        start = oldest;
    }
    report.from = Some(start);
    report.to = Some(end);
    let mut with_status = true;
    let mut rows = Vec::new();
    let mut t = start;
    while t < end {
        let next = (t + Duration::seconds(limits.max_duration)).min(end);
        rows.extend(fetch_window(
            post,
            zone,
            t,
            next,
            &limits,
            &mut with_status,
            &mut report.notes,
        )?);
        t = next;
    }
    let mut hits = Vec::new();
    for r in &rows {
        let class = classify(&r.ua);
        let Some(crawler) = class.crawler else {
            continue;
        };
        let Some((path, _)) = split_target(&r.path) else {
            continue;
        };
        let status = status_of(r);
        report.rows += 1;
        report.requests += r.count;
        hits.push(StoredHit {
            hit_id: hex16(&format!(
                "cf\u{0}{zone}\u{0}{}\u{0}{}\u{0}{}\u{0}{}",
                rfc(r.hour),
                r.ua,
                r.path,
                status
            )),
            ts: r.hour.unix_timestamp(),
            day: r.hour.date().to_string(),
            host: r.host.clone(),
            path,
            status,
            bytes: None,
            actor: class.actor.as_str().into(),
            bot: Some(crawler.id.clone()),
            verdict: Some("unchecked".into()),
            list_version: None,
            ip_hash: None,
            ua: Some(r.ua.chars().take(300).collect()),
            platform: None,
            cache: None,
            weight: r.count,
        });
    }
    let identity = format!("cloudflare:{zone}");
    let mut source = store.source(&identity)?.unwrap_or(SourceRow {
        identity: identity.clone(),
        path: format!("Cloudflare GraphQL zone {zone}"),
        format: "cloudflare-graphql".into(),
        ..Default::default()
    });
    let (lo, hi) = (start.unix_timestamp(), end.unix_timestamp());
    source.first_ts = Some(source.first_ts.map_or(lo, |t| t.min(lo)));
    source.last_ts = Some(source.last_ts.map_or(hi, |t| t.max(hi)));
    source.lines = report.rows;
    source.hits = report.requests;
    store.commit_replacing(
        &hits,
        &std::collections::HashMap::<(String, String, String), Counts>::new(),
        &source,
    )?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn pulls_and_splits_full_pages() {
        let calls = Rc::new(RefCell::new(Vec::<String>::new()));
        let seen = calls.clone();
        let post: Post = Box::new(move |body: &Value| {
            let q = body["query"].as_str().unwrap().to_string();
            seen.borrow_mut().push(q.clone());
            if q.contains("settings") {
                return Ok(
                    json!({"data":{"viewer":{"zones":[{"settings":{"httpRequestsAdaptiveGroups":{"enabled":true,"maxDuration":259200,"maxPageSize":2,"notOlderThan":2678400}}}]}}}),
                );
            }
            if q.contains("dimensions { userAgent clientRequestPath clientRequestHTTPHost datetimeHour edgeResponseStatus }") {
                return Ok(json!({"data":null,"errors":[{"message":"unknown field \"edgeResponseStatus\""}]}));
            }
            // only the 403 class has rows: GPTBot blocked twice in one hour, OAI-SearchBot fine
            if q.contains("edgeResponseStatus_geq: 403") {
                return Ok(
                    json!({"data":{"viewer":{"zones":[{"httpRequestsAdaptiveGroups":[
                    {"count":7,"dimensions":{"userAgent":"Mozilla/5.0 (compatible; GPTBot/1.2)","clientRequestPath":"/docs/","clientRequestHTTPHost":"e.com","datetimeHour":"2026-10-21T10:00:00Z"}}]}]}}}),
                );
            }
            if q.contains("edgeResponseStatus_geq: 200") {
                return Ok(
                    json!({"data":{"viewer":{"zones":[{"httpRequestsAdaptiveGroups":[
                    {"count":3,"dimensions":{"userAgent":"OAI-SearchBot/1.3","clientRequestPath":"/","clientRequestHTTPHost":"e.com","datetimeHour":"2026-10-21T11:00:00Z"}}]}]}}}),
                );
            }
            Ok(json!({"data":{"viewer":{"zones":[{"httpRequestsAdaptiveGroups":[]}]}}}))
        });
        let mut store = Store::in_memory().unwrap();
        let now = OffsetDateTime::parse("2026-10-22T00:30:00Z", &Rfc3339).unwrap();
        let r = pull(&mut store, &post, "zone1", 2, now).unwrap();
        assert_eq!((r.rows, r.requests), (2, 10));
        assert!(r.notes.iter().any(|n| n.contains("per status class")));
        let weight: i64 = store
            .conn
            .query_row(
                "SELECT SUM(weight) FROM hits WHERE bot = 'gptbot' AND status = 403",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(weight, 7);
        // pulling again replaces the same rows instead of adding to them
        pull(&mut store, &post, "zone1", 2, now).unwrap();
        let total: i64 = store
            .conn
            .query_row("SELECT SUM(weight) FROM hits", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 10);
        let q = calls.borrow();
        assert!(q.iter().any(|q| q.contains("userAgent_like: \"%GPTBot%\"")));
        assert!(q
            .iter()
            .all(|q| q.contains("settings") || q.contains("requestSource: \"eyeball\"")));
    }
}
