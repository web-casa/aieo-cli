//! K (configuration), structured-data and P rules over small projects
//! whose framework config and deployment rules carry known defects.

use std::collections::BTreeSet;

use aieo_core::config::{resolve_settings, Config, Settings};
use aieo_core::spec::spec;
use aieo_rules::readiness::Status;
use aieo_rules::{check, Options, Outcome};
use aieo_site::preset::detect;
use aieo_site::Site;
use url::Url;

const O: &str = "https://docs.example";

fn write(root: &std::path::Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn page(title: &str, canonical: &str, head: &str) -> String {
    format!(
        r#"<!doctype html><html lang="en"><head><title>{title}</title><meta name="description" content="About {title}.">
<link rel="canonical" href="{canonical}">{head}</head><body><nav><a href="/docs/">Home</a> <a href="/docs/guide/">Guide</a></nav>
<h1>{title}</h1><p>Text about {title}.</p></body></html>"#
    )
}

fn settings() -> Settings {
    resolve_settings(
        &Config::default(),
        spec(),
        time::macros::date!(2026 - 10 - 22),
    )
    .unwrap()
}

/// Run the checks the way `aieo check` does for the project at `root`.
fn run(root: &std::path::Path) -> Outcome {
    let project = detect(root, None);
    let dist = root.join(&project.default_dist);
    let build = project.build(&dist);
    let rendering = build.rendering.unwrap_or(project.rendering);
    let site = Site::load(&dist, Url::parse(O).unwrap(), build.options).unwrap();
    let s = settings();
    check(
        &site,
        &Options {
            settings: &s,
            project: Some(&project),
            rendering: Some(rendering),
            baseline: None,
        },
    )
}

/// (rule, where: `file:line` for config findings, else the URL path)
fn hits(o: &Outcome) -> BTreeSet<(String, String)> {
    o.findings
        .iter()
        .map(|f| {
            let at = match (&f.source, &f.file, f.line) {
                (Some(src), None, Some(l)) => format!("{src}:{l}"),
                (_, Some(file), Some(l)) if f.url.is_none() => format!("{file}:{l}"),
                _ => f.url.clone().unwrap_or_default().replace(O, ""),
            };
            (f.rule.clone(), at)
        })
        .collect()
}

fn astro_project() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(r, "package.json", r#"{"dependencies":{"astro":"5.0.0"}}"#);
    write(
        r,
        "astro.config.mjs",
        "export default defineConfig({\n  site: 'https://other.example',\n  base: '/docs',\n  trailingSlash: 'always',\n});\n",
    );
    write(
        r,
        "vercel.json",
        r#"{
  "redirects": [
    { "source": "/docs/a", "destination": "/docs/b" },
    { "source": "/docs/b", "destination": "/docs/a" },
    { "source": "/docs/c", "destination": "/docs/nowhere/" },
    { "source": "/docs/e", "destination": "/docs/moved/" },
    { "source": "/docs/moved", "destination": "/docs/guide/" }
  ],
  "headers": [
    { "source": "/docs/(.*)", "headers": [{ "key": "X-Robots-Tag", "value": "noindex" }] },
    { "source": "/docs/drafts/(.*)", "headers": [{ "key": "X-Robots-Tag", "value": "noindex" }] }
  ]
}"#,
    );
    let jsonld = r#"<script type="application/ld+json">{"@context":"https://schema.org","@type":"Product","name":"Widget"}</script>
<script type="application/ld+json">{"@type":"Organization","name":"Example"}</script>
<script type="application/ld+json">{"@context":"https://schema.org","@type":"HowTo","name":"Do it"}</script>"#;
    write(
        r,
        "dist/index.html",
        &page("Home", &format!("{O}/docs/"), jsonld),
    );
    write(
        r,
        "dist/guide/index.html",
        &page("Guide", &format!("{O}/docs/guide"), ""),
    );
    write(
        r,
        "dist/old/index.html",
        &page("Old", &format!("{O}/old/"), ""),
    );
    write(
        r,
        "dist/drafts/x/index.html",
        &page("Draft", &format!("{O}/docs/drafts/x/"), ""),
    );
    write(
        r,
        "dist/sitemap.xml",
        &format!(
            "<urlset><url><loc>{O}/docs/</loc></url><url><loc>{O}/docs/guide</loc></url></urlset>"
        ),
    );
    d
}

