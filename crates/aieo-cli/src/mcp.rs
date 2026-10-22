//! `aieo mcp`: the CLI as an MCP server on stdio, for coding agents.
//!
//! Every tool reads: the build check, the diff against the baseline, one
//! page, the rules, the access-log and search reports. Nothing here sends
//! anything anywhere — the tools for IndexNow and aieo.ee return a plan and
//! the command a person runs to apply it.

use std::path::PathBuf;

use anyhow::Result;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use aieo_core::bundle::Bundle;
use aieo_core::config::Config;
use aieo_core::spec::spec;
use aieo_core::{Finding, Lang, Tier};

use crate::{load_baseline, run_check, Target};

#[derive(Clone)]
pub struct Aieo {
    root: PathBuf,
    tool_router: ToolRouter<Aieo>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CheckParams {
    /// Build directory relative to the project (default: aieo.toml or the framework's)
    pub dist: Option<String>,
    /// Production origin, e.g. https://example.com (default: aieo.toml / framework config)
    pub site: Option<String>,
    /// Only findings of this tier or higher: error, warning or notice
    pub tier: Option<String>,
    /// Only this rule id, e.g. C-LINK-BROKEN
    pub rule: Option<String>,
    /// Only findings whose URL contains this text
    pub url_contains: Option<String>,
    /// At most this many findings (default 50)
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PageParams {
    /// URL or path of the page, e.g. /guide/ or https://example.com/guide/
    pub url: String,
    pub dist: Option<String>,
    pub site: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuleParams {
    /// Rule id, e.g. K-TRAILING-SLASH-MISMATCH
    pub rule: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RulesParams {
    /// Only this family: F, P, T, C, K, L, S, D
    pub family: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct LogsParams {
    /// Days back from the newest logged request (default 30)
    pub since_days: Option<u32>,
    /// For gaps: search_index (default), classic_search, user_fetch or training
    pub actor: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Days in the window (default 28)
    pub days: Option<i64>,
}

fn tier_of(s: &str) -> Option<Tier> {
    match s {
        "error" => Some(Tier::Error),
        "warning" => Some(Tier::Warning),
        "notice" => Some(Tier::Notice),
        _ => None,
    }
}

/// A finding as an agent needs it: where, what, why, how to fix.
pub fn finding_json(f: &Finding, dist: &str) -> Value {
    let file = f.file.as_ref().map(|file| {
        let d = dist.trim_end_matches('/');
        let p = if d.is_empty() || d == "." {
            file.clone()
        } else {
            format!("{d}/{file}")
        };
        match f.line {
            Some(l) => format!("{p}:{l}"),
            None => p,
        }
    });
    let source = f.source.as_ref().map(|s| match (f.file.is_none(), f.line) {
        (true, Some(l)) => format!("{s}:{l}"),
        _ => s.clone(),
    });
    let rule = spec().rule(&f.rule);
    json!({
        "rule": f.rule,
        "tier": f.tier.as_str(),
        "status": format!("{:?}", f.status).to_lowercase(),
        "title": f.message.get(Lang::En),
        "url": f.url,
        "build_file": file,
        "source_file": source,
        "evidence": f.evidence.iter().map(|e| format!("{}: {}", e.label.get(Lang::En), e.detail)).collect::<Vec<_>>(),
        "why": rule.map(|r| r.description.get(Lang::En).to_string()),
        "reference": rule.and_then(|r| r.source_url.clone()),
        "evidence_level": f.evidence_level,
        "suppressed": f.suppressed.is_some(),
    })
}

fn filtered(findings: &[Finding], p: &CheckParams) -> (Vec<Finding>, usize) {
    let min = p.tier.as_deref().and_then(tier_of);
    let all: Vec<Finding> = findings
        .iter()
        .filter(|f| min.is_none_or(|t| f.tier >= t))
        .filter(|f| {
            p.rule
                .as_deref()
                .is_none_or(|r| f.rule.eq_ignore_ascii_case(r))
        })
        .filter(|f| {
            p.url_contains
                .as_deref()
                .is_none_or(|u| f.url.as_deref().is_some_and(|x| x.contains(u)))
        })
        .cloned()
        .collect();
    let total = all.len();
    (all.into_iter().take(p.limit.unwrap_or(50)).collect(), total)
}

fn summary(b: &Bundle, findings: &[Finding], total: usize, dist: &str, notes: &[String]) -> Value {
    let s = &b.summary;
    json!({
        "site": b.site.origin,
        "preset": b.site.preset,
        "pages": s.pages,
        "indexable": s.indexable,
        "gate": { "passed": s.gate.passed, "fail_on": s.gate.fail_on },
        "findings_by_tier": { "error": s.findings.error, "warning": s.findings.warning, "notice": s.findings.notice },
        "readiness_score": b.readiness.as_ref().map(|r| r.scores.final_score),
        "multipage_score": b.readiness.as_ref().and_then(|r| r.multipage.as_ref().map(|m| m.score)),
        "incomplete": s.incomplete_reasons,
        "notes": notes,
        "findings_shown": findings.len(),
        "findings_matching": total,
        "findings": findings.iter().map(|f| finding_json(f, dist)).collect::<Vec<_>>(),
    })
}

fn pretty(v: Value) -> String {
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

impl Aieo {
    fn target(&self, dist: Option<&str>, site: Option<&str>) -> Target {
        Target {
            dist: dist.map(|d| self.root.join(d)),
            project: self.root.clone(),
            site: site.map(String::from),
            preset: None,
            config: None,
        }
    }
}

async fn blocking<F>(f: F) -> Result<String, String>
where
    F: FnOnce() -> Result<String> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => Err(format!("{e:#}")),
        Err(e) => Err(e.to_string()),
    }
}

#[tool_router]
impl Aieo {
    pub fn new(root: PathBuf) -> Aieo {
        Aieo {
            root,
            tool_router: Self::tool_router(),
        }
    }

    /// Check the site's build output (sitemap, robots.txt, canonical, hreflang, links, llms.txt,
    /// structured data, framework and hosting config) and return the gate, scores and findings
    /// with the build file and source file to change. Build the site first.
    #[tool(annotations(title = "Check the build", read_only_hint = true))]
    async fn check(&self, Parameters(p): Parameters<CheckParams>) -> Result<String, String> {
        let target = self.target(p.dist.as_deref(), p.site.as_deref());
        blocking(move || {
            let run = run_check(&target, None, Vec::new(), "check")?;
            let (shown, total) = filtered(&run.bundle.findings, &p);
            Ok(pretty(summary(
                &run.bundle,
                &shown,
                total,
                &run.prepared.dist_rel,
                &run.prepared.notes,
            )))
        })
        .await
    }

    /// Compare the build with the baseline (`aieo snapshot`): findings that are new, and those fixed.
    #[tool(annotations(title = "Diff against the baseline", read_only_hint = true))]
    async fn diff(&self, Parameters(p): Parameters<CheckParams>) -> Result<String, String> {
        let target = self.target(p.dist.as_deref(), p.site.as_deref());
        let root = self.root.clone();
        blocking(move || {
            let (config, _) = Config::load_or_default(&root)?;
            let baseline = load_baseline(None, config.check.baseline.as_deref(), &root, false)?
                .or_else(|| Bundle::read(&root.join(".aieo/baseline.json")).ok())
                .ok_or_else(|| {
                    anyhow::anyhow!("no baseline: run `aieo snapshot` on a known-good build first")
                })?;
            let run = run_check(&target, Some(&baseline), Vec::new(), "diff")?;
            let new: Vec<Finding> = run
                .bundle
                .findings
                .iter()
                .filter(|f| f.status == aieo_core::FindingStatus::New)
                .cloned()
                .collect();
            let (shown, total) = filtered(&new, &p);
            let mut v = summary(
                &run.bundle,
                &shown,
                total,
                &run.prepared.dist_rel,
                &run.prepared.notes,
            );
            v["fixed"] = json!(run.bundle.diff.as_ref().map(|d| d
                .fixed
                .iter()
                .map(|f| json!({"rule": f.rule, "url": f.url}))
                .collect::<Vec<_>>()));
            Ok(pretty(v))
        })
        .await
    }

    /// Everything known about one page: its inventory record (indexable, canonical, hreflang,
    /// sitemap, inbound links, source file) and its findings.
    #[tool(annotations(title = "One page", read_only_hint = true))]
    async fn page(&self, Parameters(p): Parameters<PageParams>) -> Result<String, String> {
        let target = self.target(p.dist.as_deref(), p.site.as_deref());
        blocking(move || {
            let run = run_check(&target, None, Vec::new(), "check")?;
            let origin = run.bundle.site.origin.clone();
            let want = if p.url.starts_with('/') {
                format!("{origin}{}", p.url)
            } else {
                p.url.clone()
            };
            let same = |u: &str| u.trim_end_matches('/') == want.trim_end_matches('/');
            let record = run.bundle.inventory.pages.iter().find(|r| same(&r.url));
            let findings: Vec<Value> = run
                .bundle
                .findings
                .iter()
                .filter(|f| f.url.as_deref().is_some_and(same))
                .map(|f| finding_json(f, &run.prepared.dist_rel))
                .collect();
            if record.is_none() && findings.is_empty() {
                anyhow::bail!("no page {want} in the build");
            }
            Ok(pretty(json!({ "page": record, "findings": findings })))
        })
        .await
    }

    /// What a rule checks, why, its tier and its source.
    #[tool(annotations(title = "Explain a rule", read_only_hint = true))]
    async fn explain_rule(&self, Parameters(p): Parameters<RuleParams>) -> Result<String, String> {
        let r = spec()
            .rules
            .iter()
            .find(|r| r.id.eq_ignore_ascii_case(&p.rule))
            .ok_or_else(|| format!("unknown rule {}", p.rule))?;
        Ok(pretty(json!({
            "id": r.id,
            "title": r.title.get(Lang::En),
            "description": r.description.get(Lang::En),
            "tier": r.tier.default.map(Tier::as_str),
            "tier_when_failing": r.tier.fail.map(Tier::as_str),
            "evidence_level": r.evidence_level,
            "source": r.source_url,
            "runs_here": r.implemented_here(),
            "default_enabled": r.default_enabled,
        })))
    }

    /// The rules this CLI runs (id, tier, title), optionally one family.
    #[tool(annotations(title = "List rules", read_only_hint = true))]
    async fn list_rules(&self, Parameters(p): Parameters<RulesParams>) -> Result<String, String> {
        let rules: Vec<Value> = spec()
            .rules
            .iter()
            .filter(|r| r.implemented_here())
            .filter(|r| {
                p.family
                    .as_deref()
                    .is_none_or(|f| r.family.eq_ignore_ascii_case(f))
            })
            .map(|r| {
                json!({
                    "id": r.id,
                    "tier": r.tier.default.or(r.tier.fail).map(Tier::as_str),
                    "title": r.title.get(Lang::En),
                })
            })
            .collect();
        Ok(pretty(
            json!({ "rules_version": spec().rules_version, "rules": rules }),
        ))
    }

    /// Crawler and AI-referral evidence from the ingested access logs (`aieo logs ingest`):
    /// per-crawler requests and verification, coverage of indexable pages, AI referrals, L findings.
    #[tool(annotations(title = "Access-log report", read_only_hint = true))]
    async fn logs_report(&self, Parameters(p): Parameters<LogsParams>) -> Result<String, String> {
        let window = crate::logs::Window {
            target: self.target(None, None),
            since: format!("{}d", p.since_days.unwrap_or(30)),
        };
        blocking(move || {
            let built = crate::logs::build_report(&window, "logs", Vec::new())?;
            let logs = &built.report.logs;
            let referrals: std::collections::BTreeMap<String, u64> =
                logs.referrals.iter().fold(Default::default(), |mut m, r| {
                    *m.entry(r.platform.clone()).or_default() += r.visits;
                    m
                });
            Ok(pretty(json!({
                "window": logs.window,
                "bots": logs.bots,
                "coverage": logs.coverage,
                "ai_referrals_by_platform": referrals,
                "anomalies": logs.anomalies,
                "notes": built.notes,
                "findings": built.findings.iter().map(|f| finding_json(f, "")).collect::<Vec<_>>(),
            })))
        })
        .await
    }

    /// Indexable pages that a class of crawler never fetched in the log window.
    #[tool(annotations(title = "Crawl gaps", read_only_hint = true))]
    async fn logs_gaps(&self, Parameters(p): Parameters<LogsParams>) -> Result<String, String> {
        let window = crate::logs::Window {
            target: self.target(None, None),
            since: format!("{}d", p.since_days.unwrap_or(30)),
        };
        let actor = p.actor.unwrap_or_else(|| "search_index".into());
        blocking(move || {
            let built = crate::logs::build_report(&window, "logs", Vec::new())?;
            let paths = built.report.never_crawled.get(&actor).ok_or_else(|| {
                anyhow::anyhow!("no coverage for {actor} (needs a build next to the logs)")
            })?;
            Ok(pretty(
                json!({ "actor": actor, "days": built.report.days, "never_crawled": paths }),
            ))
        })
        .await
    }

    /// Search Console / Bing evidence pulled with `aieo search …`, read against the build:
    /// totals, top pages, URL Inspection results, S findings.
    #[tool(annotations(title = "Search report", read_only_hint = true))]
    async fn search_report(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<String, String> {
        let target = self.target(None, None);
        let days = p.days.unwrap_or(28);
        blocking(move || {
            let Some((search, findings, _)) = crate::search::build(&target, days)? else {
                anyhow::bail!("no search data: run `aieo search google pull` / `inspect` or `aieo search bing pull` first");
            };
            Ok(pretty(json!({
                "search": search,
                "findings": findings.iter().map(|f| finding_json(f, "")).collect::<Vec<_>>(),
            })))
        })
        .await
    }

    /// The URLs an IndexNow submission would send (new, changed and removed pages since the
    /// baseline). Sends nothing: a person runs the returned command.
    #[tool(annotations(title = "IndexNow plan", read_only_hint = true))]
    async fn indexnow_plan(&self) -> Result<String, String> {
        let target = self.target(None, None);
        let root = self.root.clone();
        blocking(move || {
            let (config, _) = Config::load_or_default(&root)?;
            let baseline = load_baseline(None, config.check.baseline.as_deref(), &root, false)?
                .or_else(|| Bundle::read(&root.join(".aieo/baseline.json")).ok())
                .ok_or_else(|| anyhow::anyhow!("no baseline to compare with: `aieo snapshot` after the last submission"))?;
            let run = run_check(&target, None, Vec::new(), "check")?;
            let (added, changed, removed) = crate::cloud::changed_urls(&run.bundle, &baseline);
            Ok(pretty(json!({
                "new": added,
                "changed": changed,
                "removed": removed,
                "key_configured": config.indexnow.key.is_some(),
                "to_apply": "aieo indexnow submit --yes   (run by a person; it is recorded in .aieo/audit.jsonl)",
            })))
        })
        .await
    }

    /// What `aieo push` would upload to aieo.ee (evidence kinds, findings, project). Uploads nothing.
    #[tool(annotations(title = "Upload plan", read_only_hint = true))]
    async fn push_plan(&self) -> Result<String, String> {
        let target = self.target(None, None);
        let root = self.root.clone();
        blocking(move || {
            let (config, _) = Config::load_or_default(&root)?;
            let run = run_check(&target, None, Vec::new(), "check")?;
            let logs = root.join(".aieo/logs.db").is_file();
            let search = root.join(".aieo/search.db").is_file();
            let kinds: Vec<&str> = ["check"]
                .into_iter()
                .chain(logs.then_some("logs"))
                .chain(search.then_some("search"))
                .collect();
            Ok(pretty(json!({
                "project": config.aieo.project,
                "site": run.bundle.site.origin,
                "kinds": kinds,
                "findings_by_tier": run.bundle.summary.findings,
                "contains": "findings, page inventory, scores and aggregates — no page source, raw logs or IP addresses",
                "to_apply": "aieo push   (run by a person, logged in with `aieo login`)",
            })))
        })
        .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Aieo {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("aieo", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "aieo checks a website's build output for SEO / GEO defects and reads crawler logs and \
                 search-engine data against it. Build the site, call `check` (or `diff` against the \
                 baseline), fix findings in the reported source_file, rebuild and check again. \
                 `explain_rule` says why a rule matters. Nothing here writes outside the project: \
                 IndexNow and aieo.ee uploads are returned as plans for a person to run.",
            )
    }
}

pub fn serve(root: PathBuf) -> Result<u8> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let service = Aieo::new(root).serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        anyhow::Ok(())
    })?;
    Ok(0)
}
