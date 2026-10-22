//! `.aieo/search.db` and `.aieo/audit.jsonl`.
//!
//! Search Console and Bing numbers pulled to this machine, the latest URL
//! Inspection result per URL, a quota ledger (calls per day and API, so a
//! run never spends more than the property's daily allowance), and IndexNow
//! submissions. The audit log records every request that changes something
//! outside this machine.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use time::OffsetDateTime;

pub struct SearchStore {
    pub conn: Connection,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inspection {
    pub url: String,
    pub inspected_at: i64,
    pub verdict: Option<String>,
    pub coverage_state: Option<String>,
    pub indexing_state: Option<String>,
    pub robots_txt_state: Option<String>,
    pub page_fetch_state: Option<String>,
    pub last_crawl_time: Option<String>,
    pub google_canonical: Option<String>,
    pub user_canonical: Option<String>,
    pub crawled_as: Option<String>,
}

impl SearchStore {
    pub fn open(path: &Path) -> Result<SearchStore> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        SearchStore::init(
            Connection::open(path).with_context(|| format!("opening {}", path.display()))?,
        )
    }

    pub fn in_memory() -> Result<SearchStore> {
        SearchStore::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<SearchStore> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS quota (day TEXT NOT NULL, scope TEXT NOT NULL, api TEXT NOT NULL,
               count INTEGER NOT NULL, PRIMARY KEY (day, scope, api));
             CREATE TABLE IF NOT EXISTS gsc_page_daily (property TEXT NOT NULL, date TEXT NOT NULL,
               page TEXT NOT NULL, clicks REAL NOT NULL, impressions REAL NOT NULL, ctr REAL NOT NULL,
               position REAL NOT NULL, PRIMARY KEY (property, date, page));
             CREATE TABLE IF NOT EXISTS gsc_queries (property TEXT NOT NULL, start TEXT NOT NULL,
               end TEXT NOT NULL, query TEXT NOT NULL, clicks REAL NOT NULL, impressions REAL NOT NULL,
               ctr REAL NOT NULL, position REAL NOT NULL, PRIMARY KEY (property, start, end, query));
             CREATE TABLE IF NOT EXISTS inspections (property TEXT NOT NULL, url TEXT NOT NULL,
               inspected_at INTEGER NOT NULL, verdict TEXT, coverage_state TEXT, indexing_state TEXT,
               robots_txt_state TEXT, page_fetch_state TEXT, last_crawl_time TEXT,
               google_canonical TEXT, user_canonical TEXT, crawled_as TEXT,
               PRIMARY KEY (property, url));
             CREATE TABLE IF NOT EXISTS bing_page_daily (site TEXT NOT NULL, date TEXT NOT NULL,
               page TEXT NOT NULL, clicks INTEGER NOT NULL, impressions INTEGER NOT NULL,
               PRIMARY KEY (site, date, page));
             CREATE TABLE IF NOT EXISTS bing_query_daily (site TEXT NOT NULL, date TEXT NOT NULL,
               query TEXT NOT NULL, clicks INTEGER NOT NULL, impressions INTEGER NOT NULL,
               position REAL, PRIMARY KEY (site, date, query));
             CREATE TABLE IF NOT EXISTS bing_ai_daily (site TEXT NOT NULL, date TEXT NOT NULL,
               citations INTEGER NOT NULL, cited_pages INTEGER NOT NULL, PRIMARY KEY (site, date));
             CREATE TABLE IF NOT EXISTS indexnow (at INTEGER NOT NULL, endpoint TEXT NOT NULL,
               host TEXT NOT NULL, urls INTEGER NOT NULL, status INTEGER NOT NULL, meaning TEXT NOT NULL);",
        )?;
        Ok(SearchStore { conn })
    }

    pub fn quota_used(&self, day: &str, scope: &str, api: &str) -> Result<u64> {
        Ok(self
            .conn
            .query_row(
                "SELECT count FROM quota WHERE day = ?1 AND scope = ?2 AND api = ?3",
                params![day, scope, api],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0) as u64)
    }

    pub fn spend(&self, day: &str, scope: &str, api: &str, n: u64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO quota (day, scope, api, count) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (day, scope, api) DO UPDATE SET count = count + excluded.count",
            params![day, scope, api, n as i64],
        )?;
        Ok(())
    }

    pub fn save_inspection(&self, property: &str, i: &Inspection) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO inspections (property, url, inspected_at, verdict, coverage_state,
               indexing_state, robots_txt_state, page_fetch_state, last_crawl_time, google_canonical,
               user_canonical, crawled_as)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                property,
                i.url,
                i.inspected_at,
                i.verdict,
                i.coverage_state,
                i.indexing_state,
                i.robots_txt_state,
                i.page_fetch_state,
                i.last_crawl_time,
                i.google_canonical,
                i.user_canonical,
                i.crawled_as
            ],
        )?;
        Ok(())
    }

    pub fn inspections(&self, property: &str) -> Result<Vec<Inspection>> {
        let mut stmt = self.conn.prepare(
            "SELECT url, inspected_at, verdict, coverage_state, indexing_state, robots_txt_state,
               page_fetch_state, last_crawl_time, google_canonical, user_canonical, crawled_as
             FROM inspections WHERE property = ?1 ORDER BY url",
        )?;
        let rows = stmt.query_map([property], |r| {
            Ok(Inspection {
                url: r.get(0)?,
                inspected_at: r.get(1)?,
                verdict: r.get(2)?,
                coverage_state: r.get(3)?,
                indexing_state: r.get(4)?,
                robots_txt_state: r.get(5)?,
                page_fetch_state: r.get(6)?,
                last_crawl_time: r.get(7)?,
                google_canonical: r.get(8)?,
                user_canonical: r.get(9)?,
                crawled_as: r.get(10)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn record_indexnow(
        &self,
        endpoint: &str,
        host: &str,
        urls: usize,
        status: u16,
        meaning: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO indexnow (at, endpoint, host, urls, status, meaning)
             VALUES (strftime('%s','now'), ?1, ?2, ?3, ?4, ?5)",
            params![endpoint, host, urls as i64, status, meaning],
        )?;
        Ok(())
    }

    /// When each URL was last inspected (to inspect the stalest first).
    pub fn inspected_at(&self, property: &str) -> Result<std::collections::HashMap<String, i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT url, inspected_at FROM inspections WHERE property = ?1")?;
        let rows = stmt.query_map([property], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[derive(Debug, Serialize)]
pub struct AuditEntry<'a> {
    pub at: String,
    pub action: &'a str,
    pub target: &'a str,
    pub items: u64,
    pub status: Option<u16>,
    pub detail: &'a str,
}

/// Append one line to `.aieo/audit.jsonl`.
pub fn audit(
    dir: &Path,
    action: &str,
    target: &str,
    items: u64,
    status: Option<u16>,
    detail: &str,
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let entry = AuditEntry {
        at: OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        action,
        target,
        items,
        status,
        detail,
    };
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("audit.jsonl"))?;
    writeln!(f, "{}", serde_json::to_string(&entry)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_and_inspections() {
        let s = SearchStore::in_memory().unwrap();
        assert_eq!(
            s.quota_used("2026-10-22", "sc-domain:e.com", "inspect")
                .unwrap(),
            0
        );
        s.spend("2026-10-22", "sc-domain:e.com", "inspect", 3)
            .unwrap();
        s.spend("2026-10-22", "sc-domain:e.com", "inspect", 2)
            .unwrap();
        assert_eq!(
            s.quota_used("2026-10-22", "sc-domain:e.com", "inspect")
                .unwrap(),
            5
        );
        let i = Inspection {
            url: "https://e.com/a".into(),
            inspected_at: 1,
            verdict: Some("PASS".into()),
            ..Default::default()
        };
        s.save_inspection("p", &i).unwrap();
        s.save_inspection(
            "p",
            &Inspection {
                inspected_at: 2,
                ..i.clone()
            },
        )
        .unwrap();
        assert_eq!(s.inspections("p").unwrap().len(), 1);
        assert_eq!(s.inspected_at("p").unwrap()["https://e.com/a"], 2);
        let dir = tempfile::tempdir().unwrap();
        audit(
            dir.path(),
            "indexnow.submit",
            "api.indexnow.org",
            3,
            Some(202),
            "",
        )
        .unwrap();
        audit(
            dir.path(),
            "indexnow.submit",
            "api.indexnow.org",
            1,
            Some(200),
            "",
        )
        .unwrap();
        let text = std::fs::read_to_string(dir.path().join("audit.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 2);
    }
}