#[test]
fn reports_configuration_defects_at_their_lines() {
    let d = astro_project();
    let o = run(d.path());
    let h = hits(&o);
    for (rule, at) in [
        ("K-SITE-ORIGIN-MISMATCH", "astro.config.mjs:2"),
        ("K-BASE-PATH-MISMATCH", "astro.config.mjs:3"),
        ("K-TRAILING-SLASH-MISMATCH", "astro.config.mjs:4"),
        ("K-REDIRECT-LOOP", "vercel.json:3"),
        ("K-REDIRECT-TARGET-MISSING", "vercel.json:5"),
        ("K-REDIRECT-CHAIN", "vercel.json:6"),
        ("K-HEADERS-NOINDEX", "vercel.json:10"),
        ("T-JSONLD-MISSING-REQUIRED", "/docs/"),
        ("T-JSONLD-NO-CONTEXT", "/docs/"),
        ("T-JSONLD-RETIRED-TYPE", "/docs/"),
    ] {
        assert!(
            h.contains(&(rule.into(), at.into())),
            "missing {rule} at {at}: {h:#?}"
        );
    }
    // the drafts-only header rule is deliberate: no sitemap page, not the homepage
    assert!(!h.contains(&("K-HEADERS-NOINDEX".into(), "vercel.json:11".into())));
    let redirects: Vec<_> = o
        .findings
        .iter()
        .filter(|f| f.rule == "K-REDIRECT-LOOP")
        .collect();
    assert_eq!(redirects.len(), 1, "a loop is reported once");
    assert_eq!(redirects[0].source.as_deref(), Some("vercel.json"));
    assert_eq!(redirects[0].line, Some(3));
    let trailing = o
        .findings
        .iter()
        .find(|f| f.rule == "K-TRAILING-SLASH-MISMATCH")
        .unwrap();
    let details: Vec<&str> = trailing
        .evidence
        .iter()
        .map(|e| e.detail.as_str())
        .collect();
    assert!(
        details
            .iter()
            .any(|d| d.starts_with("1: https://docs.example/docs/guide")),
        "{details:?}"
    );
}

#[test]
fn spots_a_client_rendered_shell() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(
        r,
        "dist/index.html",
        &format!(
            r#"<!doctype html><html lang="en"><head><title>App</title><link rel="canonical" href="{O}/"></head><body><div id="app"></div><script src="/app.js"></script></body></html>"#
        ),
    );
    write(r, "dist/_redirects", "/* /index.html 200\n");
    write(
        r,
        "dist/robots.txt",
        &format!("User-agent: *\nAllow: /\nSitemap: {O}/sitemap.xml\n"),
    );
    write(
        r,
        "dist/sitemap.xml",
        &format!("<urlset><url><loc>{O}/</loc></url><url><loc>{O}/pricing</loc></url><url><loc>{O}/about</loc></url></urlset>"),
    );
    let o = run(r);
    let shell = o
        .findings
        .iter()
        .find(|f| f.rule == "K-SPA-SHELL")
        .expect("K-SPA-SHELL");
    assert_eq!(shell.file.as_deref(), Some("index.html"));
    assert!(
        shell
            .evidence
            .iter()
            .any(|e| e.detail.starts_with("2: /about, /pricing")),
        "{:?}",
        shell.evidence
    );
    // robots.txt and the sitemap are real files, not the shell
    assert!(o.robots_txt);
    assert_eq!(o.sitemaps, vec![format!("{O}/sitemap.xml")]);
}

#[test]
fn multipage_modules_judge_consistency() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let org = |name: &str, logo: &str| {
        format!(
            r#"<script type="application/ld+json">{{"@context":"https://schema.org","@type":"Organization","name":"{name}","logo":"{logo}"}}</script>"#
        )
    };
    let body = "<p>".to_string() + &"Words to read here. ".repeat(30) + "</p>";
    let mk = |title: &str, path: &str, head: &str| {
        format!(
            r#"<!doctype html><html lang="en"><head><title>{title}</title><meta name="description" content="Same description."><link rel="canonical" href="{O}{path}">{head}</head>
<body><nav><a href="/">Home</a> <a href="/a/">A</a></nav><h1>{title}</h1>{body}</body></html>"#
        )
    };
    write(
        r,
        "dist/index.html",
        &mk("Home", "/", &org("Example Inc", "https://e/logo.png")),
    );
    write(
        r,
        "dist/a/index.html",
        &mk(
            "Same",
            "/a/",
            &org("Example Incorporated", "https://e/other.png"),
        ),
    );
    write(r, "dist/b/index.html", &mk("Same", "/b/", ""));
    write(r, "dist/c/index.html", &mk("Other", "/c/", ""));
    let o = run(r);
    let status = |id: &str| {
        o.multipage
            .modules
            .iter()
            .find(|m| m.id == id)
            .unwrap()
            .status
    };
    assert_eq!(o.multipage.pages, 4);
    // 1 duplicate title + 3 duplicate descriptions over a budget of 8
    assert_eq!(status("P01"), Status::Partial);
    assert_eq!(status("P02"), Status::Pass);
    assert_eq!(status("P03"), Status::NotApplicable);
    // entity on 2/4 pages, two names and two logos: 0.5 - 0.3 - 0.2
    assert_eq!(status("P04"), Status::Fail);
    assert!(o.multipage.score > 0.0 && o.multipage.score < 100.0);
    let p04 = o
        .findings
        .iter()
        .find(|f| f.rule == "P04")
        .expect("P04 finding");
    assert!(
        p04.evidence
            .iter()
            .any(|e| e.detail == "example incorporated, example inc"),
        "{:?}",
        p04.evidence
    );
}

