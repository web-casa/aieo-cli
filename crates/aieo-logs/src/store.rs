//! `.aieo/logs.db`: what ingestion keeps.
//!
//! - `hits`: one row per crawler / automated request and per AI-referred
//!   visit, keyed by `hit_id` so overlapping files (a rotated log ingested
//!   twice) do not double count. IPs only as salted hashes, user agents only
//!   for automated clients. `weight` > 1 for pre-aggregated sources.
//! - `human_daily`: other visits as per-day, per-path counts.
//! - `sources`: each ingested file by the hash of its first bytes, with how
//!   far it was read, so a growing or rotated file resumes where it stopped.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

pub const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct StoredHit {
    pub hit_id: String,
    pub ts: i64,
    pub day: String,
    pub host: Option<String>,
    pub path: String,
    pub status: u16,
    pub bytes: Option<u64>,
    pub actor: String,
    pub bot: Option<String>,
    pub verdict: Option<String>,
    pub list_version: Option<String>,
    pub ip_hash: Option<String>,
    pub ua: Option<String>,
    pub platform: Option<String>,
    pub cache: Option<String>,
    pub weight: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceRow {
    pub identity: String,
    pub path: String,
    pub format: String,
    /// bytes consumed (decompressed), at a line boundary
    pub offset: u64,
    pub lines: u64,
    pub hits: u64,
    pub skipped: u64,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
}

/// Human visits of one day and path: total and per status class.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub hits: u64,
    pub by_class: [u64; 4],
}

