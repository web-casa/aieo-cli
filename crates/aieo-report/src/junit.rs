//! JUnit XML, for CI systems that show test reports on merge requests
//! (GitLab `artifacts:reports:junit`, Jenkins, Azure DevOps, …). One test
//! case per error or warning finding, as a failure; notices are left out.

use std::fmt::Write as _;

use aieo_core::bundle::Bundle;
use aieo_core::{Lang, Tier};

use crate::Paths;

fn esc(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(*c as u32, 0..=8 | 11 | 12 | 14..=31))
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn render(bundle: &Bundle, paths: &Paths) -> String {
    let cases: Vec<_> = bundle
        .findings
        .iter()
        .filter(|f| f.tier >= Tier::Warning && f.suppressed.is_none())
        .collect();
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        out,
        "<testsuites name=\"aieo\" tests=\"{}\" failures=\"{}\">",
        cases.len().max(1),
        cases.len()
    );
    let _ = writeln!(
        out,
        "  <testsuite name=\"aieo {}\" tests=\"{n}\" failures=\"{f}\" timestamp=\"{}\">",
        esc(&bundle.site.origin),
        esc(&bundle.generated_at),
        n = cases.len().max(1),
        f = cases.len()
    );
    if cases.is_empty() {
        let _ = writeln!(
            out,
            "    <testcase classname=\"aieo\" name=\"no errors or warnings\"/>"
        );
    }
    for f in cases {
        let at = paths
            .locate(f)
            .map(|(p, l)| match l {
                Some(l) => format!("{p}:{l}"),
                None => p,
            })
            .unwrap_or_default();
        let name = match (&f.url, at.is_empty()) {
            (Some(u), false) => format!("{u} ({at})"),
            (Some(u), true) => u.clone(),
            (None, _) => at.clone(),
        };
        let body: Vec<String> = f
            .evidence
            .iter()
            .map(|e| format!("{}: {}", e.label.get(Lang::En), e.detail))
            .collect();
        let _ = writeln!(
            out,
            "    <testcase classname=\"{}\" name=\"{}\"{}>",
            esc(&f.rule),
            esc(&name),
            if at.is_empty() {
                String::new()
            } else {
                format!(" file=\"{}\"", esc(at.split(':').next().unwrap_or("")))
            }
        );
        let _ = writeln!(
            out,
            "      <failure type=\"{}\" message=\"{}\">{}</failure>",
            f.tier.as_str(),
            esc(f.message.get(Lang::En)),
            esc(&body.join("\n"))
        );
        let _ = writeln!(out, "    </testcase>");
    }
    out.push_str("  </testsuite>\n</testsuites>\n");
    out
}
