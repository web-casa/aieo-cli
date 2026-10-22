//! Reading log files into the store.
//!
//! A file is identified by the hash of its first 4 KiB (decompressed), so a
//! log that grew resumes at the byte where the last run stopped, and a
//! rotated copy (`access.log.1`, `.gz`) of a file already read is
//! recognised as the same source. Lines are parsed and classified in
//! batches; crawler claims are verified per distinct (crawler, IP) in
//! parallel. Raw IPs never leave this module: rows carry a salted hash.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use flate2::read::MultiGzDecoder;
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::classify::{ai_platform, classify, Actor};
use crate::hit::{status_class, Hit};
use crate::parse::{Format, Line, LineParser};
use crate::store::{Counts, SourceRow, Store, StoredHit};
use crate::verify::{Verdict, Verifier};

const IDENTITY_BYTES: usize = 4096;
const BATCH: usize = 20_000;

/// The per-project secret IPs are hashed with (`.aieo/salt`, never committed).
pub fn load_or_create_salt(dir: &Path) -> Result<[u8; 32]> {
    let path = dir.join("salt");
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(salt) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(salt);
        }
    }
    let mut salt = [0u8; 32];
    getrandom::fill(&mut salt)
        .map_err(|e| anyhow::anyhow!("no randomness for the IP salt: {e}"))?;
    std::fs::create_dir_all(dir)?;
    std::fs::write(&path, salt).with_context(|| format!("writing {}", path.display()))?;
    keep_out_of_git(dir)?;
    Ok(salt)
}

/// `.aieo/.gitignore` for what must stay local (salt, logs.db, caches).
pub fn keep_out_of_git(dir: &Path) -> Result<()> {
    let path = dir.join(".gitignore");
    let want = ["salt", "logs.db*", "cache/"];
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<&str> = want
        .iter()
        .copied()
        .filter(|w| !current.lines().any(|l| l.trim() == *w))
        .collect();
    if !missing.is_empty() {
        let mut text = current;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        if text.is_empty() {
            text.push_str("# local only: the IP salt, crawler logs, downloaded IP ranges\n");
        }
        for m in missing {
            text.push_str(m);
            text.push('\n');
        }
        std::fs::write(&path, text)?;
    }
    Ok(())
}

pub fn ip_hash(salt: &[u8; 32], ip: IpAddr) -> String {
    let mut h = Sha256::new();
    h.update(salt);
    h.update(ip.to_string().as_bytes());
    h.finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn hex16(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub struct Options<'a> {
    /// None = detect from the first lines
    pub format: Option<Format>,
    /// keep only requests for this host (requests without a host are kept)
    pub host: Option<String>,
    pub verifier: &'a Verifier,
    pub salt: [u8; 32],
}

#[derive(Debug, Clone, Default)]
pub struct FileReport {
    pub path: PathBuf,
    pub format: String,
    /// lines read in this run
    pub lines: u64,
    /// requests parsed (any actor)
    pub requests: u64,
    pub stored: u64,
    pub duplicates: u64,
    pub humans: u64,
    pub other_hosts: u64,
    pub ignored: u64,
    pub malformed: u64,
    /// byte offset the run started at (0 = from the beginning)
    pub resumed_at: u64,
    /// nothing new since the last run
    pub up_to_date: bool,
    /// first malformed lines' numbers, for the message
    pub malformed_examples: Vec<u64>,
}

fn open(path: &Path) -> Result<Box<dyn Read>> {
    let mut f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut magic = [0u8; 2];
    let n = f.read(&mut magic)?;
    let head = std::io::Cursor::new(magic[..n].to_vec());
    let chained = head.chain(f);
    Ok(if n == 2 && magic == [0x1f, 0x8b] {
        Box::new(MultiGzDecoder::new(chained))
    } else {
        Box::new(chained)
    })
}

struct Batch<'a> {
    opts: &'a Options<'a>,
    hits: Vec<(Hit, Option<String>, String)>,
    report: &'a mut FileReport,
    source: SourceRow,
}

