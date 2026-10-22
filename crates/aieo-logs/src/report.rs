//! From `.aieo/logs.db` to evidence: per-crawler totals and verification,
//! per-URL daily crawls, AI referrals, coverage of the build's indexable
//! pages, 404 spikes — and the L findings.
//!
//! Requests that fail verification (a crawler's user agent from elsewhere)
//! are reported as spoofed and left out of every crawler aggregate.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::Result;
use rusqlite::params;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use aieo_core::bundle::{
    LogAnomaly, LogBot, LogCoverage, LogReferral, LogSource, LogUrlDay, LogWindow, Logs,
    StatusBuckets, VerificationCounts,
};
use aieo_core::spec::{spec, CrawlerSpec};
use aieo_core::{Evidence, Localized};
use aieo_rules::emit::{Emitter, Loc};
use aieo_site::preset::route_key;
use aieo_site::robots::{evaluate_path, rules_for_agent, PathDecision, RobotsFile};

use crate::store::Store;

/// What the build says about the site, when there is a build to read.
#[derive(Debug, Clone, Default)]
pub struct Inventory {
    /// URL paths of indexable pages (base path included)
    pub indexable: Vec<String>,
    pub robots: Option<RobotsFile>,
    pub base_path: String,
}

pub struct Params<'a> {
    /// days back from the newest request
    pub since_days: u64,
    /// the site's host: requests logged for other hosts are left out
    pub host: Option<&'a str>,
    pub origin: Option<&'a str>,
    pub inventory: Option<&'a Inventory>,
}

#[derive(Debug, Clone)]
pub struct BotStats {
    pub bot: Option<&'static CrawlerSpec>,
    pub actor: String,
    pub hits: u64,
    pub verification: VerificationCounts,
    pub status: StatusBuckets,
    /// 401 / 403 / 429
    pub blocked: u64,
    pub first: i64,
    pub last: i64,
    pub spoofer_ips: u64,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub from: i64,
    pub to: i64,
    pub days: u64,
    pub bots: Vec<BotStats>,
    pub human_visits: u64,
    pub ai_referrals: u64,
    pub logs: Logs,
    /// indexable paths no crawler of each actor class fetched
    pub never_crawled: BTreeMap<String, Vec<String>>,
}

fn ev(en: &str, zh: &str, detail: impl Into<String>) -> Evidence {
    Evidence::new(Localized::new(en, zh), detail.into())
}

fn crawler(id: &str) -> Option<&'static CrawlerSpec> {
    spec().crawlers.iter().find(|c| c.id == id)
}

fn rfc3339(ts: i64) -> String {
    OffsetDateTime::from_unix_timestamp(ts)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default()
}

/// SQL condition for the window and host (params ?1 from, ?2 to, ?3 host).
const SCOPE: &str = "ts BETWEEN ?1 AND ?2 AND (?3 IS NULL OR host IS NULL OR lower(host) = ?3 OR lower(host) = 'www.' || ?3)";

/// Status classes of requests that did not fail verification.
const GENUINE_STATUS_SUMS: &str = "SUM(CASE WHEN status < 300 AND verdict IS NOT 'mismatch' THEN weight ELSE 0 END), \
     SUM(CASE WHEN status BETWEEN 300 AND 399 AND verdict IS NOT 'mismatch' THEN weight ELSE 0 END), \
     SUM(CASE WHEN status BETWEEN 400 AND 499 AND verdict IS NOT 'mismatch' THEN weight ELSE 0 END), \
     SUM(CASE WHEN status >= 500 AND verdict IS NOT 'mismatch' THEN weight ELSE 0 END)";

const STATUS_SUMS: &str = "SUM(CASE WHEN status < 300 THEN weight ELSE 0 END), \
     SUM(CASE WHEN status BETWEEN 300 AND 399 THEN weight ELSE 0 END), \
     SUM(CASE WHEN status BETWEEN 400 AND 499 THEN weight ELSE 0 END), \
     SUM(CASE WHEN status >= 500 THEN weight ELSE 0 END)";

