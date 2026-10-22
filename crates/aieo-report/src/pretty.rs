//! Terminal output.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use aieo_core::bundle::Bundle;
use aieo_core::spec::spec;
use aieo_core::{Finding, FindingStatus, Lang, Tier};
use aieo_rules::readiness::{ModuleResult, Status};

use crate::Paths;

pub struct Style {
    pub color: bool,
    pub lang: Lang,
    /// findings shown per rule before "… and N more"
    pub per_rule: usize,
    /// list notices one by one instead of one line per rule
    pub verbose: bool,
}

impl Style {
    fn paint(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }
    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
    fn tier(&self, t: Tier) -> String {
        match t {
            Tier::Error => self.paint("1;31", " ERROR "),
            Tier::Warning => self.paint("1;33", " WARN  "),
            Tier::Notice => self.paint("1;36", " NOTE  "),
        }
    }
    fn zh(&self) -> bool {
        self.lang == Lang::Zh
    }
}

fn status_mark(s: Status) -> &'static str {
    match s {
        Status::Pass => "✓",
        Status::Partial => "◐",
        Status::Fail => "✗",
        Status::Info => "·",
        Status::NotApplicable => "–",
        Status::Unverified => "?",
    }
}

fn location(f: &Finding, paths: &Paths) -> String {
    let mut parts = Vec::new();
    if let Some(url) = &f.url {
        parts.push(url.clone());
    }
    if let Some(file) = &f.file {
        let dist = paths.dist.trim_end_matches('/');
        let mut p = if dist.is_empty() || dist == "." {
            file.clone()
        } else {
            format!("{dist}/{file}")
        };
        if let Some(line) = f.line {
            p = format!("{p}:{line}");
        }
        parts.push(p);
    }
    if let Some(src) = &f.source {
        parts.push(format!("← {src}"));
    }
    parts.join("  ")
}