impl Batch<'_> {
    fn flush(&mut self, store: &mut Store) -> Result<()> {
        let hits = std::mem::take(&mut self.hits);
        let classes: Vec<_> = hits.iter().map(|(h, _, _)| classify(&h.ua)).collect();
        // verify each distinct (crawler, ip) once, in parallel
        let mut claims: Vec<(&'static aieo_core::spec::CrawlerSpec, Option<IpAddr>)> = Vec::new();
        for ((h, _, _), c) in hits.iter().zip(&classes) {
            if let Some(crawler) = c.crawler {
                if !claims
                    .iter()
                    .any(|(k, ip)| k.id == crawler.id && *ip == h.ip)
                {
                    claims.push((crawler, h.ip));
                }
            }
        }
        let verdicts: HashMap<(String, Option<IpAddr>), (Verdict, Option<String>)> = claims
            .par_iter()
            .map(|(c, ip)| ((c.id.clone(), *ip), self.opts.verifier.verify(c, *ip)))
            .collect();

        let mut rows = Vec::new();
        let mut humans: HashMap<(String, String, String), Counts> = HashMap::new();
        for ((hit, id, raw), class) in hits.into_iter().zip(classes) {
            let day = hit.ts.date().to_string();
            let platform = (class.actor == Actor::Human)
                .then(|| ai_platform(hit.referrer.as_deref(), hit.query.as_deref()))
                .flatten();
            self.source.first_ts = Some(
                self.source
                    .first_ts
                    .map_or(hit.ts.unix_timestamp(), |t| t.min(hit.ts.unix_timestamp())),
            );
            self.source.last_ts = Some(
                self.source
                    .last_ts
                    .map_or(hit.ts.unix_timestamp(), |t| t.max(hit.ts.unix_timestamp())),
            );
            if class.actor == Actor::Human && platform.is_none() {
                self.report.humans += 1;
                let c = humans
                    .entry((day, hit.host.clone().unwrap_or_default(), hit.path.clone()))
                    .or_default();
                c.hits += 1;
                c.by_class[status_class(hit.status) - 2] += 1;
                continue;
            }
            let (verdict, list_version) = match class.crawler {
                Some(c) => {
                    let (v, ver) = verdicts[&(c.id.clone(), hit.ip)].clone();
                    (Some(v.as_str().to_string()), ver)
                }
                None => (None, None),
            };
            let human = class.actor == Actor::Human;
            rows.push(StoredHit {
                hit_id: match id {
                    Some(id) => hex16(format!("{}\u{0}{id}", self.source.format).as_bytes()),
                    None => hex16(raw.as_bytes()),
                },
                ts: hit.ts.unix_timestamp(),
                day,
                host: hit.host,
                path: hit.path,
                status: hit.status,
                bytes: hit.bytes,
                actor: class.actor.as_str().to_string(),
                bot: class.crawler.map(|c| c.id.clone()),
                verdict,
                list_version,
                // people are counted, never tracked
                ip_hash: if human {
                    None
                } else {
                    hit.ip.map(|ip| ip_hash(&self.opts.salt, ip))
                },
                ua: if human {
                    None
                } else {
                    Some(hit.ua.chars().take(300).collect())
                },
                platform: platform.map(|p| p.id.clone()),
                cache: hit.cache,
                weight: 1,
            });
        }
        let new = store.commit(&rows, &humans, &self.source)?;
        self.report.stored += new;
        self.report.duplicates += rows.len() as u64 - new;
        Ok(())
    }
}

