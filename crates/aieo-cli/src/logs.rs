//! `aieo logs …`: ingest access logs into `.aieo/logs.db` and report what
//! crawlers and AI platforms did on the site.

use std::fmt::Write as _;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use aieo_core::bundle::{Bundle, Logs};
use aieo_core::config::{parse_fail_on, resolve_settings, Config, GateCondition};
use aieo_core::spec::spec;
use aieo_core::{Finding, Lang};
use aieo_logs::ingest::{ingest_file, keep_out_of_git, load_or_create_salt, Options};
use aieo_logs::parse::Format as LogFormat;
use aieo_logs::report::{self, bot_name, Inventory, Params, Report};
use aieo_logs::store::Store;
use aieo_logs::verify::{http_fetch, SystemResolver, Verifier};
use aieo_report::{attach_logs, empty_bundle, markdown, pretty, Paths, RunMeta};
use aieo_rules::emit::Emitter;
use aieo_site::robots::parse_robots;

use crate::{env, git, run_check, write_file, Target, EXIT_GATE};

#[derive(Subcommand)]
pub enum LogsCommand {
    /// Read access logs (Nginx, Apache, Caddy, Cloudflare, Vercel, CloudFront; .gz too) into .aieo/logs.db
    Ingest(IngestArgs),
    /// Crawler coverage, verification, errors and AI referrals from the ingested logs
    Report(ReportArgs),
    /// Indexable pages a class of crawler never fetched
    Gaps {
        #[command(flatten)]
        window: Window,
        /// search_index, classic_search, user_fetch or training
        #[arg(long, default_value = "search_index")]
        actor: String,
    },
    /// Landing pages visitors reached from AI answer platforms
    Referrals {
        #[command(flatten)]
        window: Window,
    },
    /// Pull crawler traffic from a provider's analytics API
    #[command(subcommand)]
    Pull(PullSource),
}

#[derive(Subcommand)]
pub enum PullSource {
    /// Cloudflare GraphQL Analytics (sampled hourly counts of catalog crawlers; no IPs)
    Cloudflare {
        /// Zone ID (dashboard → Overview → API)
        #[arg(long)]
        zone: String,
        /// Environment variable holding an API token with Analytics:Read on the zone
        #[arg(long, default_value = "CLOUDFLARE_API_TOKEN")]
        token_env: String,
        /// Days back from now (limited by what the plan keeps)
        #[arg(long, default_value = "7d")]
        since: String,
        /// Project root (where .aieo/ lives)
        #[arg(short = 'C', long, default_value = ".")]
        project: PathBuf,
        #[arg(long, hide = true, default_value = aieo_logs::cloudflare::ENDPOINT)]
        endpoint: String,
    },
}

#[derive(Args)]
pub struct IngestArgs {
    /// Log files or directories (their files, not recursive)
    #[arg(required = true)]
    files: Vec<PathBuf>,
    /// Project root (where .aieo/ lives)
    #[arg(short = 'C', long, default_value = ".")]
    project: PathBuf,
    /// auto, combined, caddy, cloudflare, vercel, cloudfront, cloudfront-json,
    /// or a custom format: nginx:'<log_format>' / apache:'<LogFormat>'
    #[arg(long, default_value = "auto")]
    format: String,
    /// Keep only requests for this host (default: the site's host); `*` keeps all
    #[arg(long)]
    host: Option<String>,
    /// Do not download IP ranges or query DNS (crawler claims stay unchecked)
    #[arg(long)]
    offline: bool,
}

#[derive(Args, Clone)]
pub struct Window {
    #[command(flatten)]
    target: Target,
    /// Days back from the newest request: 30 or 30d
    #[arg(long, default_value = "30d")]
    since: String,
}

#[derive(Args)]
pub struct ReportArgs {
    #[command(flatten)]
    window: Window,
    #[arg(long, value_enum, default_value = "pretty")]
    format: ReportFormat,
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Gate on L findings: warning, notice, … (default: never — logs describe the past)
    #[arg(long, value_delimiter = ',')]
    fail_on: Option<Vec<String>>,
}

#[derive(Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
pub enum ReportFormat {
    Pretty,
    Json,
    Markdown,
}

