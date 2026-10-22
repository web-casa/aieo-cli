//! Snapshot tests over real framework builds (`fixtures/presets/<name>`:
//! the same four-page site built with each framework, assets stripped).
//! Each snapshot records the detected preset, exit code, page → source
//! mapping, multi-page modules and findings. `UPDATE_SNAPSHOTS=1` rewrites
//! them; review the diff like code.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/presets")
}

fn summarize(dir: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_aieo"))
        .args(["check", "--site", "https://example.com", "--format", "json"])
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env_remove("GITHUB_STEP_SUMMARY")
        .env_remove("GITHUB_ACTIONS")
        .output()
        .unwrap();
    let code = out.status.code().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let b: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{}: no JSON ({e}); stderr: {stderr}", dir.display()));
    let mut s = String::new();
    writeln!(s, "preset: {}", b["site"]["preset"].as_str().unwrap()).unwrap();
    writeln!(s, "exit: {code}").unwrap();
    for line in stderr.lines().filter(|l| l.starts_with("note:")) {
        writeln!(s, "{line}").unwrap();
    }
    for r in b["summary"]["incomplete_reasons"].as_array().unwrap() {
        writeln!(s, "incomplete: {}", r.as_str().unwrap()).unwrap();
    }
    writeln!(s, "pages:").unwrap();
    for p in b["inventory"]["pages"].as_array().unwrap() {
        writeln!(
            s,
            "  {} {} <- {}{}{}",
            p["url"]
                .as_str()
                .unwrap()
                .trim_start_matches("https://example.com"),
            p["file"].as_str().unwrap(),
            p["source"].as_str().unwrap_or("-"),
            if p["indexable"].as_bool().unwrap() {
                ""
            } else {
                " [not indexable]"
            },
            if p["in_sitemap"].as_bool().unwrap() {
                " [sitemap]"
            } else {
                ""
            },
        )
        .unwrap();
    }
    let mp = &b["readiness"]["multipage"];
    write!(
        s,
        "multipage: {} over {} page(s):",
        mp["score"], mp["pages"]
    )
    .unwrap();
    for m in mp["modules"].as_array().unwrap() {
        write!(
            s,
            " {}={}",
            m["id"].as_str().unwrap(),
            m["status"].as_str().unwrap()
        )
        .unwrap();
    }
    writeln!(s).unwrap();
    writeln!(s, "findings:").unwrap();
    for f in b["findings"].as_array().unwrap() {
        let at = match (f["source"].as_str(), f["file"].as_str()) {
            (Some(src), None) => match f["line"].as_u64() {
                Some(l) => format!("{src}:{l}"),
                None => src.to_string(),
            },
            (_, Some(file)) => file.to_string(),
            _ => "-".into(),
        };
        writeln!(
            s,
            "  {} {} {} @ {}",
            f["tier"].as_str().unwrap(),
            f["rule"].as_str().unwrap(),
            f["url"]
                .as_str()
                .unwrap_or("-")
                .trim_start_matches("https://example.com"),
            at
        )
        .unwrap();
    }
    s
}

#[test]
fn framework_builds_match_their_snapshots() {
    let root = fixtures();
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert!(names.len() >= 9, "fixtures missing: {names:?}");
    let update = std::env::var_os("UPDATE_SNAPSHOTS").is_some();
    let mut failed = Vec::new();
    for name in &names {
        let got = summarize(&root.join(name));
        let snap = root.join(format!("{name}.snap"));
        if update {
            std::fs::write(&snap, &got).unwrap();
            continue;
        }
        let want = std::fs::read_to_string(&snap).unwrap_or_default();
        if got != want {
            eprintln!("--- {name}: expected\n{want}--- got\n{got}");
            failed.push(name.clone());
        }
    }
    assert!(
        failed.is_empty(),
        "snapshots differ: {failed:?} (UPDATE_SNAPSHOTS=1 to accept)"
    );
}
