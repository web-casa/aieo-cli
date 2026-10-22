//! `aieo login`, `aieo push` and `aieo indexnow …`: the commands that talk
//! to services outside this machine. Writes are planned first and only
//! sent with `--yes`; each is recorded in `.aieo/audit.jsonl`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use url::Url;

use aieo_core::bundle::Bundle;
use aieo_core::config::Config;
use aieo_search::cloud;
use aieo_search::creds::{AieoKey, Credentials, DEFAULT_ENDPOINT};
use aieo_search::http::Ureq;
use aieo_search::indexnow::{self, KeyFile};
use aieo_search::store::{audit, SearchStore};

use crate::{load_baseline, run_check, Target};

fn aieo_dir(project: &Path) -> PathBuf {
    project.join(".aieo")
}

#[derive(Args)]
pub struct LoginArgs {
    /// aieo.ee base URL
    #[arg(long)]
    endpoint: Option<String>,
    /// Project root, to find the aieo.ee project matching its site
    #[arg(short = 'C', long, default_value = ".")]
    project: PathBuf,
}

pub fn cmd_login(args: LoginArgs) -> Result<u8> {
    let (config, _) = Config::load_or_default(&args.project)?;
    let endpoint = args
        .endpoint
        .or(config.aieo.endpoint.clone())
        .unwrap_or_else(|| DEFAULT_ENDPOINT.into());
    let api_key = match std::env::var("AIEO_API_KEY").ok().filter(|k| !k.is_empty()) {
        Some(k) => k,
        None => {
            eprintln!(
                "Create an API key with write access at {endpoint}/dashboard/settings/api-keys"
            );
            rpassword::prompt_password("API key: ")?.trim().to_string()
        }
    };
    if !api_key.starts_with("aieo_") {
        bail!("that does not look like an aieo.ee API key (they start with aieo_)");
    }
    let key = AieoKey { api_key, endpoint };
    let projects = cloud::projects(&Ureq::default(), &key)?;
    let mut creds = Credentials::load()?;
    creds.data.aieo = Some(key);
    creds.save()?;
    eprintln!(
        "logged in; the key is stored in {} (owner-only)",
        creds.path.display()
    );
    let host = config
        .site
        .as_deref()
        .and_then(|s| Url::parse(s).ok())
        .and_then(|u| u.host_str().map(String::from));
    match (host.as_deref(), &config.aieo.project) {
        (_, Some(p)) => eprintln!("aieo.toml pushes to project {p}"),
        (Some(h), None) => match cloud::project_for_host(&projects, h) {
            Some(p) => eprintln!(
                "project {:?} matches {h}; add to aieo.toml:\n\n[aieo]\nproject = \"{}\"\n",
                p.name, p.id
            ),
            None => eprintln!("no project for {h} in this organisation yet; create one on aieo.ee"),
        },
        _ => {
            for p in &projects {
                eprintln!("  {}  {}  ({})", p.id, p.domain, p.name);
            }
        }
    }
    Ok(0)
}

#[derive(Args)]
pub struct PushArgs {
    #[command(flatten)]
    target: Target,
    /// Upload this bundle instead of exporting a fresh one
    #[arg(long)]
    bundle: Option<PathBuf>,
    /// aieo.ee project id (default: aieo.toml [aieo] project)
    #[arg(long = "to")]
    to: Option<String>,
    /// Upload even when the bundle's site is not the project's domain
    #[arg(long)]
    allow_origin_mismatch: bool,
}

pub fn cmd_push(args: PushArgs) -> Result<u8> {
    let root = args.target.project.clone();
    let (config, _) = Config::load_or_default(&root)?;
    let creds = Credentials::load()?;
    let mut key = creds
        .aieo()
        .context("not logged in: run `aieo login` (or set AIEO_API_KEY)")?;
    if let Some(e) = &config.aieo.endpoint {
        if std::env::var("AIEO_ENDPOINT").is_err() {
            key.endpoint = e.clone();
        }
    }
    let project = args
        .to
        .clone()
        .or(config.aieo.project.clone())
        .context("which aieo.ee project? set [aieo] project in aieo.toml or pass --to <id> (`aieo login` lists them)")?;
    let gzip = match &args.bundle {
        Some(p) => std::fs::read(p).with_context(|| format!("reading {}", p.display()))?,
        None => {
            crate::cmd_export(
                args.target.clone(),
                PathBuf::from(".aieo/bundle.json.gz"),
                None,
            )?;
            std::fs::read(root.join(".aieo/bundle.json.gz"))?
        }
    };
    let pushed = cloud::push(
        &Ureq::default(),
        &key,
        &project,
        gzip.clone(),
        args.allow_origin_mismatch,
    )?;
    audit(
        &aieo_dir(&root),
        "aieo.push",
        &format!("{}/projects/{project}", key.endpoint),
        gzip.len() as u64,
        Some(if pushed.duplicate { 200 } else { 201 }),
        &pushed.run_id,
    )?;
    if pushed.duplicate {
        eprintln!(
            "already uploaded (run {}) — same commit and kinds",
            pushed.run_id
        );
    } else {
        eprintln!("uploaded: run {}", pushed.run_id);
    }
    if let Some(v) = pushed.inside_view {
        eprintln!("see it at {v}");
    }
    Ok(0)
}