fn aieo_dir(project: &Path) -> PathBuf {
    project.join(".aieo")
}

pub fn db_path(project: &Path) -> PathBuf {
    aieo_dir(project).join("logs.db")
}

fn since_days(s: &str) -> Result<u64> {
    let n = s.trim().trim_end_matches('d');
    n.parse::<u64>()
        .ok()
        .filter(|d| *d > 0)
        .with_context(|| format!("--since {s:?}: use a number of days, e.g. 30d"))
}

fn origin_host(origin: &str) -> Option<String> {
    url::Url::parse(origin)
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}

fn config_of(target: &Target) -> Result<Config> {
    Ok(match &target.config {
        Some(p) => Config::load(p)?,
        None => Config::load_or_default(&target.project)?.0,
    })
}

pub fn cmd_ingest(args: IngestArgs) -> Result<u8> {
    let dir = aieo_dir(&args.project);
    std::fs::create_dir_all(&dir)?;
    keep_out_of_git(&dir)?;
    let salt = load_or_create_salt(&dir)?;
    let mut store = Store::open(&db_path(&args.project))?;
    let verifier = Verifier::new(
        Some(dir.join("cache/ip-ranges")),
        (!args.offline).then(http_fetch),
        Box::new(SystemResolver),
    );
    let host = match args.host.as_deref() {
        Some("*") => None,
        Some(h) => Some(h.to_ascii_lowercase()),
        None => Config::load_or_default(&args.project)?
            .0
            .site
            .as_deref()
            .and_then(origin_host),
    };
    let format = LogFormat::from_arg(&args.format).map_err(anyhow::Error::msg)?;
    let opts = Options {
        format,
        host: host.clone(),
        verifier: &verifier,
        salt,
    };
    let mut files = Vec::new();
    for f in &args.files {
        if f.is_dir() {
            let mut inside: Vec<PathBuf> = std::fs::read_dir(f)?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect();
            inside.sort();
            files.extend(inside);
        } else {
            files.push(f.clone());
        }
    }
    if let Some(h) = &host {
        eprintln!("keeping requests for {h} (and www.{h}); --host '*' keeps every host");
    }
    let mut failed = 0;
    for path in files {
        match ingest_file(&mut store, &path, &opts) {
            Ok(r) if r.up_to_date && r.lines == 0 => {
                eprintln!("{}: already ingested", path.display())
            }
            Ok(r) => {
                let mut line = format!(
                    "{}: {} · {} lines{} · {} automated / AI-referred requests stored",
                    path.display(),
                    r.format,
                    r.lines,
                    if r.resumed_at > 0 { " (resumed)" } else { "" },
                    r.stored
                );
                if r.duplicates > 0 {
                    let _ = write!(line, " ({} already known)", r.duplicates);
                }
                let _ = write!(line, " · {} visits counted", r.humans);
                if r.other_hosts > 0 {
                    let _ = write!(line, " · {} for other hosts skipped", r.other_hosts);
                }
                if r.malformed > 0 {
                    let lines: Vec<String> =
                        r.malformed_examples.iter().map(u64::to_string).collect();
                    let _ = write!(
                        line,
                        " · {} unreadable (line {})",
                        r.malformed,
                        lines.join(", ")
                    );
                }
                eprintln!("{line}");
                if r.requests == 0 && r.lines > 0 {
                    eprintln!("  no request could be read — is the format right? (--format)");
                    failed += 1;
                }
            }
            Err(e) => {
                eprintln!("{}: {e:#}", path.display());
                failed += 1;
            }
        }
    }
    for note in verifier.notes.lock().expect("notes").iter() {
        eprintln!("note: IP ranges {note}");
    }
    Ok(if failed > 0 {
        crate::EXIT_INCOMPLETE
    } else {
        0
    })
}

