//! The rule specification shared with aieo.ee (`@aieo/rules`), embedded at
//! build time from `spec/` (refresh it with
//! `pnpm --filter @aieo/rules export:cli <this repo>`).

use std::sync::OnceLock;

use serde::Deserialize;

use crate::finding::{Localized, Scope, Tier};

#[derive(Debug, Clone, Deserialize)]
pub struct TierSpec {
    #[serde(default)]
    pub default: Option<Tier>,
    #[serde(default)]
    pub fail: Option<Tier>,
    #[serde(default)]
    pub partial: Option<Tier>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleClass {
    Scoring,
    Bonus,
    Info,
    Finding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceLevel {
    Official,
    Heuristic,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuleSpec {
    pub id: String,
    pub family: String,
    pub scope: Scope,
    pub class: RuleClass,
    pub weight: f64,
    #[serde(default)]
    pub category: Option<String>,
    pub title: Localized,
    pub description: Localized,
    pub tier: TierSpec,
    pub modes: Vec<String>,
    pub evidence_level: EvidenceLevel,
    #[serde(default)]
    pub source_url: Option<String>,
    pub implementations: Vec<String>,
    pub default_enabled: bool,
    #[serde(default)]
    pub saas_issue_type: Option<String>,
}

impl RuleSpec {
    pub fn implemented_here(&self) -> bool {
        self.implementations.iter().any(|i| i == "rust")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct VerifySpec {
    pub ip_ranges_url: Option<String>,
    pub rdns_suffixes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CrawlerSpec {
    pub id: String,
    pub name: String,
    pub operator: String,
    pub purpose: String,
    pub robots_token: String,
    pub ua_pattern: Option<String>,
    pub robots_only: bool,
    #[serde(default)]
    pub f03_category: Option<String>,
    pub verify: VerifySpec,
    #[serde(default)]
    pub docs_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RulesFile {
    version: String,
    rules: Vec<RuleSpec>,
}

#[derive(Debug, Deserialize)]
struct CrawlersFile {
    version: String,
    crawlers: Vec<CrawlerSpec>,
}

#[derive(Debug, Deserialize)]
struct SourceFile {
    sha256: String,
}

#[derive(Debug)]
pub struct Spec {
    pub rules_version: String,
    pub rules: Vec<RuleSpec>,
    pub crawlers_version: String,
    pub crawlers: Vec<CrawlerSpec>,
    /// content hash of the exported `@aieo/rules` files (spec/SOURCE.json)
    pub source_sha256: String,
}

impl Spec {
    pub fn rule(&self, id: &str) -> Option<&RuleSpec> {
        self.rules.iter().find(|r| r.id == id)
    }

    /// The spec entry for a rule id the code refers to. A missing id is a
    /// programming error caught by the test suite, never a runtime input.
    pub fn expect_rule(&self, id: &str) -> &RuleSpec {
        self.rule(id)
            .unwrap_or_else(|| panic!("rule {id} is not in spec/rules.json"))
    }

    /// robots.txt token → F03 category, in catalog order.
    pub fn f03_matrix(&self) -> impl Iterator<Item = (&str, &str)> {
        self.crawlers.iter().filter_map(|c| {
            c.f03_category
                .as_deref()
                .map(|cat| (c.robots_token.as_str(), cat))
        })
    }
}

pub const RULES_JSON: &str = include_str!("../../../spec/rules.json");
pub const CRAWLERS_JSON: &str = include_str!("../../../spec/crawlers.json");
pub const BUNDLE_SCHEMA_JSON: &str = include_str!("../../../spec/bundle.schema.json");
const SOURCE_JSON: &str = include_str!("../../../spec/SOURCE.json");

pub fn spec() -> &'static Spec {
    static SPEC: OnceLock<Spec> = OnceLock::new();
    SPEC.get_or_init(|| {
        let rules: RulesFile = serde_json::from_str(RULES_JSON).expect("spec/rules.json");
        let crawlers: CrawlersFile =
            serde_json::from_str(CRAWLERS_JSON).expect("spec/crawlers.json");
        let source: SourceFile = serde_json::from_str(SOURCE_JSON).expect("spec/SOURCE.json");
        Spec {
            rules_version: rules.version,
            rules: rules.rules,
            crawlers_version: crawlers.version,
            crawlers: crawlers.crawlers,
            source_sha256: source.sha256,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_the_spec() {
        let s = spec();
        assert_eq!(s.rules.iter().filter(|r| r.family == "F").count(), 18);
        assert_eq!(s.expect_rule("F03").class, RuleClass::Bonus);
        assert_eq!(s.f03_matrix().count(), 15);
        assert_eq!(s.f03_matrix().next(), Some(("oai-searchbot", "search")));
        assert_eq!(s.source_sha256.len(), 64);
    }
}
