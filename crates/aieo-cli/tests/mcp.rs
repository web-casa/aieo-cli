//! `aieo mcp` over stdio: the handshake, read-only tools, a check, a check
//! of another project in a monorepo, and a tool error that tells the agent
//! what to run.

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
    write(
        p,
        "apps/docs/aieo.toml",
        "site = \"https://docs.t.example\"\n",
    );
    write(
        p,
        "apps/docs/dist/index.html",
        r#"<!doctype html><html lang="en"><head><title>Docs home</title></head><body><h1>Docs</h1></body></html>"#,
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
    assert!(init["result"]["instructions"]
        .as_str()
        .unwrap()
        .contains("plans"));
    call(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));

    let list = call(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
    let tools = list["result"]["tools"].as_array().unwrap();
    assert!(tools.len() >= 10);
    for t in tools {
        assert_eq!(
            t["annotations"]["readOnlyHint"], true,
            "{} must be read-only",
            t["name"]
        );
    }

    let check = call(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"check","arguments":{"rule":"C-LINK-BROKEN"}}})).unwrap();
    let body: Value =
        serde_json::from_str(check["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["findings_matching"], 1);
    let f = &body["findings"][0];
    assert_eq!(f["rule"], "C-LINK-BROKEN");
    assert_eq!(f["build_file"], "dist/index.html:1");
    assert!(f["why"].as_str().unwrap().contains("internal link"));

    let docs = call(json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"check","arguments":{"project":"apps/docs"}}})).unwrap();
    let body: Value =
        serde_json::from_str(docs["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["site"], "https://docs.t.example");
    assert_eq!(body["pages"], 1);

    // every tool that reads a project honours `project`
    let mut id = 10;
    let mut tool = |name: &str, args: Value| -> Value {
        id += 1;
        let r = call(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}})).unwrap();
        r["result"].clone()
    };
    let text = |r: &Value| r["content"][0]["text"].as_str().unwrap().to_string();
    let page = tool("page", json!({"project":"apps/docs","url":"/"}));
    assert!(text(&page).contains("https://docs.t.example/"), "{page}");
    let push = tool("push_plan", json!({"project":"apps/docs"}));
    assert!(text(&push).contains("https://docs.t.example"), "{push}");
    for name in ["diff", "indexnow_plan"] {
        let r = tool(name, json!({"project":"apps/docs"}));
        assert_eq!(r["isError"], true, "{name}: no baseline in apps/docs");
        assert!(text(&r).contains("aieo snapshot"), "{name}: {r}");
    }
    let logs = tool("logs_report", json!({"project":"apps/docs"}));
    assert!(
        text(&logs).contains(&format!("apps{0}docs{0}.aieo", std::path::MAIN_SEPARATOR)),
        "logs_report looks in the project: {logs}"
    );
    let search = tool("search_report", json!({"project":"apps/docs"}));
    assert_eq!(search["isError"], true);
    assert!(text(&search).contains("aieo search"));

    let logs = call(json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"logs_report","arguments":{}}})).unwrap();
    assert_eq!(logs["result"]["isError"], true);
    assert!(logs["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("aieo logs ingest"));

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success());
}
