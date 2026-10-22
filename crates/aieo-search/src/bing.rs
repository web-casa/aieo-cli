//! Bing Webmaster Tools API with the user's API key (Settings → API Access):
//! `GET https://ssl.bing.com/webmaster/api.svc/json/<Method>?siteUrl=…&apikey=…`,
//! answers wrapped in `{"d": …}`, dates as `/Date(<ms><±hhmm>)/`. Query and
//! page stats are weekly snapshots; traffic stats are daily.

use anyhow::{bail, Result};
use serde_json::Value;
use time::{Date, OffsetDateTime};

use crate::http::{snippet, Http};
use crate::store::SearchStore;

pub const API: &str = "https://ssl.bing.com/webmaster/api.svc/json";

/// `/Date(1316156400000-0700)/` → the calendar date at that offset.
pub fn parse_date(s: &str) -> Option<Date> {
    let inner = s.strip_prefix("/Date(")?.strip_suffix(")/")?;
    let split = inner[1..]
        .find(['+', '-'])
        .map(|i| i + 1)
        .unwrap_or(inner.len());
    let ms: i64 = inner[..split].parse().ok()?;
    let mut t = OffsetDateTime::from_unix_timestamp(ms.div_euclid(1000)).ok()?;
    if let Some(zone) = inner.get(split..).filter(|z| z.len() == 5) {
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let h: i8 = zone[1..3].parse().ok()?;
        let m: i8 = zone[3..5].parse().ok()?;
        t = t.to_offset(time::UtcOffset::from_hms(sign * h, sign * m, 0).ok()?);
    }
    Some(t.date())
}

fn call(http: &dyn Http, key: &str, method: &str, site: Option<&str>) -> Result<Value> {
    let mut url = url::Url::parse(&format!("{API}/{method}"))?;
    {
        let mut q = url.query_pairs_mut();
        if let Some(s) = site {
            q.append_pair("siteUrl", s);
        }
        q.append_pair("apikey", key);
    }
    let r = http.get(url.as_str(), &[])?;
    if !r.ok() {
        let msg = r
            .json()
            .ok()
            .and_then(|v| v["Message"].as_str().map(String::from))
            .unwrap_or_else(|| snippet(&r.body));
        bail!("Bing Webmaster {method}: HTTP {}: {msg}", r.status);
    }
    Ok(r.json()?["d"].clone())
}

pub fn sites(http: &dyn Http, key: &str) -> Result<Vec<(String, bool)>> {
    Ok(call(http, key, "GetUserSites", None)?
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some((
                s["Url"].as_str()?.to_string(),
                s["IsVerified"].as_bool().unwrap_or(false),
            ))
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stat {
    pub date: Date,
    /// the query, or the page URL for page stats
    pub key: String,
    pub clicks: u64,
    pub impressions: u64,
    pub position: Option<f64>,
}

fn stats(http: &dyn Http, key: &str, method: &str, site: &str) -> Result<Vec<Stat>> {
    Ok(call(http, key, method, Some(site))?
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(Stat {
                date: parse_date(s["Date"].as_str()?)?,
                key: s["Query"].as_str()?.to_string(),
                clicks: s["Clicks"].as_u64().unwrap_or(0),
                impressions: s["Impressions"].as_u64().unwrap_or(0),
                position: s["AvgImpressionPosition"].as_f64(),
            })
        })
        .collect())
}

pub fn query_stats(http: &dyn Http, key: &str, site: &str) -> Result<Vec<Stat>> {
    stats(http, key, "GetQueryStats", site)
}

pub fn page_stats(http: &dyn Http, key: &str, site: &str) -> Result<Vec<Stat>> {
    stats(http, key, "GetPageStats", site)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawlIssue {
    pub url: String,
    pub http_code: u16,
    pub issues: u64,
}

pub fn crawl_issues(http: &dyn Http, key: &str, site: &str) -> Result<Vec<CrawlIssue>> {
    Ok(call(http, key, "GetCrawlIssues", Some(site))?
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(CrawlIssue {
                url: s["Url"].as_str()?.to_string(),
                http_code: s["HttpCode"].as_u64().unwrap_or(0) as u16,
                issues: s["Issues"].as_u64().unwrap_or(0),
            })
        })
        .collect())
}

pub fn save(store: &SearchStore, site: &str, pages: &[Stat], queries: &[Stat]) -> Result<()> {
    let tx = store.conn.unchecked_transaction()?;
    for p in pages {
        tx.execute(
            "INSERT OR REPLACE INTO bing_page_daily (site, date, page, clicks, impressions) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![site, p.date.to_string(), p.key, p.clicks as i64, p.impressions as i64],
        )?;
    }
    for q in queries {
        tx.execute(
            "INSERT OR REPLACE INTO bing_query_daily (site, date, query, clicks, impressions, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![site, q.date.to_string(), q.key, q.clicks as i64, q.impressions as i64, q.position],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn save_ai(
    store: &SearchStore,
    site: &str,
    days: &[crate::bing_ai::OverviewDay],
) -> Result<()> {
    let tx = store.conn.unchecked_transaction()?;
    for d in days {
        tx.execute(
            "INSERT OR REPLACE INTO bing_ai_daily (site, date, citations, cited_pages) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![site, d.day.to_string(), d.citations as i64, d.cited_pages as i64],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::fake::Fake;
    use time::macros::date;

    #[test]
    fn parses_wcf_dates() {
        assert_eq!(
            parse_date("/Date(1316156400000-0700)/"),
            Some(date!(2011 - 09 - 16))
        );
        assert_eq!(
            parse_date("/Date(1316156400000)/"),
            Some(date!(2011 - 09 - 16))
        );
        assert_eq!(parse_date("2011-09-16"), None);
    }

    #[test]
    fn reads_stats_and_errors() {
        let http = Fake::new(&[
            (
                "GET",
                "https://ssl.bing.com/webmaster/api.svc/json/GetPageStats",
                200,
                r#"{"d":[{"__type":"QueryStats:#Microsoft.Bing.Webmaster.Api","AvgClickPosition":3,"AvgImpressionPosition":5.5,"Clicks":12,"Date":"/Date(1792800000000-0700)/","Impressions":340,"Query":"https://e.com/a"}]}"#,
            ),
            (
                "GET",
                "https://ssl.bing.com/webmaster/api.svc/json/GetQueryStats",
                400,
                r#"{"ErrorCode":3,"Message":"InvalidApiKey"}"#,
            ),
        ]);
        let pages = page_stats(&http, "k", "https://e.com/").unwrap();
        assert_eq!(
            (pages[0].key.as_str(), pages[0].clicks, pages[0].position),
            ("https://e.com/a", 12, Some(5.5))
        );
        let seen = http.seen.lock().unwrap()[0].1.clone();
        assert!(seen.contains("siteUrl=https%3A%2F%2Fe.com%2F") && seen.contains("apikey=k"));
        drop(seen);
        let e = query_stats(&http, "k", "https://e.com/")
            .unwrap_err()
            .to_string();
        assert!(e.contains("InvalidApiKey"), "{e}");
        let store = SearchStore::in_memory().unwrap();
        save(&store, "https://e.com/", &pages, &[]).unwrap();
    }
}