fn buckets(r: &rusqlite::Row, at: usize) -> rusqlite::Result<StatusBuckets> {
    Ok(StatusBuckets {
        s2: r.get::<_, i64>(at)? as u64,
        s3: r.get::<_, i64>(at + 1)? as u64,
        s4: r.get::<_, i64>(at + 2)? as u64,
        s5: r.get::<_, i64>(at + 3)? as u64,
    })
}

fn median(mut v: Vec<u64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_unstable();
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2] as f64
    } else {
        (v[n / 2 - 1] + v[n / 2]) as f64 / 2.0
    }
}

pub fn build(store: &Store, p: &Params) -> Result<Option<Report>> {
    let Some((_, newest)) = store.span()? else {
        return Ok(None);
    };
    let to = newest;
    let from = to - (p.since_days as i64) * 86_400 + 1;
    let host = p.host.map(str::to_ascii_lowercase);
    let c = &store.conn;
    let (first, days) = {
        let (lo, hi): (Option<i64>, Option<i64>) = c.query_row(
            &format!("SELECT MIN(ts), MAX(ts) FROM hits WHERE {SCOPE}"),
            params![from, to, host],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        match (lo, hi) {
            (Some(lo), Some(hi)) => (lo, ((hi - lo) / 86_400 + 1) as u64),
            _ => (from, 0),
        }
    };

    // per crawler (or actor, for automation outside the catalog)
    let mut bots = Vec::new();
    {
        let mut stmt = c.prepare(&format!(
            "SELECT bot, actor, SUM(weight),
               SUM(CASE WHEN verdict = 'verified' THEN weight ELSE 0 END),
               SUM(CASE WHEN verdict = 'mismatch' THEN weight ELSE 0 END),
               SUM(CASE WHEN verdict = 'unverifiable' THEN weight ELSE 0 END),
               SUM(CASE WHEN verdict = 'unchecked' OR verdict IS NULL THEN weight ELSE 0 END),
               {GENUINE_STATUS_SUMS},
               SUM(CASE WHEN status IN (401, 403, 429) AND (verdict IS NULL OR verdict != 'mismatch') THEN weight ELSE 0 END),
               MIN(ts), MAX(ts),
               COUNT(DISTINCT CASE WHEN verdict = 'mismatch' THEN ip_hash END)
             FROM hits WHERE {SCOPE} AND actor != 'human'
             GROUP BY bot, actor ORDER BY SUM(weight) DESC"
        ))?;
        let rows = stmt.query_map(params![from, to, host], |r| {
            Ok(BotStats {
                bot: r.get::<_, Option<String>>(0)?.as_deref().and_then(crawler),
                actor: r.get(1)?,
                hits: r.get::<_, i64>(2)? as u64,
                verification: VerificationCounts {
                    verified: r.get::<_, i64>(3)? as u64,
                    mismatch: r.get::<_, i64>(4)? as u64,
                    unverifiable: r.get::<_, i64>(5)? as u64,
                    unchecked: r.get::<_, i64>(6)? as u64,
                },
                status: buckets(r, 7)?,
                blocked: r.get::<_, i64>(11)? as u64,
                first: r.get(12)?,
                last: r.get(13)?,
                spoofer_ips: r.get::<_, i64>(14)? as u64,
            })
        })?;
        for row in rows {
            bots.push(row?);
        }
    }

    // genuine catalog-crawler requests per URL and day
    let mut per_url = Vec::new();
    {
        let mut stmt = c.prepare(&format!(
            "SELECT path, actor, bot, day, SUM(weight),
               SUM(CASE WHEN verdict = 'verified' THEN weight ELSE 0 END), {STATUS_SUMS}
             FROM hits WHERE {SCOPE} AND bot IS NOT NULL AND verdict != 'mismatch'
             GROUP BY path, actor, bot, day ORDER BY day, path, bot"
        ))?;
        let rows = stmt.query_map(params![from, to, host], |r| {
            Ok(LogUrlDay {
                path: r.get(0)?,
                actor: r.get(1)?,
                bot: r.get(2)?,
                day: r.get(3)?,
                hits: r.get::<_, i64>(4)? as u64,
                verified: r.get::<_, i64>(5)? as u64,
                status: buckets(r, 6)?,
            })
        })?;
        for row in rows {
            per_url.push(row?);
        }
    }

    let mut referrals = Vec::new();
    {
        let mut stmt = c.prepare(&format!(
            "SELECT path, platform, day, SUM(weight) FROM hits
             WHERE {SCOPE} AND platform IS NOT NULL
             GROUP BY path, platform, day ORDER BY day, path"
        ))?;
        let rows = stmt.query_map(params![from, to, host], |r| {
            Ok(LogReferral {
                landing: r.get(0)?,
                platform: r.get(1)?,
                day: r.get(2)?,
                visits: r.get::<_, i64>(3)? as u64,
            })
        })?;
        for row in rows {
            referrals.push(row?);
        }
    }
    let ai_referrals = referrals.iter().map(|r| r.visits).sum();
    let human_visits: u64 = c
        .query_row(
            "SELECT COALESCE(SUM(hits), 0) FROM human_daily WHERE day BETWEEN ?1 AND ?2
               AND (?3 IS NULL OR host = '' OR lower(host) = ?3 OR lower(host) = 'www.' || ?3)",
            params![crate::ingest::day_of(from), crate::ingest::day_of(to), host],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) as u64
        + ai_referrals;

    // coverage of indexable pages
    let mut coverage = Vec::new();
    let mut never_crawled = BTreeMap::new();
    if let Some(inv) = p.inventory {
        let originals: BTreeMap<String, String> = inv
            .indexable
            .iter()
            .map(|u| (route_key(u), u.clone()))
            .collect();
        let indexable: BTreeSet<String> = originals.keys().cloned().collect();
        let mut by_bot: HashMap<(String, Option<String>), BTreeSet<String>> = HashMap::new();
        for row in &per_url {
            let key = route_key(&row.path);
            if indexable.contains(&key) && row.status.s2 + row.status.s3 > 0 {
                by_bot
                    .entry((row.actor.clone(), row.bot.clone()))
                    .or_default()
                    .insert(key.clone());
                by_bot
                    .entry((row.actor.clone(), None))
                    .or_default()
                    .insert(key);
            }
        }
        for actor in ["search_index", "classic_search", "user_fetch", "training"] {
            let crawled = by_bot.get(&(actor.to_string(), None));
            coverage.push(LogCoverage {
                actor: actor.into(),
                bot: None,
                indexable: indexable.len() as u64,
                crawled: crawled.map_or(0, |s| s.len() as u64),
            });
            never_crawled.insert(
                actor.to_string(),
                indexable
                    .iter()
                    .filter(|k| !crawled.is_some_and(|s| s.contains(*k)))
                    .map(|k| originals[k].clone())
                    .collect(),
            );
        }
        let mut per_bot: Vec<_> = by_bot
            .iter()
            .filter(|((_, b), _)| b.is_some())
            .map(|((a, b), s)| LogCoverage {
                actor: a.clone(),
                bot: b.clone(),
                indexable: indexable.len() as u64,
                crawled: s.len() as u64,
            })
            .collect();
        per_bot.sort_by(|a, b| b.crawled.cmp(&a.crawled).then(a.bot.cmp(&b.bot)));
        coverage.extend(per_bot);
    }

    // crawler 404 spikes
    let mut anomalies = Vec::new();
    {
        let mut stmt = c.prepare(&format!(
            "SELECT day, SUM(weight) FROM hits
             WHERE {SCOPE} AND bot IS NOT NULL AND verdict != 'mismatch' AND status = 404
             GROUP BY day"
        ))?;
        let counts: BTreeMap<String, u64> = stmt
            .query_map(params![from, to, host], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
            })?
            .collect::<Result<_, _>>()?;
        if days >= 7 {
            let mut all: Vec<u64> = Vec::new();
            let start = OffsetDateTime::from_unix_timestamp(from)?.date();
            let end = OffsetDateTime::from_unix_timestamp(to)?.date();
            let mut d = start;
            while d <= end {
                all.push(counts.get(&d.to_string()).copied().unwrap_or(0));
                d = d.next_day().expect("date");
            }
            let base = median(all);
            for (day, n) in &counts {
                if *n >= 20 && (*n as f64) >= 3.0 * base.max(1.0) {
                    anomalies.push(LogAnomaly {
                        day: day.clone(),
                        kind: "crawler_404_spike".into(),
                        count: *n,
                        baseline: base,
                    });
                }
            }
        }
    }

    let sources = store
        .sources()?
        .into_iter()
        .map(|s| LogSource {
            format: s.format,
            lines: s.lines,
            hits: s.hits,
            skipped: s.skipped,
        })
        .collect();
    let logs = Logs {
        window: LogWindow {
            from: rfc3339(first),
            to: rfc3339(to),
            days,
        },
        crawlers_version: spec().crawlers_version.clone(),
        sources,
        bots: bots
            .iter()
            .map(|b| LogBot {
                bot: b.bot.map(|c| c.id.clone()),
                actor: b.actor.clone(),
                hits: b.hits,
                verification: b.verification,
                status: b.status,
                first_seen: rfc3339(b.first),
                last_seen: rfc3339(b.last),
            })
            .collect(),
        per_url_actor_day: per_url,
        referrals,
        coverage,
        anomalies,
    };
    Ok(Some(Report {
        from,
        to,
        days,
        bots,
        human_visits,
        ai_referrals,
        logs,
        never_crawled,
    }))
}