pub fn cmd_pull(source: PullSource) -> Result<u8> {
    let PullSource::Cloudflare {
        zone,
        token_env,
        since,
        project,
        endpoint,
    } = source;
    let token = std::env::var(&token_env).map_err(|_| {
        anyhow::anyhow!(
            "set {token_env} to a Cloudflare API token with Analytics:Read on the zone (it is read from the environment, never from the command line)"
        )
    })?;
    let dir = aieo_dir(&project);
    std::fs::create_dir_all(&dir)?;
    keep_out_of_git(&dir)?;
    let mut store = Store::open(&db_path(&project))?;
    let post = aieo_logs::cloudflare::http_post(endpoint, token);
    let days = since_days(&since)? as i64;
    let r = aieo_logs::cloudflare::pull(&mut store, &post, &zone, days, OffsetDateTime::now_utc())?;
    eprintln!(
        "Cloudflare zone {zone}: {} rows, {} crawler requests ({} → {}); sampled counts without IPs, so crawler claims stay unchecked",
        r.rows,
        r.requests,
        r.from.map(|t| t.format(&Rfc3339).unwrap_or_default()).unwrap_or_default(),
        r.to.map(|t| t.format(&Rfc3339).unwrap_or_default()).unwrap_or_default(),
    );
    for n in r.notes {
        eprintln!("note: {n}");
    }
    Ok(0)
}

/// The report with the build's inventory when there is a build.
struct Built {
    report: Report,
    bundle: Bundle,
    findings: Vec<Finding>,
    fail_on: Vec<GateCondition>,
    notes: Vec<String>,
    store: Store,
}

fn build_report(window: &Window, kinds: &str, fail_on: Vec<GateCondition>) -> Result<Built> {
    let started = Instant::now();
    let target = &window.target;
    let path = db_path(&target.project);
    if !path.is_file() {
        bail!(
            "no logs yet: run `aieo logs ingest <files>` first ({} missing)",
            path.display()
        );
    }
    let store = Store::open(&path)?;
    let config = config_of(target)?;
    let mut notes = Vec::new();
    let (mut bundle, inventory) = match run_check(target, None, Vec::new(), kinds) {
        Ok(run) => {
            let site = &run.prepared.site;
            let robots = site
                .resolve_exact(&format!("{}/robots.txt", site.base_path))
                .and_then(|f| site.read_bytes(f))
                .map(|b| parse_robots(&String::from_utf8_lossy(&b)));
            let inventory = Inventory {
                indexable: run
                    .outcome
                    .pages
                    .iter()
                    .filter(|p| p.indexable)
                    .filter_map(|p| url::Url::parse(&p.url).ok().map(|u| u.path().to_string()))
                    .collect(),
                robots,
                base_path: site.base_path.clone(),
            };
            let mut bundle = run.bundle;
            bundle.readiness = None;
            bundle.findings.clear();
            bundle.diff = None;
            bundle.run.kinds = vec![kinds.into()];
            (bundle, Some(inventory))
        }
        Err(e) => {
            notes.push(format!(
                "no build to compare with ({e:#}); coverage and robots.txt checks are skipped"
            ));
            let origin = target
                .site
                .clone()
                .or_else(|| config.site.clone())
                .context("pass --site or set `site` in aieo.toml")?;
            let meta = RunMeta {
                origin: url::Url::parse(&origin)
                    .map(|u| u.origin().ascii_serialization())
                    .unwrap_or(origin),
                preset: "static".into(),
                project_id: None,
                kinds: vec![kinds.into()],
                commit: git(&target.project, &["rev-parse", "HEAD"]),
                branch: git(&target.project, &["rev-parse", "--abbrev-ref", "HEAD"]),
                ci: None,
                duration_ms: 0,
                generated_at: OffsetDateTime::now_utc()
                    .format(&Rfc3339)
                    .expect("RFC 3339"),
                fail_on: fail_on.clone(),
                baseline: None,
            };
            (empty_bundle(&meta), None)
        }
    };
    let origin = bundle.site.origin.clone();
    let host = origin_host(&origin);
    let params = Params {
        since_days: since_days(&window.since)?,
        host: host.as_deref(),
        origin: Some(&origin),
        inventory: inventory.as_ref(),
    };
    let Some(report) = report::build(&store, &params)? else {
        bail!("{} holds no requests yet", path.display());
    };
    let settings = resolve_settings(&config, spec(), OffsetDateTime::now_utc().date())?;
    let mut e = Emitter::new(&settings);
    report::findings(&report, &params, &store, &mut e)?;
    let findings: Vec<Finding> = e.out.into_iter().map(|p| p.finding).collect();
    attach_logs(&mut bundle, report.logs.clone(), findings.clone(), &fail_on);
    bundle.run.duration_ms = started.elapsed().as_millis() as u64;
    Ok(Built {
        report,
        bundle,
        findings,
        fail_on,
        notes,
        store,
    })
}

