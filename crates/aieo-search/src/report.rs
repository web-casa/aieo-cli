//! `.aieo/search.db` → the bundle's `search` section and the S findings,
//! which read Google's view of each page against the build:
//!
//! - an indexable page Google has not indexed (S-NOT-INDEXED);
//! - a page whose canonical Google overrode (S-CANONICAL-OVERRIDDEN);
//! - a page the build serves and allows that Google could not fetch, found
//!   blocked by robots.txt, or saw noindexed (S-LIVE-DIFFERS) — production
//!   is not what was built, or was not at Google's last crawl;
//! - clicks halved against the previous period (S-CLICKS-DROPPED);
//! - an IndexNow key the build does not serve (S-INDEXNOW-KEY-INVALID).

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::params;
use time::{Date, Duration, OffsetDateTime};

use aieo_core::bundle::{
    BingAiDay, BingRow, BingSearch, GoogleSearch, IndexNowLog, IndexNowSubmission,
    InspectionRecord, Search, SearchMetrics, SearchRow,
};
use aieo_core::{Evidence, Localized};
use aieo_rules::emit::{Emitter, Loc};

use crate::store::{Inspection, SearchStore};

/// A page of the build.
#[derive(Debug, Clone)]
pub struct Page {
    pub url: String,
    pub indexable: bool,
}

pub struct Params<'a> {
    pub property: Option<&'a str>,
    pub bing_site: Option<&'a str>,
    pub days: i64,
    pub pages: &'a [Page],
    /// Some(false) when an IndexNow key is configured but not in the build
    pub indexnow_key_in_build: Option<bool>,
    pub indexnow_endpoint: &'a str,
    pub origin: &'a str,
}

fn ev(en: &str, zh: &str, detail: impl Into<String>) -> Evidence {
    Evidence::new(Localized::new(en, zh), detail.into())
}

fn rfc3339(ts: i64) -> String {
    OffsetDateTime::from_unix_timestamp(ts)
        .ok()
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default()
}

fn date(s: &str) -> Option<Date> {
    Date::parse(s, time::macros::format_description!("[year]-[month]-[day]")).ok()
}

/// The URL as the build lists it, trying the other trailing-slash form.
fn build_key<'a>(pages: &'a HashMap<String, &'a Page>, url: &str) -> Option<&'a Page> {
    let u = url.split('#').next().unwrap_or(url);
    pages.get(u).copied().or_else(|| {
        let other = match u.strip_suffix('/') {
            Some(s) => s.to_string(),
            None => format!("{u}/"),
        };
        pages.get(&other).copied()
    })
}

fn newest_gsc_day(store: &SearchStore, property: &str) -> Result<Option<Date>> {
    let d: Option<String> = store.conn.query_row(
        "SELECT MAX(date) FROM gsc_page_daily WHERE property = ?1",
        [property],
        |r| r.get(0),
    )?;
    Ok(d.as_deref().and_then(date))
}

