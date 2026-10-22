//! Markdown summary (pull-request comments, `$GITHUB_STEP_SUMMARY`).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use aieo_core::bundle::Bundle;
use aieo_core::{Finding, FindingStatus, Lang, Tier};

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

pub fn render(bundle: &Bundle, lang: Lang, per_rule: usize) -> String {
    let zh = lang == Lang::Zh;
    let sum = &bundle.summary;
    let mut out = String::new();
    let verdict = if sum.gate.passed {
        "✅ passed"
    } else {
        "❌ failed"
    };
    let _ = writeln!(out, "## aieo check — {verdict}\n");
    let _ = writeln!(
        out,
        "`{}` · {} · {} {} ({} {})\n",
        bundle.site.origin,
        bundle.site.preset,
        sum.pages,
        if zh { "个页面" } else { "pages" },
        sum.indexable,
        if zh { "个可索引" } else { "indexable" }
    );
    let _ = writeln!(
        out,
        "| | {} | {} | {} |",
        if zh { "错误" } else { "Errors" },
        if zh { "警告" } else { "Warnings" },
        if zh { "提示" } else { "Notices" }
    );
    let _ = writeln!(out, "|---|---:|---:|---:|");
    let f = &sum.findings;
    let _ = writeln!(
        out,
        "| {} | {} | {} | {} |",
        if zh { "全部" } else { "All" },
        f.error,
        f.warning,
        f.notice
    );
    if let Some(n) = &sum.new {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            if zh { "新增" } else { "New" },
            n.error,
            n.warning,
            n.notice
        );
    }
    if let Some(r) = &bundle.readiness {
        let _ = writeln!(
            out,
            "\n{} **{:.1}** ({:.1} + {:.1})",
            if zh {
                "AI 就绪度（构建产物）："
            } else {
                "AI readiness (build):"
            },
            r.scores.final_score,
            r.scores.base,
            r.scores.bonus
        );
        if let Some(mp) = &r.multipage {
            if mp.modules.iter().any(|m| m.status != "not_applicable") {
                let _ = writeln!(
                    out,
                    "{} **{:.1}** ({} {})",
                    if zh {
                        "多页一致性："
                    } else {
                        "Multi-page consistency:"
                    },
                    mp.score,
                    mp.pages,
                    if zh { "页" } else { "pages" }
                );
            }
        }
    }
    if sum.incomplete {
        let _ = writeln!(
            out,
            "\n> ⚠️ {} {}",
            if zh {
                "证据不完整："
            } else {
                "Incomplete evidence:"
            },
            sum.incomplete_reasons.join("; ")
        );
    }

    let compared = bundle.diff.is_some();
    let mut by_rule: BTreeMap<(std::cmp::Reverse<Tier>, &str), Vec<&Finding>> = BTreeMap::new();
    for f in bundle
        .findings
        .iter()
        .filter(|f| f.suppressed.is_none() && f.tier != Tier::Notice)
    {
        if compared && f.status != FindingStatus::New {
            continue;
        }
        by_rule
            .entry((std::cmp::Reverse(f.tier), f.rule.as_str()))
            .or_default()
            .push(f);
    }
    if !by_rule.is_empty() {
        let _ = writeln!(
            out,
            "\n### {}\n",
            if compared {
                if zh {
                    "新增问题"
                } else {
                    "New problems"
                }
            } else if zh {
                "问题"
            } else {
                "Problems"
            }
        );
        let _ = writeln!(
            out,
            "| {} | {} | URL | {} |",
            if zh { "级别" } else { "Tier" },
            if zh { "规则" } else { "Rule" },
            if zh { "证据" } else { "Evidence" }
        );
        let _ = writeln!(out, "|---|---|---|---|");
        for ((tier, rule), list) in &by_rule {
            for f in list.iter().take(per_rule) {
                let evidence = f
                    .evidence
                    .iter()
                    .map(|e| format!("{}: {}", e.label.get(lang), e.detail))
                    .collect::<Vec<_>>()
                    .join("; ");
                let _ = writeln!(
                    out,
                    "| {} | `{}` {} | {} | {} |",
                    tier.0.as_str(),
                    rule,
                    cell(f.message.get(lang)),
                    cell(f.url.as_deref().unwrap_or("")),
                    cell(&evidence)
                );
            }
            if list.len() > per_rule {
                let _ = writeln!(out, "| | `{rule}` | … {} more | |", list.len() - per_rule);
            }
        }
    }
    if let Some(d) = &bundle.diff {
        let _ = writeln!(
            out,
            "\n{} {}",
            if zh {
                "已修复："
            } else {
                "Fixed since the baseline:"
            },
            d.fixed.len()
        );
    }
    out
}
