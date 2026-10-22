//! `aieo mcp` over stdio: the handshake, read-only tools, a check and a
//! tool error that tells the agent what to run.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn serves_read_only_tools_over_stdio() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    write(p, "aieo.toml", "site = \"https://t.example\"\n");
    write(
        p,
        "dist/index.html",
        r#"<!doctype html><html lang="en"><head><title>Home page</title><link rel="canonical" href="https://t.example/"></head><body><nav><a href="/missing/">x</a></nav><h1>Home</h1></body></html>"#,
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_aieo"))
        .args(["mcp", "-C"])
        .arg(p)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut call = |msg: Value| -> Option<Value> {
        writeln!(stdin, "{msg}").unwrap();
        msg.get("id")?;
        loop {
            let mut line = String::new();
            assert!(out.read_line(&mut line).unwrap() > 0, "server closed");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v.get("id") == msg.get("id") {
                return Some(v);
            }
        }
    };
    let init = call(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}})).unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "aieo");
    assert!(init["result"]["instructions"].as_str().unwrap().contains("plans"));
    call(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));

    let list = call(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
    let tools = list["result"]["tools"].as_array().unwrap();
    assert!(tools.len() >= 10);
    for t in tools {
        assert_eq!(t["annotations"]["readOnlyHint"], true, "{} must be read-only", t["name"]);
    }

    let check = call(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"check","arguments":{"rule":"C-LINK-BROKEN"}}})).unwrap();
    let body: Value = serde_json::from_str(check["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["findings_matching"], 1);
    let f = &body["findings"][0];
    assert_eq!(f["rule"], "C-LINK-BROKEN");
    assert_eq!(f["build_file"], "dist/index.html:1");
    assert!(f["why"].as_str().unwrap().contains("internal link"));

    let logs = call(json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"logs_report","arguments":{}}})).unwrap();
    assert_eq!(logs["result"]["isError"], true);
    assert!(logs["result"]["content"][0]["text"].as_str().unwrap().contains("aieo logs ingest"));

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success());
}