fn google(store: &SearchStore, property: &str, days: i64) -> Result<Option<GoogleSearch>> {
    let Some(end) = newest_gsc_day(store, property)? else {
        let inspections = store.inspections(property)?;
        if inspections.is_empty() {
            return Ok(None);
        }
        let today = OffsetDateTime::now_utc().date();
        return Ok(Some(GoogleSearch {
            property: property.into(),
            start: today.to_string(),
            end: today.to_string(),
            totals: SearchMetrics {
                clicks: 0.0,
                impressions: 0.0,
                ctr: 0.0,
                position: 0.0,
            },
            pages: Vec::new(),
            queries: Vec::new(),
            inspections: inspections.iter().map(record).collect(),
        }));
    };
    let start = end - Duration::days(days - 1);
    let (s, e) = (start.to_string(), end.to_string());
    let totals = store.conn.query_row(
        "SELECT COALESCE(SUM(clicks),0), COALESCE(SUM(impressions),0),
                COALESCE(SUM(position * impressions) / NULLIF(SUM(impressions), 0), 0)
         FROM gsc_page_daily WHERE property = ?1 AND date BETWEEN ?2 AND ?3",
        params![property, s, e],
        |r| {
            Ok((
                r.get::<_, f64>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, f64>(2)?,
            ))
        },
    )?;
    let ctr = |c: f64, i: f64| if i > 0.0 { c / i } else { 0.0 };
    let mut stmt = store.conn.prepare(
        "SELECT page, SUM(clicks), SUM(impressions),
                COALESCE(SUM(position * impressions) / NULLIF(SUM(impressions), 0), 0)
         FROM gsc_page_daily WHERE property = ?1 AND date BETWEEN ?2 AND ?3
         GROUP BY page ORDER BY SUM(clicks) DESC, SUM(impressions) DESC LIMIT 1000",
    )?;
    let pages = stmt
        .query_map(params![property, s, e], |r| {
            let (c, i): (f64, f64) = (r.get(1)?, r.get(2)?);
            Ok(SearchRow {
                key: r.get(0)?,
                clicks: c,
                impressions: i,
                ctr: ctr(c, i),
                position: r.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut stmt = store.conn.prepare(
        "SELECT query, clicks, impressions, ctr, position FROM gsc_queries
         WHERE property = ?1 AND end = (SELECT MAX(end) FROM gsc_queries WHERE property = ?1)
         ORDER BY clicks DESC, impressions DESC LIMIT 500",
    )?;
    let queries = stmt
        .query_map([property], |r| {
            Ok(SearchRow {
                key: r.get(0)?,
                clicks: r.get(1)?,
                impressions: r.get(2)?,
                ctr: r.get(3)?,
                position: r.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(GoogleSearch {
        property: property.into(),
        start: s,
        end: e,
        totals: SearchMetrics {
            clicks: totals.0,
            impressions: totals.1,
            ctr: ctr(totals.0, totals.1),
            position: totals.2,
        },
        pages,
        queries,
        inspections: store.inspections(property)?.iter().map(record).collect(),
    }))
}

fn record(i: &Inspection) -> InspectionRecord {
    InspectionRecord {
        url: i.url.clone(),
        inspected_at: rfc3339(i.inspected_at),
        verdict: i.verdict.clone(),
        coverage_state: i.coverage_state.clone(),
        indexing_state: i.indexing_state.clone(),
        robots_txt_state: i.robots_txt_state.clone(),
        page_fetch_state: i.page_fetch_state.clone(),
        last_crawl_time: i.last_crawl_time.clone(),
        google_canonical: i.google_canonical.clone(),
        user_canonical: i.user_canonical.clone(),
    }
}

fn bing(store: &SearchStore, site: &str, days: i64) -> Result<Option<BingSearch>> {
    let rows = |sql: &str| -> Result<Vec<BingRow>> {
        let mut stmt = store.conn.prepare(sql)?;
        let out = stmt
            .query_map(params![site, days], |r| {
                Ok(BingRow {
                    key: r.get(0)?,
                    clicks: r.get::<_, i64>(1)? as u64,
                    impressions: r.get::<_, i64>(2)? as u64,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(out)
    };
    let pages = rows(
        "SELECT page, SUM(clicks), SUM(impressions) FROM bing_page_daily
         WHERE site = ?1 AND date > date((SELECT MAX(date) FROM bing_page_daily WHERE site = ?1), '-' || ?2 || ' days')
         GROUP BY page ORDER BY SUM(clicks) DESC LIMIT 1000",
    )?;
    let queries = rows(
        "SELECT query, SUM(clicks), SUM(impressions) FROM bing_query_daily
         WHERE site = ?1 AND date > date((SELECT MAX(date) FROM bing_query_daily WHERE site = ?1), '-' || ?2 || ' days')
         GROUP BY query ORDER BY SUM(clicks) DESC LIMIT 500",
    )?;
    let mut stmt = store.conn.prepare(
        "SELECT date, citations, cited_pages FROM bing_ai_daily WHERE site = ?1 ORDER BY date",
    )?;
    let ai = stmt
        .query_map([site], |r| {
            Ok(BingAiDay {
                day: r.get(0)?,
                citations: r.get::<_, i64>(1)? as u64,
                cited_pages: r.get::<_, i64>(2)? as u64,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(
        (!pages.is_empty() || !queries.is_empty() || !ai.is_empty()).then(|| BingSearch {
            site: site.into(),
            pages,
            queries,
            ai,
        }),
    )
}

fn indexnow(store: &SearchStore, endpoint: &str) -> Result<Option<IndexNowLog>> {
    let mut stmt = store
        .conn
        .prepare("SELECT at, urls, status FROM indexnow ORDER BY at DESC LIMIT 50")?;
    let submissions = stmt
        .query_map([], |r| {
            Ok(IndexNowSubmission {
                at: rfc3339(r.get(0)?),
                urls: r.get::<_, i64>(1)? as u64,
                status: r.get::<_, i64>(2)? as u16,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok((!submissions.is_empty()).then(|| IndexNowLog {
        endpoint: endpoint.into(),
        submissions,
    }))
}

pub fn build(store: &SearchStore, p: &Params) -> Result<Option<Search>> {
    let search = Search {
        google: match p.property {
            Some(prop) => google(store, prop, p.days)?,
            None => None,
        },
        bing: match p.bing_site {
            Some(site) => bing(store, site, p.days)?,
            None => None,
        },
        indexnow: indexnow(store, p.indexnow_endpoint)?,
    };
    Ok(
        (search.google.is_some() || search.bing.is_some() || search.indexnow.is_some())
            .then_some(search),
    )
}

const FETCH_FAILED: [&str; 7] = [
    "NOT_FOUND",
    "SERVER_ERROR",
    "ACCESS_DENIED",
    "ACCESS_FORBIDDEN",
    "BLOCKED_4XX",
    "SOFT_404",
    "REDIRECT_ERROR",
];

fn path_of(url: &str) -> String {
    url::Url::parse(url)
        .map(|u| u.path().to_string())
        .unwrap_or_default()
}

pub fn findings(store: &SearchStore, p: &Params, e: &mut Emitter) -> Result<()> {
    let by_url: HashMap<String, &Page> = p.pages.iter().map(|pg| (pg.url.clone(), pg)).collect();
    let loc = |url: &str| Loc {
        url_path: Some(path_of(url)),
        url: Some(url.to_string()),
        file: None,
        source: None,
    };
    if let Some(prop) = p.property {
        for i in store.inspections(prop)? {
            let Some(page) = build_key(&by_url, &i.url).filter(|pg| pg.indexable) else {
                continue;
            };
            let crawl = i.last_crawl_time.clone().unwrap_or_else(|| "never".into());
            let fetch = i.page_fetch_state.as_deref().unwrap_or("");
            let blocked_robots = i.robots_txt_state.as_deref() == Some("DISALLOWED");
            let blocked_index = matches!(
                i.indexing_state.as_deref(),
                Some("BLOCKED_BY_META_TAG" | "BLOCKED_BY_HTTP_HEADER")
            );
            if FETCH_FAILED.contains(&fetch) || blocked_robots || blocked_index {
                let what = if blocked_robots {
                    "robots.txt disallows it".to_string()
                } else if blocked_index {
                    format!(
                        "noindex ({})",
                        i.indexing_state.as_deref().unwrap_or_default()
                    )
                } else {
                    format!("fetch {fetch}")
                };
                e.emit(
                    "S-LIVE-DIFFERS",
                    None,
                    loc(&page.url),
                    "",
                    vec![
                        ev("Build", "构建产物", "indexable, allowed"),
                        ev("Google saw", "Google 看到", what),
                        ev("Last crawl", "最近抓取", crawl.clone()),
                    ],
                    None,
                );
                continue;
            }
            if i.verdict.as_deref() != Some("PASS")
                && i.coverage_state.is_some()
                && !fetch.is_empty()
            {
                e.emit(
                    "S-NOT-INDEXED",
                    None,
                    loc(&page.url),
                    "",
                    vec![
                        ev(
                            "Coverage",
                            "覆盖状态",
                            i.coverage_state.clone().unwrap_or_default(),
                        ),
                        ev("Last crawl", "最近抓取", crawl.clone()),
                    ],
                    None,
                );
            }
            if let (Some(g), Some(u)) = (&i.google_canonical, &i.user_canonical) {
                if g.trim_end_matches('/') != u.trim_end_matches('/') {
                    e.emit(
                        "S-CANONICAL-OVERRIDDEN",
                        None,
                        loc(&page.url),
                        "",
                        vec![
                            ev("Declared", "声明的", u.clone()),
                            ev("Google chose", "Google 选择的", g.clone()),
                        ],
                        None,
                    );
                }
            }
        }
        if let Some(end) = newest_gsc_day(store, prop)? {
            let w = p.days;
            let sum = |from: Date, to: Date| -> Result<HashMap<String, f64>> {
                let mut stmt = store.conn.prepare(
                    "SELECT page, SUM(clicks) FROM gsc_page_daily WHERE property = ?1 AND date BETWEEN ?2 AND ?3 GROUP BY page",
                )?;
                let out = stmt
                    .query_map(params![prop, from.to_string(), to.to_string()], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })?
                    .collect::<Result<_, _>>()?;
                Ok(out)
            };
            let now = sum(end - Duration::days(w - 1), end)?;
            let before = sum(end - Duration::days(2 * w - 1), end - Duration::days(w))?;
            let has_previous: i64 = store.conn.query_row(
                "SELECT COUNT(*) FROM gsc_page_daily WHERE property = ?1 AND date <= ?2",
                params![prop, (end - Duration::days(w)).to_string()],
                |r| r.get(0),
            )?;
            if has_previous > 0 {
                for (url, prev) in &before {
                    let cur = now.get(url).copied().unwrap_or(0.0);
                    if *prev >= 20.0 && cur * 2.0 <= *prev && build_key(&by_url, url).is_some() {
                        e.emit(
                            "S-CLICKS-DROPPED",
                            None,
                            loc(url),
                            "",
                            vec![ev(
                                "Clicks",
                                "点击",
                                format!(
                                    "{prev:.0} → {cur:.0} (last {w} days against the {w} before)"
                                ),
                            )],
                            None,
                        );
                    }
                }
            }
        }
    }
    if p.indexnow_key_in_build == Some(false) {
        e.emit(
            "S-INDEXNOW-KEY-INVALID",
            None,
            Loc {
                url_path: None,
                url: Some(p.origin.to_string()),
                file: None,
                source: None,
            },
            "",
            vec![ev(
                "Build",
                "构建产物",
                "the configured key's /<key>.txt is not in the build",
            )],
            None,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aieo_core::config::{resolve_settings, Config};
    use aieo_core::spec::spec;
    use std::collections::BTreeSet;

    fn inspection(
        url: &str,
        verdict: &str,
        coverage: &str,
        fetch: &str,
        robots: &str,
        indexing: &str,
        canon: (&str, &str),
    ) -> Inspection {
        let s = |v: &str| (!v.is_empty()).then(|| v.to_string());
        Inspection {
            url: url.into(),
            inspected_at: 1_792_800_000,
            verdict: s(verdict),
            coverage_state: s(coverage),
            indexing_state: s(indexing),
            robots_txt_state: s(robots),
            page_fetch_state: s(fetch),
            last_crawl_time: Some("2026-10-20T10:00:00Z".into()),
            google_canonical: s(canon.0),
            user_canonical: s(canon.1),
            crawled_as: Some("MOBILE".into()),
        }
    }

    #[test]
    fn reads_google_against_the_build() {
        let store = SearchStore::in_memory().unwrap();
        let prop = "sc-domain:e.com";
        let ok = ("https://e.com/a/", "https://e.com/a/");
        store
            .save_inspection(
                prop,
                &inspection(
                    "https://e.com/a/",
                    "PASS",
                    "Submitted and indexed",
                    "SUCCESSFUL",
                    "ALLOWED",
                    "INDEXING_ALLOWED",
                    ok,
                ),
            )
            .unwrap();
        store
            .save_inspection(
                prop,
                &inspection(
                    "https://e.com/b/",
                    "NEUTRAL",
                    "Crawled - currently not indexed",
                    "SUCCESSFUL",
                    "ALLOWED",
                    "INDEXING_ALLOWED",
                    ("", ""),
                ),
            )
            .unwrap();
        store
            .save_inspection(
                prop,
                &inspection(
                    "https://e.com/c",
                    "FAIL",
                    "Not found (404)",
                    "NOT_FOUND",
                    "ALLOWED",
                    "INDEXING_ALLOWED",
                    ("", ""),
                ),
            )
            .unwrap();
        store
            .save_inspection(
                prop,
                &inspection(
                    "https://e.com/d/",
                    "PASS",
                    "Indexed",
                    "SUCCESSFUL",
                    "ALLOWED",
                    "INDEXING_ALLOWED",
                    ("https://e.com/other/", "https://e.com/d/"),
                ),
            )
            .unwrap();
        store
            .save_inspection(
                prop,
                &inspection(
                    "https://e.com/gone/",
                    "FAIL",
                    "Not found (404)",
                    "NOT_FOUND",
                    "ALLOWED",
                    "",
                    ("", ""),
                ),
            )
            .unwrap();
        // clicks: /a/ halves (40 → 10), /b/ steady
        let rows = |day: &str, page: &str, clicks: f64| crate::google::Row {
            keys: vec![day.into(), page.into()],
            clicks,
            impressions: clicks * 10.0,
            ctr: 0.1,
            position: 4.0,
        };
        let mut data = Vec::new();
        for d in 1..=14 {
            let day = format!("2026-10-{d:02}");
            let a = if d <= 7 { 40.0 / 7.0 } else { 10.0 / 7.0 };
            data.push(rows(&day, "https://e.com/a/", a));
            data.push(rows(&day, "https://e.com/b/", 3.0));
        }
        crate::google::save_pages(&store, prop, &data).unwrap();
        store
            .record_indexnow(
                "https://api.indexnow.org/indexnow",
                "e.com",
                3,
                202,
                "received",
            )
            .unwrap();

        let pages: Vec<Page> = [
            "https://e.com/a/",
            "https://e.com/b/",
            "https://e.com/c/",
            "https://e.com/d/",
        ]
        .iter()
        .map(|u| Page {
            url: u.to_string(),
            indexable: true,
        })
        .collect();
        let p = Params {
            property: Some(prop),
            bing_site: None,
            days: 7,
            pages: &pages,
            indexnow_key_in_build: Some(false),
            indexnow_endpoint: "https://api.indexnow.org/indexnow",
            origin: "https://e.com",
        };
        let search = build(&store, &p).unwrap().unwrap();
        let g = search.google.as_ref().unwrap();
        assert_eq!(
            (g.start.as_str(), g.end.as_str()),
            ("2026-10-08", "2026-10-14")
        );
        assert_eq!(g.pages[0].key, "https://e.com/b/");
        assert_eq!(g.inspections.len(), 5);
        assert_eq!(search.indexnow.unwrap().submissions[0].status, 202);

        let settings = resolve_settings(
            &Config::default(),
            spec(),
            time::macros::date!(2026 - 10 - 22),
        )
        .unwrap();
        let mut e = Emitter::new(&settings);
        findings(&store, &p, &mut e).unwrap();
        let got: BTreeSet<String> = e
            .out
            .iter()
            .map(|x| {
                format!(
                    "{} {}",
                    x.finding.rule,
                    x.finding.url.clone().unwrap_or_default()
                )
            })
            .collect();
        let want: BTreeSet<String> = [
            "S-NOT-INDEXED https://e.com/b/",
            "S-LIVE-DIFFERS https://e.com/c/",
            "S-CANONICAL-OVERRIDDEN https://e.com/d/",
            "S-CLICKS-DROPPED https://e.com/a/",
            "S-INDEXNOW-KEY-INVALID https://e.com",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(
            got, want,
            "a page gone from the build (/gone/) is the D rules' business"
        );
    }
}
