//! T / C / D rules over a small multilingual build with known defects.

use std::collections::{BTreeMap, BTreeSet};

use aieo_core::bundle::{
    Bundle, Gate, Inventory, Privacy, Producer, RunInfo, SiteInfo, Summary, TierCounts,
};
use aieo_core::config::{resolve_settings, Config, Settings};
use aieo_core::spec::spec;
use aieo_core::{FindingStatus, Tier};
use aieo_rules::{check, gate_passes, Options, Outcome};
use aieo_site::{Site, SiteOptions};
use url::Url;

fn page(lang: &str, title: &str, canonical: &str, extra_head: &str, body: &str) -> String {
    format!(
        r#"<!doctype html><html lang="{lang}"><head><meta charset="utf-8"><title>{title}</title>
<meta name="description" content="A page about {title}.">
<link rel="canonical" href="{canonical}">
{extra_head}
</head><body><nav><a href="/">Home</a> <a href="/guide/">Guide</a> <a href="/zh/guide/">指南</a></nav>
<h1>{title}</h1><p>Some words about {title} for readers.</p>{body}</body></html>"#
    )
}

const O: &str = "https://docs.example";

fn files() -> Vec<(&'static str, String)> {
    vec![
        ("index.html", page("en", "Home", &format!("{O}/"), "", r#"<a href="/orphan-linked/">x</a>"#)),
        (
            "guide/index.html",
            page(
                "en",
                "Guide",
                &format!("{O}/guide/"),
                &format!(r#"<link rel="alternate" hreflang="en" href="{O}/guide/"><link rel="alternate" hreflang="zh-CN" href="{O}/zh/guide/"><link rel="alternate" hreflang="english" href="{O}/guide/">"#),
                r#"<a href="/missing-page/">broken</a><a href="/moved">moved</a><script type="application/ld+json">{"@type":</script>"#,
            ),
        ),
        // zh page does not link back to the en version
        ("zh/guide/index.html", page("zh-CN", "指南", &format!("{O}/zh/guide/"), "", "")),
        ("dup-a/index.html", page("en", "Same title", &format!("{O}/dup-a/"), "", "")),
        ("dup-b/index.html", page("en", "Same title", &format!("{O}/dup-b/"), "", "")),
        // canonical on the wrong host (preview leaked into production)
        ("preview/index.html", page("en", "Preview", "https://my-site.pages.dev/preview/", "", "")),
        // noindex but listed in the sitemap
        ("draft/index.html", page("en", "Draft", &format!("{O}/draft/"), r#"<meta name="robots" content="noindex">"#, "")),
        // canonicalised to another page, and listed in the sitemap
        ("copy/index.html", page("en", "Copy", &format!("{O}/guide/"), "", "")),
        ("orphan-linked/index.html", page("en", "Linked", &format!("{O}/orphan-linked/"), "", "")),
        // indexable, never linked, not in the sitemap
        ("lonely/index.html", page("en", "Lonely", &format!("{O}/lonely/"), "", "")),
        ("404.html", "<!doctype html><html><head><title>404</title></head><body>Not found</body></html>".into()),
        (
            "sitemap-index.xml",
            format!(r#"<?xml version="1.0"?><sitemapindex><sitemap><loc>{O}/sitemap-0.xml</loc></sitemap></sitemapindex>"#),
        ),
        (
            "sitemap-0.xml",
            format!(
                r#"<?xml version="1.0"?><urlset>
<url><loc>{O}/</loc></url><url><loc>{O}/guide/</loc></url><url><loc>{O}/zh/guide/</loc></url>
<url><loc>{O}/draft/</loc></url><url><loc>{O}/copy/</loc></url><url><loc>{O}/gone/</loc></url>
<url><loc>http://localhost:4321/guide/</loc></url><url><loc>{O}/private/x/</loc></url>
<url><loc>{O}/dup-a/</loc></url><url><loc>{O}/dup-b/</loc></url><url><loc>{O}/preview/</loc></url><url><loc>{O}/orphan-linked/</loc></url>
</urlset>"#
            ),
        ),
        ("private/x/index.html", page("en", "Private", &format!("{O}/private/x/"), "", "")),
        (
            "robots.txt",
            format!("User-agent: *\nDisallow: /private/\n\nUser-agent: OAI-SearchBot\nDisallow: /\n\nSitemap: {O}/sitemap-index.xml\n"),
        ),
        ("llms.txt", "# Docs\n\n> Docs.\n\n## Pages\n- [Guide](/guide/): start\n- [Gone](/not-here/): removed\n- [Draft](/draft/)\n".into()),
        ("_redirects", "/moved /guide/ 301\n".into()),
    ]
}

fn build(files: &[(&str, String)]) -> (tempfile::TempDir, Site) {
    let dir = tempfile::tempdir().unwrap();
    for (rel, text) in files {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let site = Site::load(dir.path(), Url::parse(O).unwrap(), SiteOptions::default()).unwrap();
    (dir, site)
}

fn settings() -> Settings {
    resolve_settings(
        &Config::default(),
        spec(),
        time::macros::date!(2026 - 10 - 22),
    )
    .unwrap()
}

fn hits(o: &Outcome) -> BTreeSet<(String, String)> {
    o.findings
        .iter()
        .map(|f| {
            (
                f.rule.clone(),
                f.url.clone().unwrap_or_default().replace(O, ""),
            )
        })
        .collect()
}

#[test]
fn finds_the_planted_defects() {
    let (_d, site) = build(&files());
    let s = settings();
    let o = check(
        &site,
        &Options {
            settings: &s,
            project: None,
            rendering: None,
            baseline: None,
        },
    );
    let h = hits(&o);
    let expect = [
        ("C-LINK-BROKEN", "/guide/"),
        ("T-JSONLD-INVALID", "/guide/"),
        ("C-HREFLANG-INVALID-CODE", "/guide/"),
        ("C-HREFLANG-NOT-RECIPROCAL", "/guide/"),
        ("T-DUPLICATE-TITLE", "/dup-a/"),
        ("T-DUPLICATE-TITLE", "/dup-b/"),
        ("C-CANONICAL-FOREIGN-HOST", "/preview/"),
        ("C-SITEMAP-URL-NOINDEX", "/draft/"),
        ("C-SITEMAP-URL-NONCANONICAL", "/copy/"),
        ("C-SITEMAP-URL-MISSING", "/gone/"),
        ("C-SITEMAP-FOREIGN-HOST", "http://localhost:4321/guide/"),
        ("C-SITEMAP-ROBOTS-BLOCKED", "/private/x/"),
        ("C-PAGE-NOT-IN-SITEMAP", "/lonely/"),
        ("C-ORPHAN-PAGE", "/lonely/"),
        ("C-ROBOTS-AI-SEARCH-BLOCKED", "/robots.txt"),
        ("C-LLMS-LINK-BROKEN", "/llms.txt"),
        ("C-LLMS-LINK-NOINDEX", "/llms.txt"),
    ];
    for (rule, url) in expect {
        assert!(
            h.contains(&(rule.to_string(), url.to_string())),
            "missing {rule} at {url}; got {h:#?}"
        );
    }
    // things that are fine must stay quiet
    for quiet in [
        "C-HREFLANG-TARGET-MISSING",
        "C-ROBOTS-BLOCKS-ALL",
        "C-CANONICAL-TARGET-MISSING",
        "T-MISSING-TITLE",
    ] {
        assert!(
            !h.iter().any(|(r, _)| r == quiet),
            "unexpected {quiet}: {h:#?}"
        );
    }
    // the redirect `/moved` and the 404 page are not defects
    assert!(!h.contains(&("C-LINK-BROKEN".into(), "/".into())));
    assert!(o
        .findings
        .iter()
        .all(|f| f.url.as_deref() != Some(&format!("{O}/404"))));
    // heuristic, default-off rules do not run
    assert!(!h
        .iter()
        .any(|(r, _)| r == "T-THIN-CONTENT" || r == "T-TITLE-LENGTH"));
    // lines point into the build file
    let broken = o
        .findings
        .iter()
        .find(|f| f.rule == "C-LINK-BROKEN")
        .unwrap();
    assert_eq!(broken.file.as_deref(), Some("guide/index.html"));
    assert!(broken.line.is_some());
    assert_eq!(o.sitemaps.len(), 2);
    assert!(o.robots_txt && o.llms_txt);
    assert!(
        o.incomplete_reasons.is_empty(),
        "{:?}",
        o.incomplete_reasons
    );
    // errors fail the default gate
    let gate = aieo_core::config::parse_fail_on(&["error".into()]).unwrap();
    assert!(!gate_passes(&o.findings, &gate));
}

fn baseline_from(o: &Outcome) -> Bundle {
    Bundle {
        schema: "aieo-bundle/v1".into(),
        producer: Producer {
            name: "aieo-cli".into(),
            version: "test".into(),
            rules_version: "2.0.0".into(),
            crawlers_version: None,
        },
        generated_at: "2026-10-21T00:00:00Z".into(),
        site: SiteInfo {
            origin: O.into(),
            preset: "static".into(),
            project_id: None,
        },
        run: RunInfo {
            kinds: vec!["check".into()],
            mode: "dist".into(),
            commit: None,
            branch: None,
            ci: None,
            duration_ms: 0,
        },
        readiness: None,
        inventory: Inventory {
            pages: o.pages.clone(),
            sitemaps: o.sitemaps.clone(),
            robots_txt: true,
            llms_txt: true,
        },
        findings: o.findings.clone(),
        diff: None,
        summary: Summary {
            pages: 0,
            indexable: 0,
            findings: TierCounts::default(),
            new: None,
            gate: Gate {
                fail_on: vec![],
                passed: true,
            },
            incomplete: false,
            incomplete_reasons: vec![],
        },
        logs: None,
        search: None,
        privacy: Privacy::default(),
    }
}

#[test]
fn reports_regressions_and_new_findings_against_a_baseline() {
    let (_d, site) = build(&files());
    let s = settings();
    let before = check(
        &site,
        &Options {
            settings: &s,
            project: None,
            rendering: None,
            baseline: None,
        },
    );
    let baseline = baseline_from(&before);

    // next build: the orphan page is deleted, the guide turns noindex, the
    // dup-b gets its own title, and one new broken link appears
    let mut next = files();
    next.retain(|(rel, _)| *rel != "orphan-linked/index.html");
    for (rel, text) in next.iter_mut() {
        if *rel == "dup-b/index.html" {
            *text = text.replace("Same title", "Other title");
        }
        if *rel == "guide/index.html" {
            *text = text.replace(
                "<meta charset=\"utf-8\">",
                "<meta charset=\"utf-8\"><meta name=\"robots\" content=\"noindex\">",
            );
            *text = text.replace(
                "<a href=\"/moved\">moved</a>",
                "<a href=\"/moved\">moved</a><a href=\"/also-missing/\">new</a>",
            );
        }
    }
    let (_d2, site2) = build(&next);
    let after = check(
        &site2,
        &Options {
            settings: &s,
            project: None,
            rendering: None,
            baseline: Some(&baseline),
        },
    );
    let h = hits(&after);
    assert!(
        h.contains(&("D-PAGE-REMOVED".into(), "/orphan-linked/".into())),
        "{h:#?}"
    );
    assert!(
        h.contains(&("D-PAGE-NOINDEXED".into(), "/guide/".into())),
        "{h:#?}"
    );
    // new: the added link, and the homepage link to the deleted page
    let new_broken: BTreeSet<String> = after
        .findings
        .iter()
        .filter(|f| f.rule == "C-LINK-BROKEN" && f.status == FindingStatus::New)
        .map(|f| f.evidence[0].detail.clone())
        .collect();
    assert_eq!(
        new_broken,
        BTreeSet::from([
            format!("/also-missing/ → {O}/also-missing/"),
            format!("/orphan-linked/ → {O}/orphan-linked/"),
        ])
    );
    assert!(after.findings.iter().all(|f| f.rule != "T-DUPLICATE-TITLE"));
    assert_eq!(
        after
            .fixed
            .iter()
            .filter(|f| f.rule == "T-DUPLICATE-TITLE")
            .count(),
        2,
        "both duplicate-title findings are fixed"
    );
    let missing = |path: &str| {
        after
            .findings
            .iter()
            .find(|f| {
                f.rule == "C-SITEMAP-URL-MISSING" && f.url.as_deref() == Some(&format!("{O}{path}"))
            })
            .unwrap()
            .status
    };
    assert_eq!(missing("/gone/"), FindingStatus::Existing);
    // the deleted page is still listed in the sitemap: a new error
    assert_eq!(missing("/orphan-linked/"), FindingStatus::New);
    let new_errors = aieo_core::config::parse_fail_on(&["new:error".into()]).unwrap();
    assert!(!gate_passes(&after.findings, &new_errors));
    let counts: BTreeMap<_, _> = aieo_rules::tier_counts(
        after
            .findings
            .iter()
            .filter(|f| f.status == FindingStatus::New),
    )
    .into_iter()
    .collect();
    assert!(counts.get(&Tier::Error).copied().unwrap_or(0) >= 2);
}

#[test]
fn settings_override_tiers_switch_rules_and_suppress() {
    let (_d, site) = build(&files());
    let config: Config = toml::from_str(
        r#"
        [rules]
        "C-ORPHAN-PAGE" = "off"
        "T-THIN-CONTENT" = "on"
        "C-LINK-BROKEN" = "error"

        [[suppress]]
        rule = "C-CANONICAL-FOREIGN-HOST"
        url = "/preview/**"
        reason = "preview copy, removed next sprint"
        owner = "docs"
        expires = "2026-12-31"
        "#,
    )
    .unwrap();
    let s = resolve_settings(&config, spec(), time::macros::date!(2026 - 10 - 22)).unwrap();
    let o = check(
        &site,
        &Options {
            settings: &s,
            project: None,
            rendering: None,
            baseline: None,
        },
    );
    let h = hits(&o);
    assert!(!h.iter().any(|(r, _)| r == "C-ORPHAN-PAGE"));
    assert!(h.iter().any(|(r, _)| r == "T-THIN-CONTENT"));
    assert_eq!(
        o.findings
            .iter()
            .find(|f| f.rule == "C-LINK-BROKEN")
            .unwrap()
            .tier,
        Tier::Error
    );
    let suppressed = o
        .findings
        .iter()
        .find(|f| f.rule == "C-CANONICAL-FOREIGN-HOST")
        .unwrap();
    assert!(suppressed.suppressed.is_some());
}

#[test]
fn an_upgrade_alone_never_fails_a_diff() {
    let (_d, site) = build(&files());
    let s = settings();
    let first = check(
        &site,
        &Options {
            settings: &s,
            project: None,
            rendering: None,
            baseline: None,
        },
    );
    // a baseline taken with rules 2.0.0 that had no findings at all
    let mut baseline = baseline_from(&first);
    baseline.findings.clear();
    let o = check(
        &site,
        &Options {
            settings: &s,
            project: None,
            rendering: None,
            baseline: Some(&baseline),
        },
    );
    let later: Vec<_> = o
        .findings
        .iter()
        .filter(|f| !spec().rule_existed_in(&f.rule, "2.0.0"))
        .collect();
    assert!(
        !later.is_empty(),
        "the planted site trips rules added after 2.0.0"
    );
    for f in &later {
        assert_eq!(f.status, FindingStatus::Unknown, "{}", f.rule);
        assert!(f
            .evidence
            .iter()
            .any(|e| e.detail.contains("added after the baseline")));
    }
    let only_later: Vec<_> = later.into_iter().cloned().collect();
    let gate = aieo_core::config::parse_fail_on(&["new:notice".into()]).unwrap();
    assert!(
        gate_passes(&only_later, &gate),
        "rules from an upgrade do not count as new"
    );
    assert!(o
        .findings
        .iter()
        .any(|f| f.status == FindingStatus::New && spec().rule_existed_in(&f.rule, "2.0.0")));
}