#[derive(Subcommand)]
pub enum IndexNowCommand {
    /// Make a key and, with --write, its key file in the framework's static files
    Keygen {
        #[arg(short = 'C', long, default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        write: bool,
    },
    /// Check the key file in the build and on the live site
    Verify(IndexNowArgs),
    /// Submit URLs that changed since the baseline (a plan unless --yes)
    Submit {
        #[command(flatten)]
        args: IndexNowArgs,
        /// Baseline to compare with (default: aieo.toml [check] baseline, .aieo/baseline.json)
        #[arg(long)]
        baseline: Option<PathBuf>,
        /// Submit every indexable page instead of the changes
        #[arg(long)]
        all: bool,
        /// Submit these URLs (one per line) instead
        #[arg(long)]
        urls: Option<PathBuf>,
        /// Actually send (otherwise only the plan is printed)
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Args, Clone)]
pub struct IndexNowArgs {
    #[command(flatten)]
    target: Target,
    /// IndexNow key (default: aieo.toml [indexnow] key)
    #[arg(long)]
    key: Option<String>,
    #[arg(long)]
    endpoint: Option<String>,
}

fn key_of(args: &IndexNowArgs, config: &Config) -> Result<String> {
    args.key.clone().or(config.indexnow.key.clone()).context(
        "no IndexNow key: run `aieo indexnow keygen --write` and set [indexnow] key in aieo.toml",
    )
}

pub fn cmd_indexnow(cmd: IndexNowCommand) -> Result<u8> {
    match cmd {
        IndexNowCommand::Keygen { project, write } => {
            let key = indexnow::keygen()?;
            println!("{key}");
            let detected = aieo_site::preset::detect(&project, None);
            if write {
                let dir = detected
                    .public_dir()
                    .context("no static-files directory for a plain build directory: put the key file next to the site's files yourself")?;
                let path = project.join(&dir).join(format!("{key}.txt"));
                std::fs::create_dir_all(path.parent().expect("dir"))?;
                std::fs::write(&path, &key)?;
                eprintln!(
                    "wrote {} — deploy it, then `aieo indexnow verify`",
                    path.display()
                );
            }
            eprintln!("add to aieo.toml:\n\n[indexnow]\nkey = \"{key}\"\n");
            Ok(0)
        }
        IndexNowCommand::Verify(args) => {
            let (config, _) = Config::load_or_default(&args.target.project)?;
            let key = key_of(&args, &config)?;
            let run = run_check(&args.target, None, Vec::new(), "check")?;
            let site = &run.prepared.site;
            let path = format!("{}/{key}.txt", site.base_path);
            let built = site
                .resolve_exact(&path)
                .and_then(|f| site.read_bytes(f))
                .map(|b| String::from_utf8_lossy(&b).trim().to_string());
            let url =
                indexnow::key_url(&site.origin, &key, config.indexnow.key_location.as_deref())?;
            match built.as_deref() {
                Some(k) if k == key => eprintln!("build: {path} holds the key"),
                Some(_) => eprintln!("build: {path} holds a different key"),
                None => {
                    eprintln!("build: no {path} (run `aieo indexnow keygen --write`, then build)")
                }
            }
            let live = indexnow::verify(&Ureq::default(), &url, &key)?;
            match &live {
                KeyFile::Valid => eprintln!("live: {url} holds the key"),
                KeyFile::Wrong(body) => eprintln!("live: {url} answers something else: {body:?}"),
                KeyFile::Missing(s) => eprintln!("live: {url} answers HTTP {s}"),
            }
            Ok(if live == KeyFile::Valid {
                0
            } else {
                crate::EXIT_GATE
            })
        }
        IndexNowCommand::Submit {
            args,
            baseline,
            all,
            urls,
            yes,
        } => submit(args, baseline, all, urls, yes),
    }
}

