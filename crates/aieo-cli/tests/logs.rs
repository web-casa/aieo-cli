//! `aieo logs`: every log format through ingest, a report that validates
//! against the bundle schema, gaps and referrals, and `export` carrying logs.

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

const GPT: &str = "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; GPTBot/1.2; +https://openai.com/gptbot)";
const OAI: &str = "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; OAI-SearchBot/1.3; +https://openai.com/searchbot)";

fn project() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    let page = |t: &str, path: &str| {
        format!(
            r#"<!doctype html><html lang="en"><head><title>{t} page title</title><link rel="canonical" href="https://t.example{path}"></head><body><nav><a href="/">Home</a><a href="/a/">A</a><a href="/b/">B</a></nav><h1>{t}</h1></body></html>"#
        )
    };
    write(p, "dist/index.html", &page("Home", "/"));
    write(p, "dist/a/index.html", &page("A", "/a/"));
    write(p, "dist/b/index.html", &page("B", "/b/"));
    write(p, "dist/robots.txt", "User-agent: *\nAllow: /\n");
    write(p, "aieo.toml", "site = \"https://t.example\"\n");
    // one day of traffic in every supported format
    write(
        p,
        "logs/nginx.log",
        &format!(
            "1.2.3.4 - - [20/Oct/2026:10:00:00 +0000] \"GET /a/ HTTP/1.1\" 200 10 \"https://chatgpt.com/\" \"Mozilla/5.0 Chrome/141\"\n\
             20.171.207.5 - - [20/Oct/2026:10:00:01 +0000] \"GET /a/ HTTP/1.1\" 403 10 \"-\" \"{GPT}\"\n"
        ),
    );
    write(
        p,
        "logs/caddy.json",
        &(format!(
            r#"{{"level":"info","ts":1792836002.5,"logger":"http.log.access","msg":"handled request","request":{{"remote_ip":"104.210.140.130","client_ip":"104.210.140.130","method":"GET","host":"t.example","uri":"/","headers":{{"User-Agent":["{OAI}"]}}}},"size":10,"status":200}}"#
        ) + "\n"),
    );
    write(
        p,
        "logs/cf.ndjson",
        &(format!(
            r#"{{"ClientIP":"20.171.207.6","ClientRequestHost":"t.example","ClientRequestMethod":"GET","ClientRequestURI":"/b/","ClientRequestUserAgent":"{GPT}","EdgeResponseStatus":403,"EdgeResponseBytes":5,"EdgeStartTimestamp":"2026-10-20T10:00:03Z","RayID":"r1"}}"#
        ) + "\n"),
    );
    write(
        p,
        "logs/vercel.ndjson",
        &(r#"{"id":"1","deploymentId":"d","source":"build","message":"x"}"#.to_string()
            + "\n"
            + r#"{"id":"2","deploymentId":"d","source":"static","host":"t.vercel.app","timestamp":1792836004000,"requestId":"q1","proxy":{"timestamp":1792836004000,"method":"GET","host":"t.example","path":"/b/?x=1","userAgent":["Mozilla/5.0 Safari"],"referer":"https://www.perplexity.ai/","statusCode":200,"clientIp":"1.2.3.5","vercelCache":"HIT"}}"#
            + "\n"),
    );
    write(
        p,
        "logs/cloudfront.log",
        "#Version: 1.0\n#Fields: date time x-edge-location sc-bytes c-ip cs-method cs(Host) cs-uri-stem sc-status cs(Referer) cs(User-Agent) cs-uri-query cs(Cookie) x-edge-result-type x-edge-request-id x-host-header\n\
         2026-10-20\t10:00:05\tLAX1\t392\t66.249.66.1\tGET\td1.cloudfront.net\t/a/\t200\t-\tMozilla/5.0%20(compatible;%20Googlebot/2.1)\t-\t-\tHit\tid1\tt.example\n",
    );
    write(
        p,
        "logs/other-host.log",
        "1.2.3.4 - - [20/Oct/2026:10:00:00 +0000] \"GET / HTTP/1.1\" 200 1 \"-\" \"x\"\n",
    );
    d
}

#[test]
fn ingests_every_format_and_reports() {
    let d = project();
    let p = d.path();
    let (code, _, err) = aieo(p, &["logs", "ingest", "logs", "--offline"]);
    assert_eq!(code, 0, "{err}");
    for f in [
        "nginx.log: combined",
        "caddy.json: caddy",
        "cf.ndjson: cloudflare",
        "vercel.ndjson: vercel",
        "cloudfront.log: cloudfront",
    ] {
        assert!(err.contains(f), "{f} missing in:\n{err}");
    }
    assert!(p.join(".aieo/salt").is_file());
    assert!(std::fs::read_to_string(p.join(".aieo/.gitignore"))
        .unwrap()
        .contains("logs.db"));

    let (code, out, err) = aieo(p, &["logs", "report", "--format", "json"]);
    assert_eq!(code, 0, "{err}");
    let bundle: serde_json::Value = serde_json::from_str(&out).unwrap();
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../spec/bundle.schema.json")).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&bundle)
        .map(|e| format!("{e} at {}", e.instance_path()))
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(bundle["run"]["kinds"], serde_json::json!(["logs"]));
    assert_eq!(bundle["privacy"]["ip"], "salted-hash");
    let logs = &bundle["logs"];
    let referrals: Vec<&str> = logs["referrals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["platform"].as_str().unwrap())
        .collect();
    assert_eq!(referrals.len(), 2);
    assert!(referrals.contains(&"chatgpt") && referrals.contains(&"perplexity"));
    let cov = |actor: &str| {
        logs["coverage"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["actor"] == actor && c["bot"].is_null())
            .unwrap()["crawled"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(
        (cov("search_index"), cov("classic_search"), cov("training")),
        (1, 1, 0)
    );
    assert!(!out.contains("1.2.3.4"), "no raw IPs in the report");

    let (code, out, _) = aieo(p, &["logs", "gaps"]);
    assert_eq!(code, 0);
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        vec!["https://t.example/a/", "https://t.example/b/"]
    );

    let (code, out, _) = aieo(p, &["logs", "report"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("GPTBot") && out.contains("OAI-SearchBot"),
        "{out}"
    );

    let (code, _, err) = aieo(p, &["export"]);
    assert_eq!(code, 0, "{err}");
    let exported = serde_json::to_value(
        aieo_core::bundle::Bundle::read(&p.join(".aieo/bundle.json.gz")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        exported["run"]["kinds"],
        serde_json::json!(["check", "logs"])
    );
    assert!(validator.is_valid(&exported));
}

#[test]
fn reports_need_logs_and_formats_need_requests() {
    let d = project();
    let p = d.path();
    let (code, _, err) = aieo(p, &["logs", "report"]);
    assert_eq!(code, 2);
    assert!(err.contains("aieo logs ingest"), "{err}");
    write(p, "junk.log", "hello\nworld\n");
    let (code, _, err) = aieo(p, &["logs", "ingest", "junk.log", "--offline"]);
    assert_eq!(code, 2);
    assert!(err.contains("unknown log format"), "{err}");
    let (code, _, err) = aieo(
        p,
        &[
            "logs",
            "ingest",
            "junk.log",
            "--offline",
            "--format",
            "nginx:$remote_addr [$time_local] \"$request\" $status",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("no request could be read"), "{err}");
}