/// Logs for `aieo export`: the aggregates and L findings, when logs exist.
pub fn for_export(target: &Target) -> Result<Option<(Logs, Vec<Finding>)>> {
    if !db_path(&target.project).is_file() {
        return Ok(None);
    }
    let window = Window {
        target: target.clone(),
        since: "30d".into(),
    };
    let built = build_report(&window, "logs", Vec::new())?;
    let _ = &built.store;
    Ok(Some((built.report.logs, built.findings)))
}

/// Pad to a terminal width (CJK characters take two columns).
fn pad(s: &str, width: usize, right: bool) -> String {
    let w: usize = s
        .chars()
        .map(|c| if matches!(c as u32, 0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6) { 2 } else { 1 })
        .sum();
    let fill = " ".repeat(width.saturating_sub(w));
    if right {
        format!("{fill}{s}")
    } else {
        format!("{s}{fill}")
    }
}

fn pct(n: u64, of: u64) -> String {
    if of == 0 {
        "–".into()
    } else {
        format!("{:.0}%", n as f64 * 100.0 / of as f64)
    }
}

fn render_pretty(b: &Built, lang: Lang) -> String {
    let zh = lang == Lang::Zh;
    let r = &b.report;
    let w = &r.logs.window;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "{} {} → {} ({} {})",
        if zh {
            "aieo logs  时间窗"
        } else {
            "aieo logs  window"
        },
        &w.from[..10.min(w.from.len())],
        &w.to[..10.min(w.to.len())],
        r.days,
        if zh { "天" } else { "days" }
    );
    let crawler_hits: u64 = r
        .bots
        .iter()
        .filter(|x| x.bot.is_some())
        .map(|x| x.hits - x.verification.mismatch)
        .sum();
    let _ = writeln!(
        s,
        "{}: {} · {}: {} · {}: {}",
        if zh {
            "已知爬虫请求"
        } else {
            "known crawler requests"
        },
        crawler_hits,
        if zh { "访问" } else { "visits" },
        r.human_visits,
        if zh {
            "来自 AI 平台"
        } else {
            "from AI platforms"
        },
        r.ai_referrals
    );
    let _ = writeln!(
        s,
        "\n{} {} {} {} {:>7} {:>7} {:>7} {:>7}  {}",
        pad(if zh { "爬虫" } else { "crawler" }, 22, false),
        pad(if zh { "用途" } else { "purpose" }, 15, false),
        pad(if zh { "请求" } else { "hits" }, 7, true),
        pad(if zh { "已验证" } else { "verified" }, 9, true),
        "2xx",
        "3xx",
        "4xx",
        "5xx",
        if zh { "冒充" } else { "spoofed" }
    );
    for x in r.bots.iter().take(25) {
        let genuine = x.hits - x.verification.mismatch;
        let _ = writeln!(
            s,
            "{:<22} {:<15} {:>7} {:>9} {:>7} {:>7} {:>7} {:>7}  {}",
            bot_name(x),
            x.bot.map_or(x.actor.as_str(), |c| c.purpose.as_str()),
            genuine,
            if x.bot.is_some_and(
                |c| c.verify.ip_ranges_url.is_some() || !c.verify.rdns_suffixes.is_empty()
            ) {
                pct(x.verification.verified, genuine)
            } else {
                "n/a".into()
            },
            x.status.s2,
            x.status.s3,
            x.status.s4,
            x.status.s5,
            if x.verification.mismatch > 0 {
                x.verification.mismatch.to_string()
            } else {
                String::new()
            }
        );
    }
    let overall: Vec<_> = r.logs.coverage.iter().filter(|c| c.bot.is_none()).collect();
    if !overall.is_empty() {
        let _ = writeln!(
            s,
            "\n{}",
            if zh {
                "可索引页面覆盖率"
            } else {
                "Coverage of indexable pages"
            }
        );
        for c in overall {
            let _ = writeln!(
                s,
                "  {:<15} {}/{} ({})",
                c.actor,
                c.crawled,
                c.indexable,
                pct(c.crawled, c.indexable)
            );
        }
        let ai: Vec<String> = r
            .logs
            .coverage
            .iter()
            .filter(|c| c.bot.is_some() && c.actor == "search_index")
            .map(|c| format!("{} {}", c.bot.as_deref().unwrap_or_default(), c.crawled))
            .collect();
        if !ai.is_empty() {
            let _ = writeln!(
                s,
                "  {}: {}",
                if zh {
                    "AI 搜索爬虫"
                } else {
                    "AI search crawlers"
                },
                ai.join(" · ")
            );
        }
    }
    if r.ai_referrals > 0 {
        let mut by: std::collections::BTreeMap<(String, String), u64> = Default::default();
        for x in &r.logs.referrals {
            *by.entry((x.platform.clone(), x.landing.clone()))
                .or_default() += x.visits;
        }
        let mut rows: Vec<_> = by.into_iter().collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.1));
        let _ = writeln!(
            s,
            "\n{}",
            if zh {
                "AI 引荐落地页"
            } else {
                "AI referral landing pages"
            }
        );
        for ((platform, landing), n) in rows.into_iter().take(10) {
            let _ = writeln!(s, "  {n:>6}  {platform:<11} {landing}");
        }
    }
    s
}

