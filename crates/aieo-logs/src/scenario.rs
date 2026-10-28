//! A ten-day log with known crawler behaviour, from ingestion to findings.

use std::collections::BTreeSet;
use std::io::Write;

use aieo_core::config::{resolve_settings, Config};
use aieo_core::spec::spec;
use aieo_rules::emit::Emitter;
use aieo_site::robots::parse_robots;

use crate::ingest::{ingest_file, Options};
use crate::parse::Format;
use crate::report::{build, findings, Inventory, Params};
use crate::store::Store;
use crate::verify::tests::{offline_lists, Table};
use crate::verify::Verifier;

const GOOGLE_UA: &str = "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";
const OAI_UA: &str = "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; OAI-SearchBot/1.3; +https://openai.com/searchbot)";
const GPT_UA: &str = "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; GPTBot/1.2; +https://openai.com/gptbot)";
const CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Safari/537.36";

fn line(ip: &str, day: u32, sec: u32, path: &str, status: u16, referrer: &str, ua: &str) -> String {
    format!(
        "{ip} - - [{day:02}/Oct/2026:10:{:02}:{:02} +0000] \"GET {path} HTTP/1.1\" {status} 512 \"{referrer}\" \"{ua}\"\n",
        sec / 60 % 60,
        sec % 60
    )
}

fn log() -> String {
    let mut s = String::new();
    for day in 1..=10u32 {
        // a verified Googlebot crawl of every page, robots.txt answered 503 on day 4
        for (i, p) in (0..12).map(|i| format!("/docs/p{i}/")).enumerate() {
            s += &line("66.249.64.5", day, i as u32, &p, 200, "-", GOOGLE_UA);
        }
        s += &line(
            "66.249.64.5",
            day,
            30,
            "/robots.txt",
            if day == 4 { 503 } else { 200 },
            "-",
            GOOGLE_UA,
        );
        // one stale URL a day: the typical day has a crawler 404
        s += &line("66.249.64.5", day, 32, "/docs/old/", 404, "-", GOOGLE_UA);
        // ... and a section robots.txt disallows
        s += &line("66.249.64.5", day, 31, "/private/x", 200, "-", GOOGLE_UA);
        // OAI-SearchBot (verified) reaches one page
        s += &line("104.210.140.130", day, 40, "/docs/p0/", 200, "-", OAI_UA);
        // GPTBot is turned away by the WAF (its list is unavailable: unchecked)
        for k in 0..3 {
            s += &line(
                "20.171.207.9",
                day,
                50 + k,
                &format!("/docs/p{k}/"),
                403,
                "-",
                GPT_UA,
            );
        }
        // someone pretending to be Googlebot
        s += &line("203.0.113.5", day, 60, "/wp-login.php", 404, "-", GOOGLE_UA);
        // people, one arriving from ChatGPT
        s += &line("198.51.100.20", day, 70, "/docs/p1/", 200, "-", CHROME);
        s += &line(
            "198.51.100.21",
            day,
            71,
            "/docs/p2/",
            200,
            "https://chatgpt.com/",
            CHROME,
        );
        s += &line(
            "198.51.100.22",
            day,
            72,
            "/",
            200,
            "-",
            "python-requests/2.32",
        );
    }
    // day 8: a deploy dropped pages
    for i in 0..30u32 {
        s += &line(
            "66.249.64.5",
            8,
            100 + i,
            &format!("/old/{i}"),
            404,
            "-",
            GOOGLE_UA,
        );
    }
    s
}

fn verifier() -> Verifier {
    Verifier::new(
        None,
        Some(offline_lists()),
        Box::new(Table(vec![("203.0.113.5", "evil.example")])),
    )
}