pub fn render(
    bundle: &Bundle,
    modules: &[ModuleResult],
    notes: &[aieo_core::Localized],
    paths: &Paths,
    style: &Style,
) -> String {
    let s = style;
    let mut out = String::new();
    let sum = &bundle.summary;
    let _ = writeln!(
        out,
        "{} {}  ·  {}  ·  {}",
        s.bold("aieo check"),
        bundle.site.origin,
        bundle.site.preset,
        if s.zh() {
            format!("{} 个页面（{} 个可索引）", sum.pages, sum.indexable)
        } else {
            format!("{} pages ({} indexable)", sum.pages, sum.indexable)
        }
    );

    if let Some(r) = &bundle.readiness {
        let sc = r.scores;
        let _ = writeln!(
            out,
            "\n{}  {:.1} + {:.1} {} = {}",
            s.bold(if s.zh() {
                "AI 就绪度（构建产物）"
            } else {
                "AI readiness (build)"
            }),
            sc.base,
            sc.bonus,
            if s.zh() { "加分" } else { "bonus" },
            s.bold(&format!("{:.1}", sc.final_score))
        );
        for note in notes {
            let _ = writeln!(out, "  {}", s.dim(&format!("note: {}", note.get(s.lang))));
        }
        for m in modules.iter().filter(|m| {
            matches!(
                m.status,
                Status::Fail | Status::Partial | Status::Unverified
            )
        }) {
            let rule = spec().expect_rule(m.id);
            let fix = m
                .fix
                .as_ref()
                .map(|f| format!(" — {}", f.get(s.lang)))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "  {} {} {}{}",
                status_mark(m.status),
                m.id,
                rule.title.get(s.lang),
                s.dim(&fix)
            );
        }
        if let Some(mp) = r
            .multipage
            .as_ref()
            .filter(|mp| mp.modules.iter().any(|m| m.status != "not_applicable"))
        {
            let _ = writeln!(
                out,
                "\n{}  {}",
                s.bold(&if s.zh() {
                    format!("多页一致性（{} 页）", mp.pages)
                } else {
                    format!("Multi-page consistency ({} pages)", mp.pages)
                }),
                s.bold(&format!("{:.1}", mp.score))
            );
            for m in mp
                .modules
                .iter()
                .filter(|m| m.status == "fail" || m.status == "partial")
            {
                let status = if m.status == "fail" {
                    Status::Fail
                } else {
                    Status::Partial
                };
                let _ = writeln!(
                    out,
                    "  {} {} {}",
                    status_mark(status),
                    m.id,
                    spec().expect_rule(&m.id).title.get(s.lang)
                );
            }
        }
    }

    let new_note = match &sum.new {
        Some(n) => format!(
            "   {}",
            s.dim(&if s.zh() {
                format!(
                    "新增：{} 错误 · {} 警告 · {} 提示",
                    n.error, n.warning, n.notice
                )
            } else {
                format!(
                    "new: {} errors · {} warnings · {} notices",
                    n.error, n.warning, n.notice
                )
            })
        ),
        None => String::new(),
    };
    let f = &sum.findings;
    let _ = writeln!(
        out,
        "\n{}  {} · {} · {}{}",
        s.bold(if s.zh() { "发现项" } else { "Findings" }),
        s.paint(
            "31",
            &format!("{} {}", f.error, if s.zh() { "错误" } else { "errors" })
        ),
        s.paint(
            "33",
            &format!("{} {}", f.warning, if s.zh() { "警告" } else { "warnings" })
        ),
        s.paint(
            "36",
            &format!("{} {}", f.notice, if s.zh() { "提示" } else { "notices" })
        ),
        new_note
    );

    let mut by_rule: BTreeMap<(std::cmp::Reverse<Tier>, &str), Vec<&Finding>> = BTreeMap::new();
    for f in bundle.findings.iter().filter(|f| f.suppressed.is_none()) {
        by_rule
            .entry((std::cmp::Reverse(f.tier), f.rule.as_str()))
            .or_default()
            .push(f);
    }
    for ((tier, rule), list) in &by_rule {
        let title = list[0].message.get(s.lang);
        let new_count = list
            .iter()
            .filter(|f| f.status == FindingStatus::New)
            .count();
        let new_tag = if new_count > 0 {
            s.paint("1;35", &format!(" +{new_count} new"))
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "\n{} {} {} {}{}",
            s.tier(tier.0),
            s.bold(rule),
            title,
            s.dim(&format!("×{}", list.len())),
            new_tag
        );
        if tier.0 == Tier::Notice && !s.verbose {
            if let Some(first) = list.first() {
                let more = if list.len() > 1 {
                    format!("  (+{} more, --verbose)", list.len() - 1)
                } else {
                    String::new()
                };
                let _ = writeln!(out, "        {}{}", location(first, paths), s.dim(&more));
            }
            continue;
        }
        for f in list.iter().take(s.per_rule) {
            let marker = if f.status == FindingStatus::New {
                s.paint("35", "+ ")
            } else {
                "  ".into()
            };
            let _ = writeln!(out, "      {marker}{}", location(f, paths));
            for e in &f.evidence {
                let _ = writeln!(
                    out,
                    "          {}",
                    s.dim(&format!("{}: {}", e.label.get(s.lang), e.detail))
                );
            }
        }
        if list.len() > s.per_rule {
            let _ = writeln!(
                out,
                "        {}",
                s.dim(&format!("… {} more", list.len() - s.per_rule))
            );
        }
    }

    let suppressed = bundle
        .findings
        .iter()
        .filter(|f| f.suppressed.is_some())
        .count();
    if suppressed > 0 {
        let _ = writeln!(
            out,
            "\n{}",
            s.dim(&format!("{suppressed} finding(s) suppressed in aieo.toml"))
        );
    }
    if let Some(d) = &bundle.diff {
        let _ = writeln!(
            out,
            "{}",
            s.dim(&format!(
                "baseline {} — {} new, {} existing, {} fixed",
                d.baseline_generated_at,
                d.new,
                d.existing,
                d.fixed.len()
            ))
        );
    }
    if sum.incomplete {
        let _ = writeln!(
            out,
            "\n{} {}",
            s.paint(
                "1;31",
                if s.zh() {
                    "证据不完整："
                } else {
                    "Incomplete evidence:"
                }
            ),
            sum.incomplete_reasons.join("; ")
        );
    }
    let gate = if sum.gate.fail_on.is_empty() {
        "never".to_string()
    } else {
        sum.gate.fail_on.join(", ")
    };
    let verdict = if sum.gate.passed {
        s.paint("1;32", "PASSED")
    } else {
        s.paint("1;31", "FAILED")
    };
    let _ = writeln!(
        out,
        "\n{} {verdict}  {}",
        s.bold("Gate"),
        s.dim(&format!("(fail on: {gate})"))
    );
    out
}
