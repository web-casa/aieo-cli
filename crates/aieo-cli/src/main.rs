//! `aieo` — SEO/GEO checks for a site's build output, before it ships.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use url::Url;

use aieo_core::bundle::Bundle;
use aieo_core::config::{parse_fail_on, resolve_settings, Config, GateCondition, CONFIG_FILE};
use aieo_core::spec::{spec, RuleClass};
use aieo_core::{Lang, Tier};
use aieo_report::{build_bundle, github, markdown, pretty, sarif, Paths, RunMeta};
use aieo_rules::{check, Options};
use aieo_site::preset::{detect, infer_origin, PresetKind};
use aieo_site::{Site, SiteOptions};

/// Exit codes: 0 gate passed · 1 gate failed · 2 incomplete evidence or usage error.
const EXIT_GATE: u8 = 1;
const EXIT_INCOMPLETE: u8 = 2;

#[derive(Parser)]
#[command(
    name = "aieo",
    version,
    about = "SEO/GEO checks for a site's build output — the inside view for aieo.ee"
)]
struct Cli {
    /// Output language for terminal and Markdown reports
    #[arg(long, global = true, value_enum)]
    lang: Option<LangArg>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum LangArg {
    En,
    Zh,
}

#[derive(Subcommand)]
enum Command {
    /// Write aieo.toml for this project (framework, build directory, site origin)
    Init {
        #[command(flatten)]
        target: Target,
        /// Overwrite an existing aieo.toml
        #[arg(long)]
        force: bool,
    },
    /// Check a build directory; exit 1 when the gate fails
    Check(CheckArgs),
    /// Save this build's results as the baseline later runs compare with
    Snapshot {
        #[command(flatten)]
        target: Target,
        /// Where to write the baseline
        #[arg(short, long, default_value = ".aieo/baseline.json")]
        output: PathBuf,
    },
    /// Check against a baseline and fail only on what is new
    Diff(CheckArgs),
    /// Write the evidence bundle (aieo-bundle/v1, gzip) for aieo.ee
    Export {
        #[command(flatten)]
        target: Target,
        #[arg(short, long, default_value = ".aieo/bundle.json.gz")]
        output: PathBuf,
        /// Compare with a baseline so the bundle carries new/fixed findings
        #[arg(long)]
        baseline: Option<PathBuf>,
    },
    /// List the rules
    Rules {
        /// Only this family (F, T, C, D, G, H)
        #[arg(long)]
        family: Option<String>,
        /// Include rules this CLI does not run (aieo.ee only)
        #[arg(long)]
        all: bool,
    },
    /// Explain one rule
    Explain { rule: String },
}

#[derive(Args, Clone)]
struct Target {
    /// Build directory (default: aieo.toml `dist`, else the preset's)
    dist: Option<PathBuf>,
    /// Project root holding aieo.toml and the sources
    #[arg(short = 'C', long, default_value = ".")]
    project: PathBuf,
    /// Production origin, e.g. https://example.com
    #[arg(long)]
    site: Option<String>,
    /// Framework preset
    #[arg(long, value_parser = ["auto", "static", "astro", "starlight"])]
    preset: Option<String>,
    /// Configuration file (default: <project>/aieo.toml)
    #[arg(long)]
    config: Option<PathBuf>,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum Format {
    Pretty,
    Json,
    Sarif,
    Github,
    Markdown,
}

#[derive(Args, Clone)]
struct CheckArgs {
    #[command(flatten)]
    target: Target,
    /// Baseline bundle to compare with (enables new/fixed and D rules)
    #[arg(long)]
    baseline: Option<PathBuf>,
    /// Gate conditions: error, warning, notice, new:error, … or never
    #[arg(long, value_delimiter = ',')]
    fail_on: Option<Vec<String>>,
    /// Format printed to stdout
    #[arg(long, value_enum, default_value = "pretty")]
    format: Format,
    /// Write the stdout format to a file instead
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Extra reports: FORMAT=PATH (json, sarif, github, markdown, bundle); repeatable
    #[arg(long = "report", value_name = "FORMAT=PATH")]
    reports: Vec<String>,
    /// Findings listed per rule in terminal / Markdown output
    #[arg(long, default_value_t = 10)]
    max_per_rule: usize,
    /// List every notice
    #[arg(short, long)]
    verbose: bool,
    /// Exit by the gate even when evidence is incomplete
    #[arg(long)]
    allow_incomplete: bool,
}

struct Prepared {
    project: aieo_site::preset::Project,
    config: Config,
    dist_rel: String,
    site: Site,
    notes: Vec<String>,
}

fn lang_of(arg: Option<LangArg>) -> Lang {
    match arg {
        Some(LangArg::Zh) => Lang::Zh,
        Some(LangArg::En) => Lang::En,
        None => {
            let env = std::env::var("AIEO_LANG")
                .or_else(|_| std::env::var("LANG"))
                .unwrap_or_default();
            if env.to_lowercase().starts_with("zh") {
                Lang::Zh
            } else {
                Lang::En
            }
        }
    }
}

fn origin_of(s: &str) -> Result<Url> {
    let u = Url::parse(s.trim()).with_context(|| format!("--site {s:?} is not a URL"))?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        bail!("--site must be an http(s) origin, got {s:?}");
    }
    Ok(Url::parse(&u.origin().ascii_serialization()).expect("origin parses"))
}

fn prepare(t: &Target) -> Result<Prepared> {
    let root = t.project.clone();
    let (config, _path) = match &t.config {
        Some(p) => (Config::load(p)?, Some(p.clone())),
        None => Config::load_or_default(&root)?,
    };
    let preset_arg = t
        .preset
        .as_deref()
        .or(config.preset.as_deref())
        .filter(|p| *p != "auto");
    let forced = match preset_arg {
        Some(p) => Some(PresetKind::parse(p).with_context(|| format!("unknown preset {p:?}"))?),
        None => None,
    };
    let project = detect(&root, forced);
    let dist = match (&t.dist, &config.dist) {
        (Some(d), _) => d.clone(),
        (None, Some(d)) => root.join(d),
        (None, None) => root.join(&project.default_dist),
    };
    if !dist.is_dir() {
        bail!(
            "build directory {} not found — build the site first or pass the directory",
            dist.display()
        );
    }
    let mut notes = Vec::new();
    let origin = if let Some(s) = &t.site {
        origin_of(s)?
    } else if let Some(s) = &config.site {
        origin_of(s)?
    } else if let Some(s) = &project.site {
        origin_of(s)?
    } else if let Some((u, how)) = infer_origin(&dist) {
        let o = origin_of(u.as_str())?;
        notes.push(format!(
            "site origin {} inferred from the {how}; set `site` in {CONFIG_FILE} or pass --site",
            o.origin().ascii_serialization()
        ));
        o
    } else {
        bail!("cannot tell the site's production origin: pass --site https://… or set `site` in {CONFIG_FILE}");
    };
    let site = Site::load(
        &dist,
        origin,
        SiteOptions {
            base_path: project.base.clone().unwrap_or_default(),
            ..Default::default()
        },
    )?;
    let dist_rel = pathdiff(&dist, &root);
    Ok(Prepared {
        project,
        config,
        dist_rel,
        site,
        notes,
    })
}

/// `dist` relative to `root` when it lies inside it.
fn pathdiff(path: &Path, root: &Path) -> String {
    let abs = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    match abs(path).strip_prefix(abs(root)) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_string_lossy().to_string(),
    }
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn ci_name() -> Option<String> {
    if env("GITHUB_ACTIONS").is_some() {
        Some("github-actions".into())
    } else if env("GITLAB_CI").is_some() {
        Some("gitlab-ci".into())
    } else if env("CI").is_some() {
        Some("ci".into())
    } else {
        None
    }
}

struct Run {
    bundle: Bundle,
    outcome: aieo_rules::Outcome,
    prepared: Prepared,
}

fn run_check(
    target: &Target,
    baseline: Option<&Bundle>,
    fail_on: Vec<GateCondition>,
    kind: &str,
) -> Result<Run> {
    let started = Instant::now();
    let prepared = prepare(target)?;
    let settings = resolve_settings(&prepared.config, spec(), OffsetDateTime::now_utc().date())?;
    for s in &settings.expired {
        eprintln!(
            "note: suppression of {} ({}) expired on {} and no longer applies",
            s.rule,
            s.url.as_deref().unwrap_or("*"),
            s.expires
        );
    }
    let outcome = check(
        &prepared.site,
        &Options {
            settings: &settings,
            project: Some(&prepared.project),
            baseline,
        },
    );
    let root = &prepared.project.root;
    let meta = RunMeta {
        origin: prepared.site.origin_str(),
        preset: prepared.project.kind.as_str().into(),
        project_id: None,
        kinds: vec![kind.into()],
        commit: env("GITHUB_SHA")
            .or_else(|| env("CI_COMMIT_SHA"))
            .or_else(|| git(root, &["rev-parse", "HEAD"])),
        branch: env("GITHUB_HEAD_REF")
            .or_else(|| env("GITHUB_REF_NAME"))
            .or_else(|| env("CI_COMMIT_REF_NAME"))
            .or_else(|| git(root, &["rev-parse", "--abbrev-ref", "HEAD"])),
        ci: ci_name(),
        duration_ms: started.elapsed().as_millis() as u64,
        generated_at: OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .expect("RFC 3339"),
        fail_on,
        baseline: baseline.map(|b| (b.generated_at.clone(), b.run.commit.clone())),
    };
    let bundle = build_bundle(&outcome, &meta);
    Ok(Run {
        bundle,
        outcome,
        prepared,
    })
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

fn render(run: &Run, format: &str, lang: Lang, args: &CheckArgs, color: bool) -> Result<Vec<u8>> {
    let paths = Paths {
        dist: run.prepared.dist_rel.clone(),
    };
    Ok(match format {
        "pretty" => {
            let style = pretty::Style {
                color,
                lang,
                per_rule: args.max_per_rule,
                verbose: args.verbose,
            };
            pretty::render(
                &run.bundle,
                &run.outcome.readiness.modules,
                &run.outcome.readiness.notes,
                &paths,
                &style,
            )
            .into_bytes()
        }
        "json" => run.bundle.to_json().into_bytes(),
        "sarif" => sarif::render(&run.bundle, &paths).into_bytes(),
        "github" => github::render(&run.bundle, &paths).into_bytes(),
        "markdown" => markdown::render(&run.bundle, lang, args.max_per_rule).into_bytes(),
        "bundle" => run.bundle.to_gzip(),
        other => bail!("unknown report format {other:?} (json, sarif, github, markdown, bundle)"),
    })
}

fn format_name(f: Format) -> &'static str {
    match f {
        Format::Pretty => "pretty",
        Format::Json => "json",
        Format::Sarif => "sarif",
        Format::Github => "github",
        Format::Markdown => "markdown",
    }
}

fn load_baseline(
    path: Option<&PathBuf>,
    config: Option<&str>,
    project: &Path,
    required: bool,
) -> Result<Option<Bundle>> {
    let candidate = path.cloned().or_else(|| config.map(|c| project.join(c)));
    match candidate {
        Some(p) if p.is_file() => Ok(Some(Bundle::read(&p)?)),
        Some(p) if path.is_some() || required => bail!(
            "baseline {} not found — create it with `aieo snapshot`",
            p.display()
        ),
        _ if required => {
            bail!("no baseline: pass --baseline or set [check] baseline in {CONFIG_FILE}")
        }
        _ => Ok(None),
    }
}

fn cmd_check(args: CheckArgs, lang: Lang, diff: bool) -> Result<u8> {
    let (config, _) = match &args.target.config {
        Some(p) => (Config::load(p)?, None),
        None => Config::load_or_default(&args.target.project)?,
    };
    let baseline = load_baseline(
        args.baseline.as_ref(),
        config.check.baseline.as_deref(),
        &args.target.project,
        diff,
    )?;
    let fail_on = match (&args.fail_on, &config.check.fail_on) {
        (Some(v), _) | (None, Some(v)) => parse_fail_on(v)?,
        (None, None) if baseline.is_some() => {
            parse_fail_on(&["new:error".into(), "new:warning".into()])?
        }
        (None, None) => parse_fail_on(&["error".into()])?,
    };
    let run = run_check(
        &args.target,
        baseline.as_ref(),
        fail_on,
        if baseline.is_some() { "diff" } else { "check" },
    )?;
    for note in &run.prepared.notes {
        eprintln!("note: {note}");
    }

    let color =
        args.output.is_none() && std::io::stdout().is_terminal() && env("NO_COLOR").is_none();
    let main = render(&run, format_name(args.format), lang, &args, color)?;
    match &args.output {
        Some(p) => write_file(p, &main)?,
        None => {
            use std::io::Write;
            std::io::stdout().write_all(&main)?;
        }
    }
    for spec in &args.reports {
        let Some((format, path)) = spec.split_once('=') else {
            bail!("--report expects FORMAT=PATH, got {spec:?}")
        };
        write_file(Path::new(path), &render(&run, format, lang, &args, false)?)?;
    }
    if let Some(summary) = env("GITHUB_STEP_SUMMARY") {
        let md = markdown::render(&run.bundle, lang, args.max_per_rule);
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(summary)
        {
            let _ = writeln!(f, "{md}");
        }
    }

    let sum = &run.bundle.summary;
    if sum.incomplete && !args.allow_incomplete {
        eprintln!(
            "aieo: incomplete evidence ({}); exiting 2",
            sum.incomplete_reasons.join("; ")
        );
        return Ok(EXIT_INCOMPLETE);
    }
    Ok(if sum.gate.passed { 0 } else { EXIT_GATE })
}

fn cmd_snapshot(target: Target, output: PathBuf) -> Result<u8> {
    let run = run_check(&target, None, Vec::new(), "check")?;
    for note in &run.prepared.notes {
        eprintln!("note: {note}");
    }
    let path = if output.is_relative() {
        target.project.join(&output)
    } else {
        output
    };
    write_file(&path, run.bundle.to_json().as_bytes())?;
    let s = &run.bundle.summary;
    eprintln!(
        "baseline written to {} ({} pages, {} errors, {} warnings, {} notices)",
        path.display(),
        s.pages,
        s.findings.error,
        s.findings.warning,
        s.findings.notice
    );
    Ok(if s.incomplete { EXIT_INCOMPLETE } else { 0 })
}

fn cmd_export(target: Target, output: PathBuf, baseline: Option<PathBuf>) -> Result<u8> {
    let (config, _) = Config::load_or_default(&target.project)?;
    let base = load_baseline(
        baseline.as_ref(),
        config.check.baseline.as_deref(),
        &target.project,
        false,
    )?;
    let run = run_check(
        &target,
        base.as_ref(),
        Vec::new(),
        if base.is_some() { "diff" } else { "check" },
    )?;
    let path = if output.is_relative() {
        target.project.join(&output)
    } else {
        output
    };
    write_file(&path, &run.bundle.to_gzip())?;
    eprintln!(
        "bundle written to {} — upload it at aieo.ee or with `aieo push` (0.4)",
        path.display()
    );
    Ok(if run.bundle.summary.incomplete {
        EXIT_INCOMPLETE
    } else {
        0
    })
}

fn cmd_init(target: Target, force: bool) -> Result<u8> {
    let root = &target.project;
    let path = root.join(CONFIG_FILE);
    if path.exists() && !force {
        bail!(
            "{} already exists (use --force to overwrite)",
            path.display()
        );
    }
    let forced = target
        .preset
        .as_deref()
        .filter(|p| *p != "auto")
        .and_then(PresetKind::parse);
    let project = detect(root, forced);
    let dist = target
        .dist
        .as_ref()
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_else(|| project.default_dist.clone());
    let site = target
        .site
        .clone()
        .or_else(|| project.site.clone())
        .or_else(|| infer_origin(&root.join(&dist)).map(|(u, _)| u.origin().ascii_serialization()));
    let site_line = match &site {
        Some(s) => format!("site = \"{}\"", origin_of(s)?.origin().ascii_serialization()),
        None => "# site = \"https://example.com\"   # production origin (required when the build does not declare it)".into(),
    };
    let text = format!(
        r#"# aieo-cli configuration — https://aieo.ee
{site_line}
dist = "{dist}"
preset = "{preset}"

[check]
# gate: by default ["error"], and ["new:error", "new:warning"] once a baseline exists
# fail_on = ["error"]
baseline = ".aieo/baseline.json"
# URL paths left out of page rules
ignore = []

[rules]
# "C-ORPHAN-PAGE" = "notice"   # error | warning | notice | off | on

# [[suppress]]
# rule = "C-LINK-BROKEN"
# url = "/legacy/**"
# reason = "why this is acceptable"
# owner = "who decided"
# expires = "2026-12-31"
"#,
        preset = project.kind.as_str()
    );
    write_file(&path, text.as_bytes())?;
    eprintln!(
        "wrote {} (preset {}, build directory {dist})",
        path.display(),
        project.kind.as_str()
    );
    if site.is_none() {
        eprintln!("note: set `site` — the build does not declare its production origin");
    }
    eprintln!("next: build the site, run `aieo check`, then `aieo snapshot` to start a baseline");
    Ok(0)
}

fn tier_label(r: &aieo_core::RuleSpec) -> String {
    let t = |t: Option<Tier>| t.map(|t| t.as_str()).unwrap_or("-");
    if r.class == RuleClass::Finding {
        t(r.tier.default).to_string()
    } else {
        format!("{}/{}", t(r.tier.fail), t(r.tier.partial))
    }
}

fn cmd_rules(family: Option<String>, all: bool, lang: Lang) -> Result<u8> {
    println!("@aieo/rules {}", spec().rules_version);
    for r in spec().rules.iter().filter(|r| all || r.implemented_here()) {
        if family
            .as_deref()
            .is_some_and(|f| !r.family.eq_ignore_ascii_case(f))
        {
            continue;
        }
        let flags = format!(
            "{}{}",
            if r.default_enabled { "" } else { " [off]" },
            if r.evidence_level == aieo_core::spec::EvidenceLevel::Heuristic {
                " [heuristic]"
            } else {
                ""
            }
        );
        println!(
            "{:<28} {:<15} {}{}",
            r.id,
            tier_label(r),
            r.title.get(lang),
            flags
        );
    }
    Ok(0)
}

fn cmd_explain(id: &str, lang: Lang) -> Result<u8> {
    let Some(r) = spec().rules.iter().find(|r| r.id.eq_ignore_ascii_case(id)) else {
        bail!("unknown rule {id:?} — see `aieo rules`");
    };
    println!("{} — {}", r.id, r.title.get(lang));
    println!();
    println!("{}", r.description.get(lang));
    println!();
    println!("tier:      {}", tier_label(r));
    println!(
        "scope:     {}",
        if r.scope == aieo_core::Scope::Page {
            "page"
        } else {
            "site"
        }
    );
    println!(
        "evidence:  {}",
        if r.evidence_level == aieo_core::spec::EvidenceLevel::Official {
            "official"
        } else {
            "heuristic"
        }
    );
    println!(
        "runs in:   {}",
        r.implementations
            .join(", ")
            .replace("ts", "aieo.ee")
            .replace("rust", "aieo-cli")
    );
    if !r.default_enabled {
        println!("default:   off (enable with [rules] \"{}\" = \"on\")", r.id);
    }
    if let Some(src) = &r.source_url {
        println!("source:    {src}");
    }
    Ok(0)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let lang = lang_of(cli.lang);
    let result = match cli.command {
        Command::Init { target, force } => cmd_init(target, force),
        Command::Check(args) => cmd_check(args, lang, false),
        Command::Diff(args) => cmd_check(args, lang, true),
        Command::Snapshot { target, output } => cmd_snapshot(target, output),
        Command::Export {
            target,
            output,
            baseline,
        } => cmd_export(target, output, baseline),
        Command::Rules { family, all } => cmd_rules(family, all, lang),
        Command::Explain { rule } => cmd_explain(&rule, lang),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("aieo: {e:#}");
            ExitCode::from(EXIT_INCOMPLETE)
        }
    }
}
