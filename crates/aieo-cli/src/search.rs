//! `aieo search …`: Search Console and Bing Webmaster evidence pulled with
//! the user's own credentials into `.aieo/search.db`, and its report.

use std::fmt::Write as _;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Subcommand;
use time::{Duration, OffsetDateTime};

use aieo_core::bundle::{Bundle, Search};
use aieo_core::config::{resolve_settings, Config};
use aieo_core::spec::spec;
use aieo_core::Finding;
use aieo_rules::emit::Emitter;
use aieo_search::creds::Credentials;
use aieo_search::google_auth::{self, ServiceAccount};
use aieo_search::http::{Http, Ureq};
use aieo_search::report::{self, Page, Params};
use aieo_search::store::SearchStore;
use aieo_search::{bing, bing_ai, google, indexnow};

use crate::{run_check, Target};

#[derive(Subcommand)]
pub enum SearchCommand {
    /// Google Search Console (Search Analytics, URL Inspection)
    #[command(subcommand)]
    Google(GoogleCommand),
    /// Bing Webmaster Tools (query and page stats, AI Performance export)
    #[command(subcommand)]
    Bing(BingCommand),
    /// What search engines report about the site, read against the build
    Report {
        #[command(flatten)]
        target: Target,
        /// Days in the window (and the period compared with)
        #[arg(long, default_value_t = 28)]
        days: i64,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum GoogleCommand {
    /// Authorize with your own Desktop-app OAuth client (browser, loopback redirect)
    Login {
        /// The client JSON downloaded from Google Cloud Console (or AIEO_GOOGLE_CLIENT_ID / _SECRET)
        #[arg(long)]
        client_secret: Option<PathBuf>,
        /// Name for this login in the credentials file
        #[arg(long, default_value = "default")]
        account: String,
        /// Print the URL instead of opening a browser
        #[arg(long)]
        no_browser: bool,
    },
    /// List the Search Console properties the credentials reach
    Properties {
        #[arg(short = 'C', long, default_value = ".")]
        project: PathBuf,
    },
    /// Pull Search Analytics: clicks per page and day, top queries
    Pull {
        #[command(flatten)]
        target: Target,
        #[arg(long, default_value_t = 28)]
        days: i64,
    },
    /// Inspect the build's indexable pages with URL Inspection, stalest first, within the daily quota
    Inspect {
        #[command(flatten)]
        target: Target,
        /// At most this many today
        #[arg(long)]
        limit: Option<usize>,
        /// Inspect these URLs (one per line) instead
        #[arg(long)]
        urls: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum BingCommand {
    /// Pull query and page stats with an API key (BING_WEBMASTER_API_KEY or `aieo search bing key`)
    Pull {
        #[command(flatten)]
        target: Target,
    },
    /// Store a Bing Webmaster API key (Settings → API Access)
    Key,
    /// Import the AI Performance overview CSV export (Date, Citations, Cited Pages)
    ImportAi {
        file: PathBuf,
        #[arg(short = 'C', long, default_value = ".")]
        project: PathBuf,
    },
}

fn db(project: &Path) -> Result<SearchStore> {
    SearchStore::open(&project.join(".aieo/search.db"))
}

fn config(project: &Path) -> Result<Config> {
    Ok(Config::load_or_default(project)?.0)
}

/// An access token: a service account (GOOGLE_APPLICATION_CREDENTIALS), else the stored login.
fn google_token(http: &dyn Http, account: &str) -> Result<String> {
    if let Some(path) = std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS") {
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "reading GOOGLE_APPLICATION_CREDENTIALS ({})",
                PathBuf::from(&path).display()
            )
        })?;
        let sa: ServiceAccount = serde_json::from_str(&text)
            .context("GOOGLE_APPLICATION_CREDENTIALS is not a service account key")?;
        return google_auth::service_account_token(http, &sa);
    }
    let mut creds = Credentials::load()?;
    let token = creds
        .data
        .google
        .get_mut(account)
        .context("not logged in to Google: run `aieo search google login` (or set GOOGLE_APPLICATION_CREDENTIALS to a service account key)")?;
    let (access, changed) = google_auth::access_token(http, token)?;
    if changed {
        creds.save()?;
    }
    Ok(access)
}

fn account(config: &Config) -> String {
    config
        .search
        .google_account
        .clone()
        .unwrap_or_else(|| "default".into())
}