pub fn ingest_file(store: &mut Store, path: &Path, opts: &Options) -> Result<FileReport> {
    let mut reader = BufReader::with_capacity(1 << 20, open(path)?);
    // identity: hash of the first 4 KiB (kept to replay)
    let mut prefix = Vec::with_capacity(IDENTITY_BYTES);
    (&mut reader)
        .take(IDENTITY_BYTES as u64)
        .read_to_end(&mut prefix)?;
    if prefix.is_empty() {
        return Ok(FileReport {
            path: path.to_path_buf(),
            up_to_date: true,
            ..Default::default()
        });
    }
    let identity = hex16(&prefix);
    let known = store.source(&identity)?;
    let mut report = FileReport {
        path: path.to_path_buf(),
        ..Default::default()
    };
    let format = match (&opts.format, &known) {
        (Some(f), _) => f.clone(),
        (None, Some(k)) => Format::from_arg(&k.format)
            .map_err(anyhow::Error::msg)?
            .context("stored format")?,
        (None, None) => {
            let text = String::from_utf8_lossy(&prefix);
            let mut lines: Vec<&str> = text.lines().collect();
            if prefix.len() == IDENTITY_BYTES && lines.len() > 1 {
                lines.pop(); // possibly cut
            }
            match Format::detect(lines) {
                Some(f) => f,
                None => bail!(
                    "{}: unknown log format; pass --format (combined, caddy, cloudflare, vercel, cloudfront, nginx:'…', apache:'…')",
                    path.display()
                ),
            }
        }
    };
    report.format = format.name();
    let mut source = known.clone().unwrap_or(SourceRow {
        identity: identity.clone(),
        path: path.display().to_string(),
        format: format.name(),
        ..Default::default()
    });
    source.path = path.display().to_string();
    let mut stream = std::io::Cursor::new(prefix).chain(reader);
    let mut consumed = 0u64;
    if let Some(k) = &known {
        // skip what an earlier run read
        let skipped = std::io::copy(&mut (&mut stream).take(k.offset), &mut std::io::sink())?;
        consumed = skipped;
        report.resumed_at = k.offset;
        if skipped < k.offset {
            report.up_to_date = true;
            return Ok(report);
        }
    }
    let mut buffered = BufReader::new(stream);
    let mut parser = LineParser::new(format);
    if known.is_some() && matches!(parser.format(), Format::CloudFront) {
        // the #Fields header was before the resume point: read it again
        let mut again = BufReader::new(open(path)?);
        let mut line = String::new();
        while again.read_line(&mut line)? > 0 && line.starts_with('#') {
            parser.parse(&line);
            line.clear();
        }
    }
    let host = opts.host.as_ref().map(|h| h.to_ascii_lowercase());
    let mut batch = Batch {
        opts,
        hits: Vec::with_capacity(BATCH),
        report: &mut report,
        source,
    };
    let mut buf = Vec::new();
    let mut line_no = batch.source.lines;
    let skipped_before = batch.source.skipped;
    loop {
        buf.clear();
        let n = buffered.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }
        let line = String::from_utf8_lossy(&buf).into_owned();
        let parsed = parser.parse(&line);
        // an unterminated last line may still be being written: leave it
        // for the next run unless it is already a complete request
        if !buf.ends_with(b"\n") && !matches!(parsed, Line::Hit(_)) {
            break;
        }
        consumed += n as u64;
        line_no += 1;
        batch.report.lines += 1;
        match parsed {
            Line::Hit(p) => {
                let (hit, id) = *p;
                batch.report.requests += 1;
                let other_host = match (&host, &hit.host) {
                    (Some(want), Some(h)) => {
                        let h = h.to_ascii_lowercase();
                        h != *want && h.strip_prefix("www.") != Some(want.as_str())
                    }
                    _ => false,
                };
                if other_host {
                    batch.report.other_hosts += 1;
                } else {
                    batch.hits.push((hit, id, line.trim_end().to_string()));
                }
            }
            Line::Ignored => batch.report.ignored += 1,
            Line::Malformed => {
                batch.report.malformed += 1;
                if batch.report.malformed_examples.len() < 3 {
                    batch.report.malformed_examples.push(line_no);
                }
            }
        }
        if batch.hits.len() >= BATCH {
            batch.source.offset = consumed;
            batch.source.lines = line_no;
            batch.source.hits += batch.hits.len() as u64;
            batch.source.skipped = skipped_before + batch.report.malformed;
            batch.flush(store)?;
        }
    }
    batch.source.offset = consumed;
    batch.source.lines = line_no;
    batch.source.hits += batch.hits.len() as u64;
    batch.source.skipped = skipped_before + batch.report.malformed;
    batch.flush(store)?;
    if report.lines == 0 {
        report.up_to_date = true;
    }
    Ok(report)
}

/// Day as `YYYY-MM-DD` (UTC).
pub fn day_of(ts: i64) -> String {
    OffsetDateTime::from_unix_timestamp(ts)
        .map(|t| t.date().to_string())
        .unwrap_or_default()
}