#[test]
fn hugo_and_next_origin_findings_point_at_the_declaring_line() {
    // Hugo keys are case-insensitive (stored lower-case): baseURL still has a line
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(
        r,
        "hugo.toml",
        "title = 'x'\nbaseURL = 'https://staging.example/docs/'\n",
    );
    write(r, "content/_index.md", "# x\n");
    write(
        r,
        "public/index.html",
        &page("Home", &format!("{O}/docs/"), ""),
    );
    let h = hits(&run(r));
    assert!(
        h.contains(&("K-SITE-ORIGIN-MISMATCH".into(), "hugo.toml:2".into())),
        "{h:#?}"
    );

    // Next.js: the origin comes from metadataBase in the root layout
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(
        r,
        "next.config.mjs",
        "export default { output: 'export' };\n",
    );
    write(
        r,
        "app/layout.tsx",
        "import type { Metadata } from 'next';\n\nexport const metadata: Metadata = {\n  metadataBase: new URL('https://staging.example'),\n};\n",
    );
    write(r, "out/index.html", &page("Home", &format!("{O}/"), ""));
    let h = hits(&run(r));
    assert!(
        h.contains(&("K-SITE-ORIGIN-MISMATCH".into(), "app/layout.tsx:4".into())),
        "{h:#?}"
    );
}

#[test]
fn netlify_shadowing_and_header_removal_are_not_defects() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(r, "dist/index.html", &page("Home", &format!("{O}/"), ""));
    write(r, "dist/a/index.html", &page("A", &format!("{O}/a/"), ""));
    write(r, "dist/b/index.html", &page("B", &format!("{O}/b/"), ""));
    write(
        r,
        "dist/sitemap.xml",
        &format!("<urlset><url><loc>{O}/</loc></url><url><loc>{O}/b/</loc></url></urlset>"),
    );
    // /a → /b is shadowed by b/index.html (unforced); /c ↔ /d really loops
    write(
        r,
        "dist/_redirects",
        "/a/ /b/ 301\n/b/ /a/ 301\n/c/ /d/ 301!\n/d/ /c/ 301!\n",
    );
    // noindex everywhere, then removed again for the whole site but /drafts
    write(
        r,
        "dist/_headers",
        "/*\n  X-Robots-Tag: noindex\n/*\n  ! X-Robots-Tag\n",
    );
    let o = run(r);
    let loops: Vec<_> = o
        .findings
        .iter()
        .filter(|f| f.rule == "K-REDIRECT-LOOP")
        .map(|f| f.url.clone().unwrap_or_default())
        .collect();
    assert_eq!(loops.len(), 1, "{loops:?}");
    // the shadowed /a/ → /b/ rule never applies, so it is no chain either
    assert!(
        !o.findings.iter().any(|f| f.rule == "K-REDIRECT-CHAIN"
            && f.url
                .as_deref()
                .is_some_and(|u| u.ends_with("/a/") || u.ends_with("/b/"))),
        "{:#?}",
        o.findings
            .iter()
            .filter(|f| f.rule.starts_with("K-REDIRECT"))
            .collect::<Vec<_>>()
    );
    assert!(
        loops[0].ends_with("/c/") || loops[0].ends_with("/d/"),
        "{loops:?}"
    );
    assert!(
        !o.findings.iter().any(|f| f.rule == "K-HEADERS-NOINDEX"),
        "the header is removed again"
    );
}