/// A crawler's name, or the actor class for automation outside the catalog.
pub fn bot_name(b: &BotStats) -> String {
    b.bot.map_or_else(|| b.actor.clone(), |c| c.name.clone())
}

/// The L findings of a report.
pub fn findings(report: &Report, p: &Params, store: &Store, e: &mut Emitter) -> Result<()> {
    let origin = p.origin.unwrap_or("");
    let site_loc = |path: Option<&str>| Loc {
        url_path: path.map(String::from),
        url: path.map(|x| format!("{origin}{x}")),
        file: None,
        source: None,
    };
    for b in report.bots.iter().filter(|b| b.bot.is_some()) {
        let c = b.bot.expect("catalog crawler");
        let genuine = b.hits - b.verification.mismatch;
        if genuine >= 20 && b.blocked * 2 >= genuine {
            e.emit(
                "L-BOT-BLOCKED",
                None,
                site_loc(None),
                &c.id,
                vec![
                    ev(
                        "Crawler",
                        "爬虫",
                        format!("{} ({}, {})", c.name, c.operator, c.purpose),
                    ),
                    ev(
                        "Blocked",
                        "被拦截",
                        format!("{} of {genuine} requests answered 401/403/429", b.blocked),
                    ),
                    ev(
                        "Verified",
                        "已验证",
                        format!(
                            "{} verified, {} unverifiable",
                            b.verification.verified, b.verification.unverifiable
                        ),
                    ),
                ],
                None,
            );
        }
        if genuine >= 20 && b.status.s5 >= 10 && b.status.s5 * 20 >= genuine {
            e.emit(
                "L-BOT-SERVER-ERRORS",
                None,
                site_loc(None),
                &c.id,
                vec![
                    ev("Crawler", "爬虫", c.name.clone()),
                    ev(
                        "5xx",
                        "5xx",
                        format!("{} of {genuine} requests", b.status.s5),
                    ),
                ],
                None,
            );
        }
        if b.verification.mismatch > 0 {
            e.emit(
                "L-SPOOFED-BOT",
                None,
                site_loc(None),
                &c.id,
                vec![
                    ev("Claimed crawler", "冒充的爬虫", c.name.clone()),
                    ev(
                        "Requests",
                        "请求",
                        format!(
                            "{} from {} address(es) outside its published ranges",
                            b.verification.mismatch, b.spoofer_ips
                        ),
                    ),
                ],
                None,
            );
        }
    }

    let (from, to) = (report.from, report.to);
    let host = p.host.map(str::to_ascii_lowercase);
    let base = p.inventory.map(|i| i.base_path.clone()).unwrap_or_default();
    let robots_path = format!("{base}/robots.txt");
    let bad_robots: Vec<(u16, u64, String)> = {
        let mut stmt = store.conn.prepare(&format!(
            "SELECT status, SUM(weight), GROUP_CONCAT(DISTINCT bot) FROM hits
             WHERE {SCOPE} AND bot IS NOT NULL AND verdict != 'mismatch' AND path = ?4
               AND (status >= 500 OR status = 429)
             GROUP BY status ORDER BY status"
        ))?;
        let rows = stmt.query_map(params![from, to, host, robots_path], |r| {
            Ok((
                r.get::<_, i64>(0)? as u16,
                r.get::<_, i64>(1)? as u64,
                r.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    if !bad_robots.is_empty() {
        e.emit(
            "L-ROBOTS-TXT-UNAVAILABLE",
            None,
            site_loc(Some(&robots_path)),
            "",
            bad_robots
                .iter()
                .map(|(s, n, bots)| ev("Responses", "响应", format!("{s} ×{n} ({bots})")))
                .collect(),
            None,
        );
    }

    for a in &report.logs.anomalies {
        e.emit(
            "L-404-SPIKE",
            None,
            site_loc(None),
            &a.day,
            vec![
                ev("Day", "日期", a.day.clone()),
                ev(
                    "Crawler 404s",
                    "爬虫 404",
                    format!("{} (typical day: {:.0})", a.count, a.baseline),
                ),
            ],
            None,
        );
    }

    if let Some(robots) = p.inventory.and_then(|i| i.robots.as_ref()) {
        let mut stmt = store.conn.prepare(&format!(
            "SELECT bot, path, SUM(weight) FROM hits
             WHERE {SCOPE} AND bot IS NOT NULL AND verdict = 'verified'
             GROUP BY bot, path"
        ))?;
        let rows: Vec<(String, String, i64)> = stmt
            .query_map(params![from, to, host], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<Result<_, _>>()?;
        let mut by_bot: BTreeMap<String, (u64, Vec<String>)> = BTreeMap::new();
        for (bot, path, n) in rows {
            let Some(c) = crawler(&bot) else { continue };
            if path == robots_path {
                continue;
            }
            let group = rules_for_agent(robots, &c.robots_token);
            let local = path.strip_prefix(&base).unwrap_or(&path);
            if evaluate_path(group.as_ref(), local) == PathDecision::Disallow {
                let entry = by_bot.entry(bot).or_default();
                entry.0 += n as u64;
                entry.1.push(path);
            }
        }
        for (bot, (n, paths)) in by_bot {
            let c = crawler(&bot).expect("catalog crawler");
            let mut shown: Vec<String> = paths.iter().take(3).cloned().collect();
            if paths.len() > 3 {
                shown.push(format!("… {} more", paths.len() - 3));
            }
            e.emit(
                "L-DISALLOWED-CRAWLED",
                None,
                site_loc(None),
                &bot,
                vec![
                    ev(
                        "Crawler",
                        "爬虫",
                        format!("{} (robots token {})", c.name, c.robots_token),
                    ),
                    ev(
                        "Disallowed fetches",
                        "被禁止的抓取",
                        format!("{n} on {} path(s): {}", paths.len(), shown.join(", ")),
                    ),
                ],
                None,
            );
        }
    }

    if let Some(cov) = report
        .logs
        .coverage
        .iter()
        .find(|c| c.actor == "search_index" && c.bot.is_none())
    {
        if report.days >= 7 && cov.indexable >= 10 && cov.crawled * 10 < cov.indexable {
            let per_bot: Vec<String> = report
                .logs
                .coverage
                .iter()
                .filter(|c| c.actor == "search_index" && c.bot.is_some())
                .map(|c| format!("{} {}", c.bot.as_deref().unwrap_or_default(), c.crawled))
                .collect();
            e.emit(
                "L-AI-SEARCH-COVERAGE-LOW",
                None,
                site_loc(None),
                "",
                vec![
                    ev(
                        "Crawled",
                        "已抓取",
                        format!(
                            "{} of {} indexable pages in {} days",
                            cov.crawled, cov.indexable, report.days
                        ),
                    ),
                    ev(
                        "By crawler",
                        "按爬虫",
                        if per_bot.is_empty() {
                            "no AI search crawler requests at all".into()
                        } else {
                            per_bot.join(", ")
                        },
                    ),
                ],
                None,
            );
        }
    }
    Ok(())
}