fn open_browser(url: &str) {
    let cmd = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(cmd)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// The property for the site: aieo.toml, else the one covering the origin.
fn property(http: &dyn Http, token: &str, config: &Config, origin: &url::Url) -> Result<String> {
    if let Some(p) = &config.search.google_property {
        return Ok(p.clone());
    }
    let props = google::properties(http, token)?;
    google::property_for(&props, origin).with_context(|| {
        format!(
            "no Search Console property covers {origin}; set [search] google_property in aieo.toml ({} available)",
            props.len()
        )
    })
}

fn origin_of(target: &Target, config: &Config) -> Result<url::Url> {
    let s = target
        .site
        .clone()
        .or(config.site.clone())
        .context("pass --site or set `site` in aieo.toml")?;
    Ok(url::Url::parse(&s)?)
}

pub fn cmd_search(cmd: SearchCommand) -> Result<u8> {
    let http = Ureq::default();
    match cmd {
        SearchCommand::Google(g) => match g {
            GoogleCommand::Login {
                client_secret,
                account,
                no_browser,
            } => {
                let client = match client_secret {
                    Some(p) => google_auth::client_from_json(&std::fs::read_to_string(&p)?)?,
                    None => google_auth::Client {
                        id: std::env::var("AIEO_GOOGLE_CLIENT_ID")
                            .context("pass --client-secret <file> (a Desktop app OAuth client from Google Cloud Console) or set AIEO_GOOGLE_CLIENT_ID")?,
                        secret: std::env::var("AIEO_GOOGLE_CLIENT_SECRET").ok(),
                    },
                };
                let listener = TcpListener::bind("127.0.0.1:0")?;
                let redirect = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
                let pkce = google_auth::pkce()?;
                let url = google_auth::authorize_url(&client, &redirect, &pkce);
                eprintln!("Open this URL to authorize read-only Search Console access:\n\n{url}\n");
                if !no_browser {
                    open_browser(&url);
                }
                let code = google_auth::receive_code(&listener, &pkce.state)?;
                let token = google_auth::exchange(&http, &client, &code, &redirect, &pkce)?;
                let mut creds = Credentials::load()?;
                creds.data.google.insert(account.clone(), token);
                creds.save()?;
                eprintln!(
                    "authorized; stored as {account:?} in {}",
                    creds.path.display()
                );
                Ok(0)
            }
            GoogleCommand::Properties { project } => {
                let config = config(&project)?;
                let token = google_token(&http, &account(&config))?;
                for p in google::properties(&http, &token)? {
                    println!("{}\t{}", p.site_url, p.permission);
                }
                Ok(0)
            }
            GoogleCommand::Pull { target, days } => {
                let config = config(&target.project)?;
                let origin = origin_of(&target, &config)?;
                let token = google_token(&http, &account(&config))?;
                let prop = property(&http, &token, &config, &origin)?;
                // finalized data trails by a few days; pull two windows for comparisons
                let end = OffsetDateTime::now_utc().date() - Duration::days(3);
                let start = end - Duration::days(2 * days - 1);
                let store = db(&target.project)?;
                let pages = google::analytics(
                    &http,
                    &token,
                    &prop,
                    start,
                    end,
                    &["date", "page"],
                    2_000_000,
                )?;
                google::save_pages(&store, &prop, &pages)?;
                let q_start = end - Duration::days(days - 1);
                let queries =
                    google::analytics(&http, &token, &prop, q_start, end, &["query"], 5_000)?;
                google::save_queries(&store, &prop, q_start, end, &queries)?;
                eprintln!(
                    "{prop}: {} page-day rows ({start} → {end}), {} queries for the last {days} days",
                    pages.len(),
                    queries.len()
                );
                Ok(0)
            }
            GoogleCommand::Inspect {
                target,
                limit,
                urls,
            } => {
                let config = config(&target.project)?;
                let token = google_token(&http, &account(&config))?;
                let (origin, list) = match &urls {
                    Some(file) => (
                        origin_of(&target, &config)?,
                        std::fs::read_to_string(file)?
                            .lines()
                            .map(str::trim)
                            .filter(|l| !l.is_empty())
                            .map(String::from)
                            .collect::<Vec<_>>(),
                    ),
                    None => {
                        let run = run_check(&target, None, Vec::new(), "check")?;
                        let urls = run
                            .bundle
                            .inventory
                            .pages
                            .iter()
                            .filter(|p| p.indexable)
                            .map(|p| p.url.clone())
                            .collect();
                        (run.prepared.site.origin.clone(), urls)
                    }
                };
                let prop = property(&http, &token, &config, &origin)?;
                let store = db(&target.project)?;
                // the quota is Pacific-time days; UTC is close enough for a ledger that stops early
                let today = OffsetDateTime::now_utc().date().to_string();
                let run =
                    google::inspect_many(&http, &token, &prop, &list, &store, &today, limit, true)?;
                let not_indexed = run
                    .inspected
                    .iter()
                    .filter(|i| i.verdict.as_deref() != Some("PASS"))
                    .count();
                eprintln!(
                    "{prop}: inspected {} URL(s) ({not_indexed} not indexed); {} left for later (daily quota {})",
                    run.inspected.len(),
                    run.deferred,
                    google::INSPECT_DAILY
                );
                for e in run.errors.iter().take(5) {
                    eprintln!("  {e}");
                }
                Ok(if run.errors.is_empty() {
                    0
                } else {
                    crate::EXIT_INCOMPLETE
                })
            }
        },
        SearchCommand::Bing(b) => match b {
            BingCommand::Key => {
                let key = rpassword::prompt_password("Bing Webmaster API key: ")?
                    .trim()
                    .to_string();
                let sites = bing::sites(&http, &key)?;
                let mut creds = Credentials::load()?;
                creds.data.bing_api_key = Some(key);
                creds.save()?;
                eprintln!("stored in {}; sites:", creds.path.display());
                for (url, verified) in sites {
                    eprintln!("  {url}{}", if verified { "" } else { " (not verified)" });
                }
                Ok(0)
            }
            BingCommand::Pull { target } => {
                let config = config(&target.project)?;
                let key = Credentials::load()?.bing_key().context(
                    "no Bing API key: set BING_WEBMASTER_API_KEY or run `aieo search bing key`",
                )?;
                let site = match &config.search.bing_site {
                    Some(s) => s.clone(),
                    None => {
                        let mut s = origin_of(&target, &config)?.origin().ascii_serialization();
                        s.push('/');
                        s
                    }
                };
                let pages = bing::page_stats(&http, &key, &site)?;
                let queries = bing::query_stats(&http, &key, &site)?;
                let issues = bing::crawl_issues(&http, &key, &site).unwrap_or_default();
                bing::save(&db(&target.project)?, &site, &pages, &queries)?;
                eprintln!(
                    "{site}: {} page rows, {} query rows, {} URLs with crawl issues",
                    pages.len(),
                    queries.len(),
                    issues.len()
                );
                for i in issues.iter().take(10) {
                    eprintln!("  {} HTTP {} (issues {})", i.url, i.http_code, i.issues);
                }
                Ok(0)
            }
            BingCommand::ImportAi { file, project } => {
                let config = config(&project)?;
                let days = bing_ai::parse_overview(
                    &std::fs::read(&file)?,
                    OffsetDateTime::now_utc().date(),
                )?;
                let site = config
                    .search
                    .bing_site
                    .clone()
                    .or(config
                        .site
                        .clone()
                        .map(|s| format!("{}/", s.trim_end_matches('/'))))
                    .context("set `site` (or [search] bing_site) in aieo.toml")?;
                bing::save_ai(&db(&project)?, &site, &days)?;
                let total: u64 = days.iter().map(|d| d.citations).sum();
                eprintln!(
                    "{}: {} days ({} → {}), {total} citations",
                    file.display(),
                    days.len(),
                    days[0].day,
                    days[days.len() - 1].day
                );
                Ok(0)
            }
        },
        SearchCommand::Report { target, days, json } => {
            let (bundle_search, findings, run_bundle) = build(&target, days)?
                .context("nothing pulled yet: `aieo search google pull`, `aieo search google inspect` or `aieo search bing pull`")?;
            if json {
                let mut b = run_bundle;
                b.findings.clear();
                b.readiness = None;
                b.run.kinds = vec!["search".into()];
                aieo_report::attach_search(&mut b, bundle_search, findings, &[]);
                print!("{}", b.to_json());
                return Ok(0);
            }
            print!("{}", render(&bundle_search, &findings));
            Ok(0)
        }
    }
}

/// The search section and S findings for the project (None without data).
pub fn build(target: &Target, days: i64) -> Result<Option<(Search, Vec<Finding>, Bundle)>> {
    let path = target.project.join(".aieo/search.db");
    if !path.is_file() {
        return Ok(None);
    }
    let config = config(&target.project)?;
    let store = SearchStore::open(&path)?;
    let run = run_check(target, None, Vec::new(), "search")?;
    let pages: Vec<Page> = run
        .bundle
        .inventory
        .pages
        .iter()
        .map(|p| Page {
            url: p.url.clone(),
            indexable: p.indexable,
        })
        .collect();
    let site = &run.prepared.site;
    let key_in_build = config.indexnow.key.as_ref().and_then(|k| {
        crate::cloud::key_file_in_build(site, k, config.indexnow.key_location.as_deref())
            .map(|(_, held)| held.as_deref() == Some(k.as_str()))
    });
    // the property recorded with the data (no network needed to report)
    let property: Option<String> = config.search.google_property.clone().or_else(|| {
        store
            .conn
            .query_row(
                "SELECT property FROM (SELECT property FROM gsc_page_daily UNION SELECT property FROM inspections) LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok()
    });
    let bing_site: Option<String> = config.search.bing_site.clone().or_else(|| {
        store
            .conn
            .query_row(
                "SELECT site FROM (SELECT site FROM bing_page_daily UNION SELECT site FROM bing_ai_daily) LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok()
    });
    let endpoint = config
        .indexnow
        .endpoint
        .clone()
        .unwrap_or_else(|| indexnow::DEFAULT_ENDPOINT.into());
    let origin = site.origin_str();
    let params = Params {
        property: property.as_deref(),
        bing_site: bing_site.as_deref(),
        days,
        pages: &pages,
        indexnow_key_in_build: key_in_build,
        indexnow_endpoint: &endpoint,
        origin: &origin,
    };
    let Some(search) = report::build(&store, &params)? else {
        return Ok(None);
    };
    let settings = resolve_settings(&config, spec(), OffsetDateTime::now_utc().date())?;
    let mut e = Emitter::new(&settings);
    report::findings(&store, &params, &mut e)?;
    let findings = e.out.into_iter().map(|p| p.finding).collect();
    Ok(Some((search, findings, run.bundle)))
}

fn render(s: &Search, findings: &[Finding]) -> String {
    let mut out = String::new();
    if let Some(g) = &s.google {
        let _ = writeln!(
            out,
            "Google  {}  {} → {}\n  clicks {:.0} · impressions {:.0} · CTR {:.1}% · avg position {:.1}",
            g.property,
            g.start,
            g.end,
            g.totals.clicks,
            g.totals.impressions,
            g.totals.ctr * 100.0,
            g.totals.position
        );
        for p in g.pages.iter().take(10) {
            let _ = writeln!(out, "  {:>7.0} {:>9.0}  {}", p.clicks, p.impressions, p.key);
        }
        if !g.inspections.is_empty() {
            let indexed = g
                .inspections
                .iter()
                .filter(|i| i.verdict.as_deref() == Some("PASS"))
                .count();
            let _ = writeln!(
                out,
                "  URL Inspection: {indexed}/{} indexed",
                g.inspections.len()
            );
        }
    }
    if let Some(b) = &s.bing {
        let clicks: u64 = b.pages.iter().map(|p| p.clicks).sum();
        let _ = writeln!(
            out,
            "Bing  {}\n  clicks {clicks} over {} pages",
            b.site,
            b.pages.len()
        );
        if !b.ai.is_empty() {
            let cites: u64 = b.ai.iter().map(|d| d.citations).sum();
            let _ = writeln!(
                out,
                "  AI citations {cites} ({} → {})",
                b.ai[0].day,
                b.ai[b.ai.len() - 1].day
            );
        }
    }
    if let Some(i) = &s.indexnow {
        let last = &i.submissions[0];
        let _ = writeln!(
            out,
            "IndexNow  last submission {}: {} URL(s), HTTP {}",
            last.at, last.urls, last.status
        );
    }
    if !findings.is_empty() {
        let _ = writeln!(out, "\nFindings");
        for f in findings {
            let _ = writeln!(
                out,
                "  {:<8} {:<26} {}  {}",
                f.tier.as_str(),
                f.rule,
                f.url.as_deref().unwrap_or(""),
                f.evidence
                    .iter()
                    .map(|e| e.detail.as_str())
                    .collect::<Vec<_>>()
                    .join(" · ")
            );
        }
    }
    out
}

/// Search evidence for `aieo export`.
pub fn for_export(target: &Target) -> Result<Option<(Search, Vec<Finding>)>> {
    Ok(build(target, 28)?.map(|(s, f, _)| (s, f)))
}