pub struct Store {
    pub conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Store::init(conn)
    }

    pub fn in_memory() -> Result<Store> {
        Store::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Store> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS sources (
               identity TEXT PRIMARY KEY, path TEXT NOT NULL, format TEXT NOT NULL,
               offset INTEGER NOT NULL, lines INTEGER NOT NULL, hits INTEGER NOT NULL,
               skipped INTEGER NOT NULL, first_ts INTEGER, last_ts INTEGER, updated_at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS hits (
               hit_id TEXT PRIMARY KEY, ts INTEGER NOT NULL, day TEXT NOT NULL, host TEXT,
               path TEXT NOT NULL, status INTEGER NOT NULL, bytes INTEGER, actor TEXT NOT NULL,
               bot TEXT, verdict TEXT, list_version TEXT, ip_hash TEXT, ua TEXT, platform TEXT,
               cache TEXT, weight INTEGER NOT NULL DEFAULT 1);
             CREATE INDEX IF NOT EXISTS hits_day ON hits (day);
             CREATE INDEX IF NOT EXISTS hits_bot ON hits (bot, path);
             CREATE INDEX IF NOT EXISTS hits_platform ON hits (platform) WHERE platform IS NOT NULL;
             CREATE TABLE IF NOT EXISTS human_daily (
               day TEXT NOT NULL, host TEXT NOT NULL, path TEXT NOT NULL, hits INTEGER NOT NULL,
               s2 INTEGER NOT NULL, s3 INTEGER NOT NULL, s4 INTEGER NOT NULL, s5 INTEGER NOT NULL,
               PRIMARY KEY (day, host, path));",
        )?;
        let version: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |r| {
                r.get(0)
            })
            .optional()?;
        match version {
            None => {
                conn.execute(
                    "INSERT INTO meta (key, value) VALUES ('schema', ?1)",
                    [SCHEMA_VERSION.to_string()],
                )?;
            }
            Some(v) if v == SCHEMA_VERSION.to_string() => {}
            Some(v) => anyhow::bail!(
                "logs.db has schema {v}, this aieo expects {SCHEMA_VERSION}; delete it and ingest again"
            ),
        }
        Ok(Store { conn })
    }

    pub fn source(&self, identity: &str) -> Result<Option<SourceRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT identity, path, format, offset, lines, hits, skipped, first_ts, last_ts
                 FROM sources WHERE identity = ?1",
                [identity],
                |r| {
                    Ok(SourceRow {
                        identity: r.get(0)?,
                        path: r.get(1)?,
                        format: r.get(2)?,
                        offset: r.get::<_, i64>(3)? as u64,
                        lines: r.get::<_, i64>(4)? as u64,
                        hits: r.get::<_, i64>(5)? as u64,
                        skipped: r.get::<_, i64>(6)? as u64,
                        first_ts: r.get(7)?,
                        last_ts: r.get(8)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn sources(&self) -> Result<Vec<SourceRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT identity, path, format, offset, lines, hits, skipped, first_ts, last_ts
             FROM sources ORDER BY path",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SourceRow {
                identity: r.get(0)?,
                path: r.get(1)?,
                format: r.get(2)?,
                offset: r.get::<_, i64>(3)? as u64,
                lines: r.get::<_, i64>(4)? as u64,
                hits: r.get::<_, i64>(5)? as u64,
                skipped: r.get::<_, i64>(6)? as u64,
                first_ts: r.get(7)?,
                last_ts: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Store a batch atomically: hits (duplicates ignored), human counts,
    /// the source's progress. Returns how many hits were new.
    pub fn commit(
        &mut self,
        hits: &[StoredHit],
        humans: &std::collections::HashMap<(String, String, String), Counts>,
        source: &SourceRow,
    ) -> Result<u64> {
        self.write(hits, humans, source, false)
    }

    /// Like [`Store::commit`], but rows with a known `hit_id` are replaced:
    /// aggregated sources (Cloudflare analytics) restate their counts.
    pub fn commit_replacing(
        &mut self,
        hits: &[StoredHit],
        humans: &std::collections::HashMap<(String, String, String), Counts>,
        source: &SourceRow,
    ) -> Result<u64> {
        self.write(hits, humans, source, true)
    }

    fn write(
        &mut self,
        hits: &[StoredHit],
        humans: &std::collections::HashMap<(String, String, String), Counts>,
        source: &SourceRow,
        replace: bool,
    ) -> Result<u64> {
        let tx = self.conn.transaction()?;
        let mut new = 0u64;
        {
            let verb = if replace {
                "INSERT OR REPLACE"
            } else {
                "INSERT OR IGNORE"
            };
            let mut insert = tx.prepare_cached(&format!(
                "{verb} INTO hits (hit_id, ts, day, host, path, status, bytes, actor, bot,
                   verdict, list_version, ip_hash, ua, platform, cache, weight)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"
            ))?;
            for h in hits {
                new += insert.execute(params![
                    h.hit_id,
                    h.ts,
                    h.day,
                    h.host,
                    h.path,
                    h.status,
                    h.bytes.map(|b| b as i64),
                    h.actor,
                    h.bot,
                    h.verdict,
                    h.list_version,
                    h.ip_hash,
                    h.ua,
                    h.platform,
                    h.cache,
                    h.weight as i64,
                ])? as u64;
            }
            let mut upsert = tx.prepare_cached(
                "INSERT INTO human_daily (day, host, path, hits, s2, s3, s4, s5)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (day, host, path) DO UPDATE SET hits = hits + excluded.hits,
                   s2 = s2 + excluded.s2, s3 = s3 + excluded.s3, s4 = s4 + excluded.s4,
                   s5 = s5 + excluded.s5",
            )?;
            for ((day, host, path), c) in humans {
                upsert.execute(params![
                    day,
                    host,
                    path,
                    c.hits as i64,
                    c.by_class[0] as i64,
                    c.by_class[1] as i64,
                    c.by_class[2] as i64,
                    c.by_class[3] as i64,
                ])?;
            }
            tx.execute(
                "INSERT INTO sources (identity, path, format, offset, lines, hits, skipped, first_ts,
                   last_ts, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, strftime('%s','now'))
                 ON CONFLICT (identity) DO UPDATE SET path = excluded.path, offset = excluded.offset,
                   lines = excluded.lines, hits = excluded.hits, skipped = excluded.skipped,
                   first_ts = excluded.first_ts, last_ts = excluded.last_ts,
                   updated_at = excluded.updated_at",
                params![
                    source.identity,
                    source.path,
                    source.format,
                    source.offset as i64,
                    source.lines as i64,
                    source.hits as i64,
                    source.skipped as i64,
                    source.first_ts,
                    source.last_ts,
                ],
            )?;
        }
        tx.commit()?;
        Ok(new)
    }

    /// Earliest and latest stored request (hits and human days).
    pub fn span(&self) -> Result<Option<(i64, i64)>> {
        let r: (Option<i64>, Option<i64>) =
            self.conn
                .query_row("SELECT MIN(first_ts), MAX(last_ts) FROM sources", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
        Ok(match r {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        })
    }
}
