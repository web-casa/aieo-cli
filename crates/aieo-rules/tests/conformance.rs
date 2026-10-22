//! Conformance with aieo.ee (`@aieo/rules` fixtures/conformance): every case
//! is a tiny site whose module results the TypeScript engine produced over
//! HTTP. Read as a build directory, it must give the same status, ratio
//! (2 dp) and reason for every module the case lists.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aieo_rules::readiness::{rounded_ratio, run};
use aieo_site::{Site, SiteOptions};
use serde_json::Value;
use url::Url;

const ORIGIN: &str = "https://fixture.test";

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/fixtures/conformance")
}

fn materialize(text: &str) -> String {
    let text = text.replace("{{origin}}", ORIGIN);
    let re = regex::Regex::new(r"\{\{pad:(\d+)\}\}").unwrap();
    re.replace_all(&text, |c: &regex::Captures| {
        "x".repeat(c[1].parse().unwrap())
    })
    .into_owned()
}

fn copy_site(from: &Path, to: &Path) {
    for entry in walkdir(from) {
        let rel = entry.strip_prefix(from).unwrap();
        let dest = to.join(rel);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(
            &dest,
            materialize(&std::fs::read_to_string(&entry).unwrap()),
        )
        .unwrap();
    }
}

fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(walkdir(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn matches_the_typescript_engine() {
    let mut cases: Vec<PathBuf> = std::fs::read_dir(fixtures_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    cases.sort();
    assert!(
        cases.len() >= 30,
        "conformance fixtures missing: run `pnpm --filter @aieo/rules export:cli`"
    );
    let mut failures = Vec::new();
    let mut compared = 0;
    for case in &cases {
        let id = case.file_name().unwrap().to_string_lossy().to_string();
        let meta: Value =
            serde_json::from_str(&std::fs::read_to_string(case.join("case.json")).unwrap())
                .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        copy_site(&case.join("site"), tmp.path());
        let mut overrides: HashMap<String, Vec<(String, String)>> = HashMap::new();
        if let Some(Value::Object(h)) = meta.get("headers") {
            for (path, headers) in h {
                let pairs = headers
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                    .collect();
                overrides.insert(path.clone(), pairs);
            }
        }
        let site = Site::load(
            tmp.path(),
            Url::parse(ORIGIN).unwrap(),
            SiteOptions {
                header_overrides: overrides,
                ..Default::default()
            },
        )
        .unwrap();
        let report = run(&site);
        for (module, expected) in meta["expect"].as_object().unwrap() {
            let actual = report.modules.iter().find(|m| m.id == module).unwrap();
            let want_status = expected["status"].as_str().unwrap();
            let want_ratio = expected["ratio"].as_f64().unwrap();
            let want_reason = expected.get("reason").and_then(Value::as_str);
            let got_ratio = rounded_ratio(actual.ratio);
            compared += 1;
            if actual.status.as_str() != want_status
                || (got_ratio - want_ratio).abs() > 1e-9
                || actual.reason != want_reason
            {
                failures.push(format!(
                    "{id} {module}: want {want_status} {want_ratio} {want_reason:?}, got {} {got_ratio} {:?} — {:?}",
                    actual.status.as_str(),
                    actual.reason,
                    actual.evidence.iter().map(|e| format!("{}: {}", e.label.en, e.detail)).collect::<Vec<_>>()
                ));
            }
        }
    }
    assert_eq!(
        compared,
        cases.len() * 16,
        "every case lists the 16 conformance modules"
    );
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
