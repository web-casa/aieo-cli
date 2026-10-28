//! The `aieo` binary: exit codes, baselines, reports.

use std::path::Path;
use std::process::Command;

fn aieo(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_aieo"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env_remove("GITHUB_STEP_SUMMARY")
        .env_remove("GITHUB_ACTIONS")
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into(),
        String::from_utf8_lossy(&out.stderr).into(),
    )
}

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

const PAGE: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Guide to the thing</title>
<meta name="description" content="How to use the thing."><link rel="canonical" href="https://t.example/guide/"></head>
<body><nav><a href="/">Home</a></nav><h1>Guide</h1><p>Words.</p></body></html>"#;

fn project() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    write(
        p,
        "dist/index.html",
        r#"<!doctype html><html lang="en"><head><title>Home of the thing</title><link rel="canonical" href="https://t.example/"></head><body><nav><a href="/guide/">Guide</a></nav><h1>Home</h1></body></html>"#,
    );
    write(p, "dist/guide/index.html", PAGE);
    write(
        p,
        "dist/robots.txt",
        "User-agent: *\nAllow: /\nSitemap: https://t.example/sitemap.xml\n",
    );
    write(p, "dist/sitemap.xml", "<urlset><url><loc>https://t.example/</loc></url><url><loc>https://t.example/guide/</loc></url></urlset>");
    d
}

#[test]
fn passes_a_clean_build_and_fails_a_regression() {
    let d = project();
    let p = d.path();
    let (code, _, err) = aieo(p, &["init"]);
    assert_eq!(code, 0, "{err}");
    let config = std::fs::read_to_string(p.join("aieo.toml")).unwrap();
    assert!(
        config.contains("site = \"https://t.example\""),
        "origin inferred from the build: {config}"
    );

    let (code, out, err) = aieo(p, &["check"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("Gate PASSED"));

    let (code, _, err) = aieo(p, &["snapshot"]);
    assert_eq!(code, 0, "{err}");
    assert!(p.join(".aieo/baseline.json").is_file());

    // the guide page turns noindex while the sitemap still lists it
    write(
        p,
        "dist/guide/index.html",
        &PAGE.replace(
            "<meta charset=\"utf-8\">",
            "<meta charset=\"utf-8\"><meta name=\"robots\" content=\"noindex\">",
        ),
    );
    let (code, out, _) = aieo(p, &["diff", "--format", "json"]);
    assert_eq!(code, 1, "{out}");
    let bundle: serde_json::Value = serde_json::from_str(&out).unwrap();
    let new: Vec<&str> = bundle["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["status"] == "new")
        .map(|f| f["rule"].as_str().unwrap())
        .collect();
    assert!(
        new.contains(&"D-PAGE-NOINDEXED") && new.contains(&"C-SITEMAP-URL-NOINDEX"),
        "{new:?}"
    );
    assert_eq!(bundle["summary"]["gate"]["passed"], false);
    assert_eq!(
        bundle["summary"]["gate"]["fail_on"],
        serde_json::json!(["new:error", "new:warning"])
    );

    // --fail-on never: report only
    let (code, _, _) = aieo(
        p,
        &[
            "diff",
            "--fail-on",
            "never",
            "--report",
            "sarif=out/a.sarif",
            "--report",
            "bundle=out/b.json.gz",
        ],
    );
    assert_eq!(code, 0);
    assert!(p.join("out/a.sarif").is_file() && p.join("out/b.json.gz").is_file());
}

#[test]
fn diff_finds_the_snapshot_without_a_config() {
    let d = project();
    let p = d.path();
    let (code, _, err) = aieo(p, &["snapshot", "--site", "https://t.example"]);
    assert_eq!(code, 0, "{err}");
    assert!(!p.join("aieo.toml").exists());
    let (code, out, err) = aieo(p, &["diff", "--site", "https://t.example"]);
    assert_eq!(code, 0, "{out}{err}");
}

#[test]
fn incomplete_evidence_and_usage_errors_exit_2() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    // no build directory
    let (code, _, err) = aieo(p, &["check"]);
    assert_eq!(code, 2);
    assert!(err.contains("not found"), "{err}");

    // a build without a homepage: fail closed
    write(p, "dist/about/index.html", PAGE);
    let (code, _, err) = aieo(p, &["check", "--site", "https://t.example"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("incomplete evidence"), "{err}");
    let (code, _, _) = aieo(
        p,
        &[
            "check",
            "--site",
            "https://t.example",
            "--allow-incomplete",
            "--fail-on",
            "never",
        ],
    );
    assert_eq!(code, 0);

    // no origin anywhere: no --site, no aieo.toml, nothing in the build
    write(
        p,
        "bare/index.html",
        "<!doctype html><html><head><title>t</title></head><body>x</body></html>",
    );
    let (code, _, err) = aieo(p, &["check", "bare"]);
    assert_eq!(code, 2);
    assert!(err.contains("--site"), "{err}");

    // diff without a baseline
    let (code, _, err) = aieo(p, &["diff", "--site", "https://t.example"]);
    assert_eq!(code, 2);
    assert!(err.contains("aieo snapshot"), "{err}");

    // a bad aieo.toml is an error, not a silent default
    write(p, "aieo.toml", "[rules]\n\"NOPE\" = \"off\"\n");
    let (code, _, err) = aieo(p, &["check", "--site", "https://t.example"]);
    assert_eq!(code, 2);
    assert!(err.contains("unknown rule"), "{err}");
}

#[test]
fn lists_and_explains_rules() {
    let d = tempfile::tempdir().unwrap();
    let (code, out, _) = aieo(d.path(), &["rules", "--family", "C"]);
    assert_eq!(code, 0);
    assert!(out.contains("C-HREFLANG-NOT-RECIPROCAL") && !out.contains("F04"));
    let (code, out, _) = aieo(
        d.path(),
        &["--lang", "zh", "explain", "c-sitemap-url-noindex"],
    );
    assert_eq!(code, 0);
    assert!(
        out.contains("Sitemap 列出了 noindex 页面") && out.contains("developers.google.com"),
        "{out}"
    );
    let (code, _, _) = aieo(d.path(), &["explain", "X-NOPE"]);
    assert_eq!(code, 2);
}