/// Indexable URLs that are new or changed since the baseline, and baseline
/// URLs that are gone (engines should see their redirect or 404).
pub fn changed_urls(
    current: &Bundle,
    baseline: &Bundle,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let before: BTreeMap<&str, &str> = baseline
        .inventory
        .pages
        .iter()
        .filter(|p| p.indexable)
        .map(|p| (p.url.as_str(), p.hash.as_str()))
        .collect();
    let now: BTreeMap<&str, &str> = current
        .inventory
        .pages
        .iter()
        .filter(|p| p.indexable)
        .map(|p| (p.url.as_str(), p.hash.as_str()))
        .collect();
    let added = now
        .keys()
        .filter(|u| !before.contains_key(*u))
        .map(|u| u.to_string())
        .collect();
    let changed = now
        .iter()
        .filter(|(u, h)| before.get(*u).is_some_and(|b| b != *h))
        .map(|(u, _)| u.to_string())
        .collect();
    let removed = before
        .keys()
        .filter(|u| !now.contains_key(*u))
        .map(|u| u.to_string())
        .collect();
    (added, changed, removed)
}

fn submit(
    args: IndexNowArgs,
    baseline: Option<PathBuf>,
    all: bool,
    urls: Option<PathBuf>,
    yes: bool,
) -> Result<u8> {
    let root = args.target.project.clone();
    let (config, _) = Config::load_or_default(&root)?;
    let key = key_of(&args, &config)?;
    let run = run_check(&args.target, None, Vec::new(), "check")?;
    let origin = run.prepared.site.origin.clone();
    let list: Vec<String> = if let Some(file) = &urls {
        std::fs::read_to_string(file)?
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(String::from)
            .collect()
    } else if all {
        run.bundle
            .inventory
            .pages
            .iter()
            .filter(|p| p.indexable)
            .map(|p| p.url.clone())
            .collect()
    } else {
        let base = load_baseline(baseline.as_ref(), config.check.baseline.as_deref(), &root, false)?
            .or_else(|| Bundle::read(&root.join(".aieo/baseline.json")).ok())
            .context("no baseline to compare with: `aieo snapshot` after the last submission, or pass --all / --urls")?;
        let (added, changed, removed) = changed_urls(&run.bundle, &base);
        eprintln!(
            "since the baseline: {} new, {} changed, {} removed",
            added.len(),
            changed.len(),
            removed.len()
        );
        added.into_iter().chain(changed).chain(removed).collect()
    };
    let endpoint = args
        .endpoint
        .clone()
        .or(config.indexnow.endpoint.clone())
        .unwrap_or_else(|| indexnow::DEFAULT_ENDPOINT.into());
    let plan = indexnow::plan(
        &endpoint,
        &origin,
        &key,
        config.indexnow.key_location.as_deref(),
        list,
    )?;
    for r in &plan.rejected {
        eprintln!("skipped (not on {}): {r}", plan.host);
    }
    if plan.urls() == 0 {
        eprintln!("nothing to submit");
        return Ok(0);
    }
    if !yes {
        for u in plan.batches.iter().flatten().take(20) {
            println!("{u}");
        }
        if plan.urls() > 20 {
            println!("… {} more", plan.urls() - 20);
        }
        eprintln!(
            "plan: {} URL(s) in {} request(s) to {} — run again with --yes to send",
            plan.urls(),
            plan.batches.len(),
            plan.endpoint
        );
        return Ok(0);
    }
    let http = Ureq::default();
    let key_url = indexnow::key_url(&origin, &key, config.indexnow.key_location.as_deref())?;
    if indexnow::verify(&http, &key_url, &key)? != KeyFile::Valid {
        bail!("{key_url} does not serve the key yet: deploy the key file first (`aieo indexnow verify`)");
    }
    let sent = indexnow::submit(&http, &plan)?;
    let store = SearchStore::open(&aieo_dir(&root).join("search.db"))?;
    let mut failed = false;
    for s in &sent {
        let meaning = indexnow::meaning(s.status);
        eprintln!("{} URL(s): HTTP {} — {meaning}", s.urls, s.status);
        store.record_indexnow(&plan.endpoint, &plan.host, s.urls, s.status, meaning)?;
        audit(
            &aieo_dir(&root),
            "indexnow.submit",
            &plan.endpoint,
            s.urls as u64,
            Some(s.status),
            meaning,
        )?;
        failed |= !matches!(s.status, 200 | 202);
    }
    Ok(if failed { crate::EXIT_GATE } else { 0 })
}
