//! Offline paths of `aieo indexnow`, `aieo search` and `aieo push`: the
//! IndexNow plan from a baseline, a Bing AI import read back as the
//! bundle's `search` section, and the refusals without credentials.

use std::path::Path;
use std::process::Command;

fn aieo(dir: &Path, args: &[&str], config_dir: &Path) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_aieo"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env("AIEO_CONFIG_DIR", config_dir)
        .env_remove("AIEO_API_KEY")
        .env_remove("BING_WEBMASTER_API_KEY")
        .env_remove("GOOGLE_APPLICATION_CREDENTIALS")
        .env_remove("GITHUB_STEP_SUMMARY")
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

fn page(title: &str, path: &str) -> String {
    format!(
        r#"<!doctype html><html lang="en"><head><title>{title} page</title><link rel="canonical" href="https://t.example{path}"></head><body><nav><a href="/">Home</a><a href="/a/">A</a></nav><h1>{title}</h1><p>{title} text.</p></body></html>"#
    )
}

fn project() -> (tempfile::TempDir, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    write(p, "dist/index.html", &page("Home", "/"));
    write(p, "dist/a/index.html", &page("A", "/a/"));
    write(p, "dist/b/index.html", &page("B", "/b/"));
    write(
        p,
        "aieo.toml",
        "site = \"https://t.example\"\n\n[indexnow]\nkey = \"0123456789abcdef\"\n",
    );
    (d, tempfile::tempdir().unwrap())
}

#[test]
fn indexnow_plans_the_changes_and_sends_nothing_without_yes() {
    let (d, cfg) = project();
    let p = d.path();
    let (code, _, err) = aieo(p, &["snapshot"], cfg.path());
    assert_eq!(code, 0, "{err}");
    write(p, "dist/a/index.html", &page("A changed", "/a/"));
    write(p, "dist/c/index.html", &page("C", "/c/"));
    std::fs::remove_dir_all(p.join("dist/b")).unwrap();
    let (code, out, err) = aieo(p, &["indexnow", "submit"], cfg.path());
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("1 new, 1 changed, 1 removed"), "{err}");
    let mut urls: Vec<&str> = out.lines().collect();
    urls.sort();
    assert_eq!(
        urls,
        vec![
            "https://t.example/a/",
            "https://t.example/b/",
            "https://t.example/c/"
        ]
    );
    assert!(err.contains("--yes"));
    assert!(
        !p.join(".aieo/audit.jsonl").exists(),
        "a plan sends nothing"
    );
}

#[test]
fn bing_ai_import_reaches_the_search_section() {
    let (d, cfg) = project();
    let p = d.path();
    write(
        p,
        "ai.csv",
        "Date,Citations,Cited Pages\n10/1/2026 12:00:00 AM,12,3\n10/2/2026 12:00:00 AM,\"1,000\",40\n",
    );
    let (code, _, err) = aieo(p, &["search", "bing", "import-ai", "ai.csv"], cfg.path());
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("1012 citations"), "{err}");
    let (code, out, err) = aieo(p, &["search", "report", "--json"], cfg.path());
    assert_eq!(code, 0, "{err}");
    let bundle: serde_json::Value = serde_json::from_str(&out).unwrap();
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../spec/bundle.schema.json")).unwrap();
    let v = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<String> = v
        .iter_errors(&bundle)
        .map(|e| format!("{e} at {}", e.instance_path()))
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(bundle["search"]["bing"]["ai"][1]["citations"], 1000);
    // the configured IndexNow key is not in the build
    assert!(bundle["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["rule"] == "S-INDEXNOW-KEY-INVALID"));
    // served where key_location says: no finding
    write(p, "dist/keys/indexnow.txt", "0123456789abcdef\n");
    write(
        p,
        "aieo.toml",
        "site = \"https://t.example\"\n\n[indexnow]\nkey = \"0123456789abcdef\"\nkey_location = \"/keys/indexnow.txt\"\n",
    );
    let (code, out, err) = aieo(p, &["search", "report", "--json"], cfg.path());
    assert_eq!(code, 0, "{err}");
    let bundle: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(bundle["findings"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["rule"] != "S-INDEXNOW-KEY-INVALID"));
    write(
        p,
        "ai-bad.csv",
        "Date,Citations,Cited Pages\n2026-10-01,1,1\n",
    );
    let (code, _, err) = aieo(
        p,
        &["search", "bing", "import-ai", "ai-bad.csv"],
        cfg.path(),
    );
    assert_eq!(code, 2);
    assert!(err.contains("unsupported date"), "{err}");
}

#[test]
fn cloud_commands_need_credentials() {
    let (d, cfg) = project();
    let p = d.path();
    let (code, _, err) = aieo(p, &["push"], cfg.path());
    assert_eq!(code, 2);
    assert!(err.contains("aieo login"), "{err}");
    let (code, _, err) = aieo(p, &["search", "google", "pull"], cfg.path());
    assert_eq!(code, 2);
    assert!(err.contains("aieo search google login"), "{err}");
    let (code, _, err) = aieo(p, &["search", "bing", "pull"], cfg.path());
    assert_eq!(code, 2);
    assert!(err.contains("BING_WEBMASTER_API_KEY"), "{err}");
}
