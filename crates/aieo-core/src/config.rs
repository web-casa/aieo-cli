//! `aieo.toml` — project configuration.
//!
//! ```toml
//! site = "https://example.com"   # production origin (canonical host)
//! dist = "dist"                  # build output directory
//! preset = "starlight"           # auto | static | astro | starlight
//!
//! [check]
//! fail_on = ["error"]            # gate without a baseline
//! baseline = ".aieo/baseline.json"
//! ignore = ["/drafts/**"]        # URL paths left out of page rules
//!
//! [rules]
//! "C-ORPHAN-PAGE" = "notice"     # error | warning | notice | off | on
//!
//! [[suppress]]
//! rule = "C-LINK-BROKEN"
//! url = "/legacy/**"
//! reason = "old docs, removed in Q1"
//! owner = "docs-team"
//! expires = "2026-12-31"
//!
//! [aieo]
//! project = "…"                  # aieo.ee project id for `aieo push`
//!
//! [search]
//! google_property = "sc-domain:example.com"   # or "https://example.com/"
//! bing_site = "https://example.com/"
//!
//! [indexnow]
//! key = "…"                      # public; its key file is served at /<key>.txt
//!
//! # what middleware or the host does that no file in the build says
//! [[hosting.rewrites]]           # applies when no file answers the path
//! from = "/docs/:path*"
//! to = "/en/docs/:path*"
//! [[hosting.redirects]]          # applies before any file
//! from = "/old"
//! to = "/new"
//! status = 301
//! ```

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{bail, Context, Result};
use globset::{Glob, GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use time::macros::format_description;
use time::Date;

use crate::finding::{Scope, Tier};
use crate::spec::Spec;

pub const CONFIG_FILE: &str = "aieo.toml";

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub site: Option<String>,
    pub dist: Option<String>,
    pub preset: Option<String>,
    #[serde(default)]
    pub check: CheckConfig,
    #[serde(default)]
    pub rules: BTreeMap<String, String>,
    #[serde(default)]
    pub suppress: Vec<SuppressRule>,
    #[serde(default)]
    pub aieo: AieoConfig,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub indexnow: IndexNowConfig,
    #[serde(default)]
    pub hosting: HostingConfig,
}

