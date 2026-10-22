//! Turning rule hits into findings: tier from the spec (or `[rules]`
//! overrides), default-off rules, ignored URLs, suppressions.

use aieo_core::config::{RuleSetting, Settings};
use aieo_core::spec::spec;
use aieo_core::{fingerprint, Evidence, Finding, FindingStatus, Scope, Suppressed, Tier};

/// Where a finding points.
#[derive(Debug, Clone, Default)]
pub struct Loc {
    pub url_path: Option<String>,
    pub url: Option<String>,
    /// build-relative file
    pub file: Option<String>,
    pub source: Option<String>,
}

/// A finding plus the text to look for in `file` to give it a line number.
#[derive(Debug, Clone)]
pub struct Pending {
    pub finding: Finding,
    pub needle: Option<String>,
}

pub struct Emitter<'a> {
    settings: &'a Settings,
    pub out: Vec<Pending>,
}

impl<'a> Emitter<'a> {
    pub fn new(settings: &'a Settings) -> Self {
        Emitter {
            settings,
            out: Vec::new(),
        }
    }

    /// Whether `rule` produces findings in this run.
    pub fn enabled(&self, rule: &str) -> bool {
        let r = spec().expect_rule(rule);
        match self.settings.rules.get(rule) {
            Some(RuleSetting::Off) => false,
            Some(RuleSetting::On | RuleSetting::Tier(_)) => r.implemented_here(),
            None => r.default_enabled && r.implemented_here(),
        }
    }

    /// Record a hit. `tier`: the module tier for F results (fail/partial);
    /// None = the rule's default tier. `key` makes the fingerprint unique
    /// among hits of one rule on one URL.
    pub fn emit(
        &mut self,
        rule: &str,
        tier: Option<Tier>,
        loc: Loc,
        key: &str,
        evidence: Vec<Evidence>,
        needle: Option<String>,
    ) {
        if !self.enabled(rule) {
            return;
        }
        let r = spec().expect_rule(rule);
        if r.scope == Scope::Page
            && loc
                .url_path
                .as_deref()
                .is_some_and(|p| self.settings.is_ignored(p))
        {
            return;
        }
        let tier = match self.settings.rules.get(rule) {
            Some(RuleSetting::Tier(t)) => Some(*t),
            _ => tier.or(r.tier.default),
        };
        let Some(tier) = tier else { return };
        let suppressed = self
            .settings
            .suppression_for(rule, loc.url_path.as_deref())
            .map(|s| Suppressed {
                reason: s.reason.clone(),
                owner: s.owner.clone(),
                expires: s.expires.clone(),
            });
        let finding = Finding {
            rule: rule.to_string(),
            fingerprint: fingerprint(rule, loc.url_path.as_deref().unwrap_or(""), key),
            tier,
            scope: r.scope,
            url: loc.url,
            file: loc.file,
            line: None,
            source: loc.source,
            message: r.title.clone(),
            evidence,
            evidence_level: r.evidence_level,
            status: FindingStatus::Unknown,
            suppressed,
        };
        self.out.push(Pending { finding, needle });
    }
}