#[test]
fn ingests_classifies_and_reports() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("access.log");
    let text = log();
    let (first, _) = text.split_at(text.len() / 2);
    let cut = first.rfind('\n').unwrap() + 1;
    std::fs::write(&path, &text[..cut]).unwrap();
    let v = verifier();
    let opts = Options {
        format: None,
        host: None,
        verifier: &v,
        salt: [7; 32],
    };
    let mut store = Store::in_memory().unwrap();
    let r1 = ingest_file(&mut store, &path, &opts).unwrap();
    assert_eq!(r1.format, "combined");
    assert_eq!(r1.malformed, 0);

    // the log grows, and is rotated + compressed: both resume, nothing doubles
    std::fs::write(&path, &text).unwrap();
    let rotated = dir.path().join("access.log.1.gz");
    let mut gz = flate2::write::GzEncoder::new(
        std::fs::File::create(&rotated).unwrap(),
        flate2::Compression::default(),
    );
    gz.write_all(text.as_bytes()).unwrap();
    gz.finish().unwrap();
    let r2 = ingest_file(&mut store, &path, &opts).unwrap();
    assert_eq!(r2.resumed_at, cut as u64);
    assert_eq!(r2.lines as usize, text[cut..].lines().count());
    let r3 = ingest_file(&mut store, &rotated, &opts).unwrap();
    assert!(r3.up_to_date, "same source, already read to the end");
    let total: i64 = store
        .conn
        .query_row("SELECT SUM(weight) FROM hits", [], |r| r.get(0))
        .unwrap();
    let humans: i64 = store
        .conn
        .query_row("SELECT SUM(hits) FROM human_daily", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total as usize + humans as usize, text.lines().count());
    assert_eq!(humans, 10, "plain visits are counted per day, not stored");
    let ips: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM hits WHERE ip_hash LIKE '%.%' OR ua LIKE '%Chrome%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ips, 0, "no raw IPs; no browser user agents");

    let inventory = Inventory {
        indexable: (0..12).map(|i| format!("/docs/p{i}/")).collect(),
        robots: Some(parse_robots("User-agent: *\nDisallow: /private/\n")),
        base_path: String::new(),
    };
    let params = Params {
        since_days: 30,
        host: None,
        origin: Some("https://docs.example"),
        inventory: Some(&inventory),
    };
    let report = build(&store, &params).unwrap().unwrap();
    assert_eq!(report.days, 10);
    assert_eq!(report.ai_referrals, 10);
    let google = report
        .bots
        .iter()
        .find(|b| b.bot.map(|c| c.id.as_str()) == Some("googlebot"))
        .unwrap();
    assert_eq!(google.verification.mismatch, 10);
    assert_eq!(google.verification.verified, 10 * 15 + 30);
    let cov = |actor: &str| {
        report
            .logs
            .coverage
            .iter()
            .find(|c| c.actor == actor && c.bot.is_none())
            .unwrap()
            .crawled
    };
    assert_eq!(
        (cov("classic_search"), cov("search_index"), cov("training")),
        (12, 1, 0)
    );
    assert_eq!(report.never_crawled["search_index"].len(), 11);
    assert_eq!(report.logs.anomalies.len(), 1);
    assert_eq!(report.logs.anomalies[0].day, "2026-10-08");
    assert_eq!(
        report.logs.anomalies[0].baseline, 1.0,
        "the 20 days before the logs begin are not zero-404 days"
    );
    assert!(
        report
            .logs
            .per_url_actor_day
            .iter()
            .all(|r| r.path != "/wp-login.php"),
        "spoofed requests are not crawls"
    );

    let settings = resolve_settings(
        &Config::default(),
        spec(),
        time::macros::date!(2026 - 10 - 22),
    )
    .unwrap();
    let mut e = Emitter::new(&settings);
    findings(&report, &params, &store, &mut e).unwrap();
    let got: BTreeSet<String> = e
        .out
        .iter()
        .map(|p| {
            format!(
                "{} {}",
                p.finding.rule,
                p.finding.url.clone().unwrap_or_default()
            )
        })
        .collect();
    let want: BTreeSet<String> = [
        "L-BOT-BLOCKED ",
        "L-SPOOFED-BOT ",
        "L-ROBOTS-TXT-UNAVAILABLE https://docs.example/robots.txt",
        "L-404-SPIKE ",
        "L-DISALLOWED-CRAWLED ",
        "L-AI-SEARCH-COVERAGE-LOW ",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(got, want);
    let blocked = e
        .out
        .iter()
        .find(|p| p.finding.rule == "L-BOT-BLOCKED")
        .unwrap();
    assert!(blocked.finding.evidence[0].detail.starts_with("GPTBot"));

    // a build under a base path: crawlers still fetch /robots.txt at the root
    let docs = Inventory {
        indexable: inventory.indexable.clone(),
        robots: Some(parse_robots("User-agent: *\nDisallow: /private/\n")),
        base_path: "/docs".into(),
    };
    let params = Params {
        since_days: 30,
        host: None,
        origin: Some("https://docs.example"),
        inventory: Some(&docs),
    };
    let mut e = Emitter::new(&settings);
    findings(&report, &params, &store, &mut e).unwrap();
    let robots = e
        .out
        .iter()
        .find(|p| p.finding.rule == "L-ROBOTS-TXT-UNAVAILABLE")
        .expect("robots.txt 503 at the root");
    assert_eq!(
        robots.finding.url.as_deref(),
        Some("https://docs.example/robots.txt")
    );
    let _ = Format::Combined;
}
