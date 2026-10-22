//! Every format renders, and the bundle validates against the schema aieo.ee
//! checks uploads with (spec/bundle.schema.json).

use aieo_core::bundle::Bundle;
use aieo_core::config::{parse_fail_on, resolve_settings, Config};
use aieo_core::spec::{spec, BUNDLE_SCHEMA_JSON};
use aieo_report::{build_bundle, github, markdown, pretty, sarif, Paths, RunMeta};
use aieo_rules::{check, Options};
use aieo_site::{Site, SiteOptions};
use url::Url;

fn run(
    baseline: Option<&Bundle>,
) -> (
    tempfile::TempDir,
    Bundle,
    Vec<aieo_rules::readiness::ModuleResult>,
) {
    let dir = tempfile::tempdir().unwrap();
    let w = |rel: &str, t: &str| {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, t).unwrap();
    };
    w(
        "index.html",
        r#"<!doctype html><html lang="en"><head><title>Home page of the site</title><link rel="canonical" href="https://s.test/"></head><body><nav><a href="/a/">A</a><a href="/missing/">M</a></nav><h1>Home</h1></body></html>"#,
    );
    w(
        "a/index.html",
        r#"<!doctype html><html><head><title>A | Pipe</title></head><body><h1>A</h1><a href="/">home</a><script type="application/ld+json">{oops</script></body></html>"#,
    );
    w("robots.txt", "User-agent: *\nDisallow: /\n");
    let site = Site::load(
        dir.path(),
        Url::parse("https://s.test").unwrap(),
        SiteOptions::default(),
    )
    .unwrap();
    let settings = resolve_settings(
        &Config::default(),
        spec(),
        time::macros::date!(2026 - 10 - 22),
    )
    .unwrap();
    let outcome = check(
        &site,
        &Options {
            settings: &settings,
            project: None,
            rendering: None,
            baseline,
        },
    );
    let meta = RunMeta {
        origin: "https://s.test".into(),
        preset: "static".into(),
        project_id: None,
        kinds: vec![if baseline.is_some() { "diff" } else { "check" }.into()],
        commit: Some("abc123".into()),
        branch: Some("main".into()),
        ci: None,
        duration_ms: 12,
        generated_at: "2026-10-22T08:00:00.123Z".into(),
        fail_on: parse_fail_on(&["error".into()]).unwrap(),
        baseline: baseline.map(|b| (b.generated_at.clone(), b.run.commit.clone())),
    };
    let bundle = build_bundle(&outcome, &meta);
    (dir, bundle, outcome.readiness.modules)
}

fn validate(bundle: &Bundle) {
    let schema: serde_json::Value = serde_json::from_str(BUNDLE_SCHEMA_JSON).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let instance = serde_json::to_value(bundle).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&instance)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "bundle does not match spec/bundle.schema.json:\n{}",
        errors.join("\n")
    );
}

#[test]
fn bundles_match_the_schema() {
    let (_d, first, _) = run(None);
    validate(&first);
    // the validator does reject what aieo.ee must refuse
    let schema: serde_json::Value = serde_json::from_str(BUNDLE_SCHEMA_JSON).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let mut tampered = serde_json::to_value(&first).unwrap();
    tampered["privacy"]["raw_logs_included"] = serde_json::json!(true);
    assert!(!validator.is_valid(&tampered));
    let mut tampered = serde_json::to_value(&first).unwrap();
    tampered["findings"][0]["fingerprint"] = serde_json::json!("not-hex");
    assert!(!validator.is_valid(&tampered));
    assert!(!first.summary.gate.passed);
    assert!(first
        .findings
        .iter()
        .any(|f| f.rule == "C-ROBOTS-BLOCKS-ALL"));

    // baseline round-trip through JSON and gzip
    let dir = tempfile::tempdir().unwrap();
    let json = dir.path().join("baseline.json");
    std::fs::write(&json, first.to_json()).unwrap();
    let gz = dir.path().join("bundle.json.gz");
    std::fs::write(&gz, first.to_gzip()).unwrap();
    let read = Bundle::read(&json).unwrap();
    assert_eq!(
        read.findings.len(),
        Bundle::read(&gz).unwrap().findings.len()
    );

    let (_d2, second, _) = run(Some(&read));
    validate(&second);
    let diff = second.diff.as_ref().unwrap();
    assert_eq!(diff.new, 0);
    assert_eq!(diff.existing as usize, second.findings.len());
    assert_eq!(second.summary.new.unwrap().error, 0);
}

#[test]
fn renders_every_format() {
    let (_d, bundle, modules) = run(None);
    let paths = Paths {
        dist: "dist".into(),
    };

    let sarif: serde_json::Value = serde_json::from_str(&sarif::render(&bundle, &paths)).unwrap();
    let results = sarif["runs"][0]["results"].as_array().unwrap();
    assert_eq!(results.len(), bundle.findings.len());
    let broken = results
        .iter()
        .find(|r| r["ruleId"] == "C-LINK-BROKEN")
        .unwrap();
    assert_eq!(
        broken["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
        "dist/index.html"
    );
    assert!(
        broken["locations"][0]["physicalLocation"]["region"]["startLine"]
            .as_u64()
            .is_some()
    );
    assert!(sarif["runs"][0]["tool"]["driver"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["id"] == "C-LINK-BROKEN"));

    let gh = github::render(&bundle, &paths);
    assert!(
        gh.lines()
            .any(|l| l.starts_with("::error file=dist/robots.txt")
                && l.contains("C-ROBOTS-BLOCKS-ALL")),
        "{gh}"
    );
    assert!(gh.contains("::warning file=dist/index.html,line="), "{gh}");

    let md = markdown::render(&bundle, aieo_core::Lang::En, 10);
    assert!(md.contains("aieo check — ❌ failed"));
    assert!(
        md.contains("A \\| Pipe") || !md.contains("A | Pipe |"),
        "table cells are escaped"
    );

    let style = pretty::Style {
        color: false,
        lang: aieo_core::Lang::Zh,
        per_rule: 5,
        verbose: false,
    };
    let text = pretty::render(&bundle, &modules, &[], &paths, &style);
    assert!(
        text.contains("C-ROBOTS-BLOCKS-ALL") && text.contains("FAILED"),
        "{text}"
    );
    assert!(text.contains("AI 就绪度"));
}