pub fn cmd_report(args: ReportArgs, lang: Lang) -> Result<u8> {
    let fail_on = match &args.fail_on {
        Some(v) => parse_fail_on(v)?,
        None => Vec::new(),
    };
    let built = build_report(&args.window, "logs", fail_on)?;
    for n in &built.notes {
        eprintln!("note: {n}");
    }
    let out = match args.format {
        ReportFormat::Json => built.bundle.to_json(),
        ReportFormat::Markdown => markdown::render(&built.bundle, lang, 10),
        ReportFormat::Pretty => {
            let color = args.output.is_none()
                && std::io::stdout().is_terminal()
                && env("NO_COLOR").is_none();
            let mut s = render_pretty(&built, lang);
            if !built.findings.is_empty() {
                let style = pretty::Style {
                    color,
                    lang,
                    per_rule: 10,
                    verbose: true,
                };
                let paths = Paths {
                    dist: String::new(),
                };
                let findings = pretty::render(&built.bundle, &[], &[], &paths, &style);
                // the findings section only: the header describes a check run
                if let Some(i) = findings
                    .find("\nFindings")
                    .or_else(|| findings.find("\n发现"))
                {
                    s.push_str(&findings[i..]);
                } else {
                    s.push_str(&findings);
                }
            }
            s
        }
    };
    match &args.output {
        Some(p) => write_file(p, out.as_bytes())?,
        None => print!("{out}"),
    }
    Ok(
        if built.bundle.summary.gate.passed || built.fail_on.is_empty() {
            0
        } else {
            EXIT_GATE
        },
    )
}

pub fn cmd_gaps(window: Window, actor: String) -> Result<u8> {
    let built = build_report(&window, "logs", Vec::new())?;
    let Some(paths) = built.report.never_crawled.get(&actor) else {
        bail!(
            "no coverage for {actor:?}: it needs a build to compare with, and one of search_index, classic_search, user_fetch, training"
        );
    };
    for p in paths {
        println!("{}{p}", built.bundle.site.origin);
    }
    eprintln!(
        "{} indexable page(s) never fetched by a {actor} crawler in {} days",
        paths.len(),
        built.report.days
    );
    Ok(0)
}

pub fn cmd_referrals(window: Window) -> Result<u8> {
    let built = build_report(&window, "logs", Vec::new())?;
    let mut by: std::collections::BTreeMap<(String, String), u64> = Default::default();
    for x in &built.report.logs.referrals {
        *by.entry((x.landing.clone(), x.platform.clone()))
            .or_default() += x.visits;
    }
    let mut rows: Vec<_> = by.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for ((landing, platform), n) in &rows {
        println!(
            "{n:>7}  {platform:<11} {}{landing}",
            built.bundle.site.origin
        );
    }
    eprintln!(
        "{} visit(s) from AI platforms in {} days",
        built.report.ai_referrals, built.report.days
    );
    Ok(0)
}
