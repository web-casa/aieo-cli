//! Search Console API: properties, Search Analytics, URL Inspection.
//!
//! Search Analytics is paged with `startRow` (up to 25,000 rows a call)
//! until a short page. URL Inspection spends from the local quota ledger
//! (2,000 inspections per property per day, 600 a minute) and inspects the
//! least recently inspected URLs first.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use time::{Date, OffsetDateTime};

use crate::http::{snippet, Http};
use crate::store::{Inspection, SearchStore};

pub const API: &str = "https://searchconsole.googleapis.com";
pub const PAGE_ROWS: u64 = 25_000;
pub const INSPECT_DAILY: u64 = 2_000;
pub const INSPECT_PER_MINUTE: u32 = 600;

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn check(r: &crate::http::Response, what: &str) -> Result<Value> {
    if r.ok() {
        return r.json();
    }
    let msg = r
        .json()
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(String::from))
        .unwrap_or_else(|| snippet(&r.body));
    match r.status {
        401 => bail!("{what}: not authorized ({msg}) — log in again"),
        403 => bail!("{what}: no access ({msg}) — is this account (or service account) a user of the property?"),
        429 => bail!("{what}: quota exceeded ({msg})"),
        s => bail!("{what}: HTTP {s}: {msg}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    pub site_url: String,
    pub permission: String,
}

pub fn properties(http: &dyn Http, token: &str) -> Result<Vec<Property>> {
    let auth = format!("Bearer {token}");
    let r = http.get(
        &format!("{API}/webmasters/v3/sites"),
        &[("Authorization", &auth)],
    )?;
    let v = check(&r, "listing Search Console properties")?;
    Ok(v["siteEntry"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some(Property {
                site_url: e["siteUrl"].as_str()?.to_string(),
                permission: e["permissionLevel"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect())
}

/// The property covering `origin`: a domain property for its host (or a
/// parent domain), else the longest URL-prefix property it starts with.
pub fn property_for(props: &[Property], origin: &url::Url) -> Option<String> {
    let host = origin.host_str()?.to_ascii_lowercase();
    let usable = props
        .iter()
        .filter(|p| p.permission != "siteUnverifiedUser");
    let domain = usable.clone().find_map(|p| {
        let d = p.site_url.strip_prefix("sc-domain:")?;
        (host == d || host.ends_with(&format!(".{d}"))).then(|| p.site_url.clone())
    });
    domain.or_else(|| {
        usable
            .filter(|p| {
                origin.as_str().starts_with(p.site_url.as_str())
                    || format!("{}/", origin.as_str().trim_end_matches('/'))
                        .starts_with(p.site_url.as_str())
            })
            .max_by_key(|p| p.site_url.len())
            .map(|p| p.site_url.clone())
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub keys: Vec<String>,
    pub clicks: f64,
    pub impressions: f64,
    pub ctr: f64,
    pub position: f64,
}

/// Every row of a Search Analytics query, paging with `startRow`.
pub fn analytics(
    http: &dyn Http,
    token: &str,
    property: &str,
    start: Date,
    end: Date,
    dimensions: &[&str],
    max_rows: u64,
) -> Result<Vec<Row>> {
    let auth = format!("Bearer {token}");
    let url = format!(
        "{API}/webmasters/v3/sites/{}/searchAnalytics/query",
        enc(property)
    );
    let mut out = Vec::new();
    loop {
        let limit = PAGE_ROWS.min(max_rows - out.len() as u64);
        let body = json!({
            "startDate": start.to_string(),
            "endDate": end.to_string(),
            "dimensions": dimensions,
            "type": "web",
            "rowLimit": limit,
            "startRow": out.len(),
            "dataState": "final",
        });
        let r = http.post_json(&url, &[("Authorization", &auth)], &body)?;
        let v = check(&r, "Search Analytics query")?;
        let rows: Vec<Row> = v["rows"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| Row {
                keys: r["keys"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|k| k.as_str().map(String::from))
                    .collect(),
                clicks: r["clicks"].as_f64().unwrap_or(0.0),
                impressions: r["impressions"].as_f64().unwrap_or(0.0),
                ctr: r["ctr"].as_f64().unwrap_or(0.0),
                position: r["position"].as_f64().unwrap_or(0.0),
            })
            .collect();
        let n = rows.len() as u64;
        out.extend(rows);
        if n < limit || out.len() as u64 >= max_rows {
            return Ok(out);
        }
    }
}

pub fn inspect(http: &dyn Http, token: &str, property: &str, url: &str) -> Result<Inspection> {
    let auth = format!("Bearer {token}");
    let r = http.post_json(
        &format!("{API}/v1/urlInspection/index:inspect"),
        &[("Authorization", &auth)],
        &json!({ "inspectionUrl": url, "siteUrl": property, "languageCode": "en-US" }),
    )?;
    let v = check(&r, &format!("inspecting {url}"))?;
    let s = &v["inspectionResult"]["indexStatusResult"];
    let field = |k: &str| s[k].as_str().map(String::from);
    Ok(Inspection {
        url: url.to_string(),
        inspected_at: OffsetDateTime::now_utc().unix_timestamp(),
        verdict: field("verdict"),
        coverage_state: field("coverageState"),
        indexing_state: field("indexingState"),
        robots_txt_state: field("robotsTxtState"),
        page_fetch_state: field("pageFetchState"),
        last_crawl_time: field("lastCrawlTime"),
        google_canonical: field("googleCanonical"),
        user_canonical: field("userCanonical"),
        crawled_as: field("crawledAs"),
    })
}

#[derive(Debug, Default)]
pub struct InspectRun {
    pub inspected: Vec<Inspection>,
    /// left for another day (daily quota)
    pub deferred: usize,
    pub errors: Vec<String>,
}

/// Inspect up to the day's remaining quota, stalest first; results go to the store.
#[allow(clippy::too_many_arguments)]
pub fn inspect_many(
    http: &dyn Http,
    token: &str,
    property: &str,
    urls: &[String],
    store: &SearchStore,
    today: &str,
    limit: Option<usize>,
    pace: bool,
) -> Result<InspectRun> {
    let used = store.quota_used(today, property, "url_inspection")?;
    let left = INSPECT_DAILY.saturating_sub(used) as usize;
    let last = store.inspected_at(property)?;
    let mut order: Vec<&String> = urls.iter().collect();
    order.sort_by_key(|u| last.get(*u).copied().unwrap_or(0));
    let budget = left.min(limit.unwrap_or(usize::MAX));
    let mut run = InspectRun {
        deferred: order.len().saturating_sub(budget),
        ..Default::default()
    };
    let mut window = Instant::now();
    let mut in_window = 0u32;
    for url in order.into_iter().take(budget) {
        if pace && in_window >= INSPECT_PER_MINUTE {
            let wait = Duration::from_secs(60).saturating_sub(window.elapsed());
            std::thread::sleep(wait);
            window = Instant::now();
            in_window = 0;
        }
        in_window += 1;
        store.spend(today, property, "url_inspection", 1)?;
        match inspect(http, token, property, url) {
            Ok(i) => {
                store.save_inspection(property, &i)?;
                run.inspected.push(i);
            }
            Err(e) => {
                let quota = e.to_string().contains("quota");
                run.errors.push(e.to_string());
                if quota {
                    break;
                }
            }
        }
    }
    Ok(run)
}

/// Store Search Analytics rows: `(date, page)` per day, queries for the window.
pub fn save_pages(store: &SearchStore, property: &str, rows: &[Row]) -> Result<()> {
    let tx = store.conn.unchecked_transaction()?;
    for r in rows {
        let (Some(date), Some(page)) = (r.keys.first(), r.keys.get(1)) else {
            continue;
        };
        tx.execute(
            "INSERT OR REPLACE INTO gsc_page_daily (property, date, page, clicks, impressions, ctr, position)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![property, date, page, r.clicks, r.impressions, r.ctr, r.position],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn save_queries(
    store: &SearchStore,
    property: &str,
    start: Date,
    end: Date,
    rows: &[Row],
) -> Result<()> {
    let tx = store.conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM gsc_queries WHERE property = ?1 AND start = ?2 AND end = ?3",
        rusqlite::params![property, start.to_string(), end.to_string()],
    )?;
    for r in rows {
        let Some(q) = r.keys.first() else { continue };
        tx.execute(
            "INSERT OR REPLACE INTO gsc_queries (property, start, end, query, clicks, impressions, ctr, position)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![property, start.to_string(), end.to_string(), q, r.clicks, r.impressions, r.ctr, r.position],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Clicks per page in `[start, end]` from the stored daily rows.
pub fn page_clicks(
    store: &SearchStore,
    property: &str,
    start: Date,
    end: Date,
) -> Result<std::collections::HashMap<String, f64>> {
    let mut stmt = store.conn.prepare(
        "SELECT page, SUM(clicks) FROM gsc_page_daily WHERE property = ?1 AND date BETWEEN ?2 AND ?3 GROUP BY page",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![property, start.to_string(), end.to_string()],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?)),
    )?;
    rows.collect::<Result<_, _>>().context("reading clicks")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::fake::Fake;
    use time::macros::date;

    #[test]
    fn picks_the_property() {
        let props = vec![
            Property {
                site_url: "https://example.com/".into(),
                permission: "siteOwner".into(),
            },
            Property {
                site_url: "https://example.com/docs/".into(),
                permission: "siteFullUser".into(),
            },
            Property {
                site_url: "sc-domain:other.org".into(),
                permission: "siteOwner".into(),
            },
        ];
        let u = |s: &str| url::Url::parse(s).unwrap();
        assert_eq!(
            property_for(&props, &u("https://example.com")).as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(
            property_for(&props, &u("https://docs.other.org")).as_deref(),
            Some("sc-domain:other.org")
        );
        assert_eq!(property_for(&props, &u("https://nope.net")), None);
    }

    #[test]
    fn pages_through_analytics() {
        let page = |n: usize| {
            let rows: Vec<Value> = (0..n)
                .map(|i| json!({"keys": ["2026-10-01", format!("https://e.com/{i}")], "clicks": 1, "impressions": 10, "ctr": 0.1, "position": 3.2}))
                .collect();
            json!({ "rows": rows }).to_string()
        };
        // 25,000 then 2: two calls
        let full = page(25_000);
        let tail = page(2);
        struct Two(Vec<String>, std::sync::Mutex<usize>);
        impl Http for Two {
            fn send(
                &self,
                _: &str,
                _: &str,
                _: &[(&str, &str)],
                body: Option<(&str, Vec<u8>)>,
            ) -> Result<crate::http::Response> {
                let b: Value = serde_json::from_slice(&body.unwrap().1).unwrap();
                let mut n = self.1.lock().unwrap();
                assert_eq!(b["startRow"], json!(*n * 25_000));
                let r = crate::http::Response {
                    status: 200,
                    body: self.0[*n].clone(),
                };
                *n += 1;
                Ok(r)
            }
        }
        let http = Two(vec![full, tail], std::sync::Mutex::new(0));
        let rows = analytics(
            &http,
            "t",
            "sc-domain:e.com",
            date!(2026 - 10 - 01),
            date!(2026 - 10 - 28),
            &["date", "page"],
            1_000_000,
        )
        .unwrap();
        assert_eq!(rows.len(), 25_002);
        let store = SearchStore::in_memory().unwrap();
        save_pages(&store, "sc-domain:e.com", &rows).unwrap();
        let clicks = page_clicks(
            &store,
            "sc-domain:e.com",
            date!(2026 - 10 - 01),
            date!(2026 - 10 - 01),
        )
        .unwrap();
        assert_eq!(clicks["https://e.com/0"], 1.0, "one row per date and page");
    }

    #[test]
    fn inspects_within_the_daily_quota() {
        let http = Fake::new(&[(
            "POST",
            "https://searchconsole.googleapis.com/v1/urlInspection/index:inspect",
            200,
            r#"{"inspectionResult":{"inspectionResultLink":"x","indexStatusResult":{"verdict":"NEUTRAL","coverageState":"Crawled - currently not indexed","robotsTxtState":"ALLOWED","indexingState":"INDEXING_ALLOWED","lastCrawlTime":"2026-10-01T10:00:00Z","pageFetchState":"SUCCESSFUL","googleCanonical":"https://e.com/a","userCanonical":"https://e.com/a","crawledAs":"MOBILE"}}}"#,
        )]);
        let store = SearchStore::in_memory().unwrap();
        store
            .spend("2026-10-22", "p", "url_inspection", INSPECT_DAILY - 2)
            .unwrap();
        let urls: Vec<String> = (0..5).map(|i| format!("https://e.com/{i}")).collect();
        let run = inspect_many(&http, "t", "p", &urls, &store, "2026-10-22", None, false).unwrap();
        assert_eq!((run.inspected.len(), run.deferred), (2, 3));
        assert_eq!(
            run.inspected[0].coverage_state.as_deref(),
            Some("Crawled - currently not indexed")
        );
        assert_eq!(
            store
                .quota_used("2026-10-22", "p", "url_inspection")
                .unwrap(),
            INSPECT_DAILY
        );
        // tomorrow, the URLs never inspected go first
        let run2 =
            inspect_many(&http, "t", "p", &urls, &store, "2026-10-23", Some(1), false).unwrap();
        assert!(!["https://e.com/0", "https://e.com/1"].contains(&run2.inspected[0].url.as_str()));
    }
}