/// Rules middleware or the host applies that the build cannot show
/// (path-to-regexp sources: `:name`, `:name*`, `(regex)`).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostingConfig {
    #[serde(default)]
    pub rewrites: Vec<HostRule>,
    #[serde(default)]
    pub redirects: Vec<HostRule>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRule {
    pub from: String,
    pub to: String,
    /// redirects only (default 308)
    pub status: Option<u16>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AieoConfig {
    /// aieo.ee project id the bundles are pushed to
    pub project: Option<String>,
    /// aieo.ee base URL (default https://aieo.ee)
    pub endpoint: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchConfig {
    /// Search Console property: `sc-domain:example.com` or a URL prefix
    pub google_property: Option<String>,
    /// Bing Webmaster Tools site URL
    pub bing_site: Option<String>,
    /// account label in the credentials file (default "default")
    pub google_account: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexNowConfig {
    pub key: Option<String>,
    /// where the key file is served when not at `/<key>.txt`
    pub key_location: Option<String>,
    /// default https://api.indexnow.org/indexnow
    pub endpoint: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckConfig {
    pub fail_on: Option<Vec<String>>,
    pub baseline: Option<String>,
    #[serde(default)]
    pub ignore: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuppressRule {
    pub rule: String,
    pub url: Option<String>,
    pub reason: String,
    pub owner: String,
    pub expires: String,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// `aieo.toml` in `dir`, or the default configuration when there is none.
    pub fn load_or_default(dir: &Path) -> Result<(Config, Option<std::path::PathBuf>)> {
        let path = dir.join(CONFIG_FILE);
        if path.is_file() {
            Ok((Config::load(&path)?, Some(path)))
        } else {
            Ok((Config::default(), None))
        }
    }
}

/// One gate condition: fail when a finding at `tier` or above exists
/// (`new_only`: and is new against the baseline).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateCondition {
    pub tier: Tier,
    pub new_only: bool,
}

impl GateCondition {
    pub fn parse(s: &str) -> Result<GateCondition> {
        let (new_only, tier) = match s.strip_prefix("new:") {
            Some(rest) => (true, rest),
            None => (false, s),
        };
        let Some(tier) = Tier::parse(tier) else {
            bail!("invalid fail-on condition {s:?} (expected error|warning|notice, optionally prefixed with new:)");
        };
        Ok(GateCondition { tier, new_only })
    }

    pub fn label(&self) -> String {
        if self.new_only {
            format!("new:{}", self.tier.as_str())
        } else {
            self.tier.as_str().to_string()
        }
    }
}

pub fn parse_fail_on(items: &[String]) -> Result<Vec<GateCondition>> {
    items
        .iter()
        .flat_map(|s| s.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "never")
        .map(GateCondition::parse)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleSetting {
    Off,
    On,
    Tier(Tier),
}

#[derive(Debug, Clone)]
pub struct Suppression {
    pub rule: String,
    pub url: Option<GlobMatcher>,
    pub url_pattern: Option<String>,
    pub reason: String,
    pub owner: String,
    pub expires: String,
}

impl Suppression {
    pub fn matches(&self, rule: &str, url_path: Option<&str>) -> bool {
        if self.rule != rule {
            return false;
        }
        match (&self.url, url_path) {
            (None, _) => true,
            (Some(glob), Some(path)) => glob.is_match(path),
            (Some(_), None) => false,
        }
    }
}

/// Configuration checked against the rule spec.
#[derive(Debug, Default)]
pub struct Settings {
    pub rules: HashMap<String, RuleSetting>,
    pub suppressions: Vec<Suppression>,
    /// suppressions past their expiry date: no longer applied, reported
    pub expired: Vec<SuppressRule>,
    pub ignore: Option<GlobSet>,
}

impl Settings {
    pub fn is_ignored(&self, url_path: &str) -> bool {
        self.ignore.as_ref().is_some_and(|g| g.is_match(url_path))
    }

    pub fn suppression_for(&self, rule: &str, url_path: Option<&str>) -> Option<&Suppression> {
        self.suppressions.iter().find(|s| s.matches(rule, url_path))
    }
}

fn url_glob(pattern: &str) -> Result<Glob> {
    // `*` stays inside one path segment, `**` crosses segments
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .with_context(|| format!("invalid URL pattern {pattern:?}"))
}

fn matches_everything(pattern: &str) -> bool {
    matches!(pattern.trim(), "" | "*" | "**" | "/**" | "/*" | "**/*")
}

pub fn resolve_settings(config: &Config, spec: &Spec, today: Date) -> Result<Settings> {
    let mut settings = Settings::default();

    for (id, value) in &config.rules {
        if spec.rule(id).is_none() {
            bail!("[rules] names an unknown rule {id:?} (see `aieo rules`)");
        }
        let setting = match value.as_str() {
            "off" => RuleSetting::Off,
            "on" => RuleSetting::On,
            other => match Tier::parse(other) {
                Some(t) => RuleSetting::Tier(t),
                None => {
                    bail!("[rules] {id} = {other:?}: expected error, warning, notice, off or on")
                }
            },
        };
        settings.rules.insert(id.clone(), setting);
    }

    let date_format = format_description!("[year]-[month]-[day]");
    for s in &config.suppress {
        let Some(rule) = spec.rule(&s.rule) else {
            bail!("[[suppress]] names an unknown rule {:?}", s.rule);
        };
        if s.reason.trim().is_empty() || s.owner.trim().is_empty() {
            bail!("[[suppress]] for {} needs a reason and an owner", s.rule);
        }
        let expires = Date::parse(&s.expires, &date_format)
            .with_context(|| format!("[[suppress]] for {}: expires must be YYYY-MM-DD", s.rule))?;
        // a page rule silenced for every URL is the rule switched off — say so
        // in [rules] instead of hiding it behind a suppression
        let broad = s.url.as_deref().is_none_or(matches_everything);
        if broad && rule.scope == Scope::Page {
            bail!(
                "[[suppress]] for {} covers every URL; give it a narrower url pattern or set \"{}\" = \"off\" in [rules]",
                s.rule,
                s.rule
            );
        }
        if expires < today {
            settings.expired.push(s.clone());
            continue;
        }
        let url = match s.url.as_deref() {
            Some(p) => Some(url_glob(p)?.compile_matcher()),
            None => None,
        };
        settings.suppressions.push(Suppression {
            rule: s.rule.clone(),
            url,
            url_pattern: s.url.clone(),
            reason: s.reason.clone(),
            owner: s.owner.clone(),
            expires: s.expires.clone(),
        });
    }

    if !config.check.ignore.is_empty() {
        let mut b = GlobSetBuilder::new();
        for p in &config.check.ignore {
            b.add(url_glob(p)?);
        }
        settings.ignore = Some(b.build()?);
    }
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::spec;
    use time::macros::date;

    fn cfg(text: &str) -> Config {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn parses_gate_conditions() {
        let g = parse_fail_on(&["error,new:warning".into()]).unwrap();
        assert_eq!(
            g,
            vec![
                GateCondition {
                    tier: Tier::Error,
                    new_only: false
                },
                GateCondition {
                    tier: Tier::Warning,
                    new_only: true
                },
            ]
        );
        assert!(parse_fail_on(&["never".into()]).unwrap().is_empty());
        assert!(parse_fail_on(&["fatal".into()]).is_err());
    }

    #[test]
    fn suppressions_need_owner_reason_and_a_narrow_pattern() {
        let today = date!(2026 - 10 - 22);
        let ok = cfg(r#"
            [[suppress]]
            rule = "C-LINK-BROKEN"
            url = "/legacy/**"
            reason = "removed"
            owner = "docs"
            expires = "2026-12-31"
        "#);
        let s = resolve_settings(&ok, spec(), today).unwrap();
        assert!(s
            .suppression_for("C-LINK-BROKEN", Some("/legacy/a/b/"))
            .is_some());
        assert!(s
            .suppression_for("C-LINK-BROKEN", Some("/guide/"))
            .is_none());

        let broad = cfg(r#"
            [[suppress]]
            rule = "C-LINK-BROKEN"
            url = "/**"
            reason = "noise"
            owner = "me"
            expires = "2026-12-31"
        "#);
        assert!(resolve_settings(&broad, spec(), today).is_err());

        let expired = cfg(r#"
            [[suppress]]
            rule = "C-ROBOTS-BLOCKS-ALL"
            reason = "staging"
            owner = "ops"
            expires = "2026-01-01"
        "#);
        let s = resolve_settings(&expired, spec(), today).unwrap();
        assert!(s.suppressions.is_empty());
        assert_eq!(s.expired.len(), 1);
    }

    #[test]
    fn rejects_unknown_rules_and_values() {
        let today = date!(2026 - 10 - 22);
        assert!(resolve_settings(&cfg("[rules]\n\"X-NOPE\" = \"off\""), spec(), today).is_err());
        assert!(
            resolve_settings(&cfg("[rules]\n\"T-MISSING-H1\" = \"fatal\""), spec(), today).is_err()
        );
        let s =
            resolve_settings(&cfg("[rules]\n\"T-THIN-CONTENT\" = \"on\""), spec(), today).unwrap();
        assert_eq!(s.rules["T-THIN-CONTENT"], RuleSetting::On);
    }
}
