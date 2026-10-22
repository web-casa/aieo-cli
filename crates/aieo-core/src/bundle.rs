//! `aieo-bundle/v1` (spec/bundle.schema.json): the report format of every
//! run. `--format json` prints it, `aieo snapshot` saves it as a baseline,
//! `aieo export` gzips it for aieo.ee. Findings and aggregates only — never
//! raw logs, IP addresses or source code.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::finding::{Finding, Tier};

pub const BUNDLE_SCHEMA: &str = "aieo-bundle/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub schema: String,
    pub producer: Producer,
    pub generated_at: String,
    pub site: SiteInfo,
    pub run: RunInfo,
    pub readiness: Option<Readiness>,
    pub inventory: Inventory,
    pub findings: Vec<Finding>,
    pub diff: Option<DiffInfo>,
    pub summary: Summary,
    pub logs: Option<Logs>,
    pub search: Option<serde_json::Value>,
    pub privacy: Privacy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Producer {
    pub name: String,
    pub version: String,
    pub rules_version: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub crawlers_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteInfo {
    pub origin: String,
    pub preset: String,
    #[serde(default)]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunInfo {
    pub kinds: Vec<String>,
    pub mode: String,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub ci: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Readiness {
    pub rules_version: String,
    pub modules: Vec<ReadinessModule>,
    #[serde(with = "scores_serde")]
    pub scores: Scores,
    /// P01–P04 over every content page (absent in bundles before rules 2.1)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub multipage: Option<Multipage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Multipage {
    pub modules: Vec<MultipageModule>,
    pub score: f64,
    pub pages: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultipageModule {
    pub id: String,
    pub status: String,
    pub ratio: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadinessModule {
    pub id: String,
    pub status: String,
    pub ratio: f64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scores {
    pub base: f64,
    pub bonus: f64,
    pub final_score: f64,
    pub incomplete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inventory {
    pub pages: Vec<PageRecord>,
    pub sitemaps: Vec<String>,
    pub robots_txt: bool,
    pub llms_txt: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageRecord {
    pub url: String,
    pub file: String,
    #[serde(default)]
    pub source: Option<String>,
    pub indexable: bool,
    pub noindex: bool,
    pub canonical: Option<String>,
    pub hreflang: BTreeMap<String, String>,
    pub title: Option<String>,
    pub words: u64,
    pub jsonld_types: Vec<String>,
    pub inlinks: u64,
    pub in_sitemap: bool,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffInfo {
    pub baseline_generated_at: String,
    #[serde(default)]
    pub baseline_commit: Option<String>,
    pub new: u64,
    pub existing: u64,
    pub fixed: Vec<FixedFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixedFinding {
    pub rule: String,
    pub fingerprint: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierCounts {
    pub error: u64,
    pub warning: u64,
    pub notice: u64,
}

impl TierCounts {
    pub fn add(&mut self, tier: Tier) {
        match tier {
            Tier::Error => self.error += 1,
            Tier::Warning => self.warning += 1,
            Tier::Notice => self.notice += 1,
        }
    }

    pub fn get(&self, tier: Tier) -> u64 {
        match tier {
            Tier::Error => self.error,
            Tier::Warning => self.warning,
            Tier::Notice => self.notice,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gate {
    pub fail_on: Vec<String>,
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub pages: u64,
    pub indexable: u64,
    pub findings: TierCounts,
    pub new: Option<TierCounts>,
    pub gate: Gate,
    pub incomplete: bool,
    pub incomplete_reasons: Vec<String>,
}

/// Status classes of a group of requests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusBuckets {
    #[serde(rename = "2xx")]
    pub s2: u64,
    #[serde(rename = "3xx")]
    pub s3: u64,
    #[serde(rename = "4xx")]
    pub s4: u64,
    #[serde(rename = "5xx")]
    pub s5: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationCounts {
    pub verified: u64,
    pub mismatch: u64,
    pub unverifiable: u64,
    pub unchecked: u64,
}

/// Crawler and AI-referral aggregates (`aieo logs`); `bundle.schema.json#/properties/logs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Logs {
    pub window: LogWindow,
    pub crawlers_version: String,
    pub sources: Vec<LogSource>,
    pub bots: Vec<LogBot>,
    pub per_url_actor_day: Vec<LogUrlDay>,
    pub referrals: Vec<LogReferral>,
    pub coverage: Vec<LogCoverage>,
    pub anomalies: Vec<LogAnomaly>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogWindow {
    pub from: String,
    pub to: String,
    pub days: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSource {
    pub format: String,
    pub lines: u64,
    pub hits: u64,
    pub skipped: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogBot {
    pub bot: Option<String>,
    pub actor: String,
    pub hits: u64,
    pub verification: VerificationCounts,
    pub status: StatusBuckets,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogUrlDay {
    pub path: String,
    pub actor: String,
    pub bot: Option<String>,
    pub day: String,
    pub hits: u64,
    pub verified: u64,
    pub status: StatusBuckets,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogReferral {
    pub landing: String,
    pub platform: String,
    pub day: String,
    pub visits: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogCoverage {
    pub actor: String,
    pub bot: Option<String>,
    pub indexable: u64,
    pub crawled: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogAnomaly {
    pub day: String,
    pub kind: String,
    pub count: u64,
    pub baseline: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Privacy {
    pub raw_logs_included: bool,
    pub source_included: bool,
    pub ip: String,
}

impl Default for Privacy {
    fn default() -> Self {
        Privacy {
            raw_logs_included: false,
            source_included: false,
            ip: "none".into(),
        }
    }
}

// `final` is a Rust keyword: the struct field is `final_score`, the JSON key `final`.
mod scores_serde {
    use super::Scores;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Wire {
        base: f64,
        bonus: f64,
        #[serde(rename = "final")]
        final_: f64,
        incomplete: bool,
    }

    pub fn serialize<S: Serializer>(s: &Scores, ser: S) -> Result<S::Ok, S::Error> {
        Wire {
            base: s.base,
            bonus: s.bonus,
            final_: s.final_score,
            incomplete: s.incomplete,
        }
        .serialize(ser)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Scores, D::Error> {
        let w = Wire::deserialize(de)?;
        Ok(Scores {
            base: w.base,
            bonus: w.bonus,
            final_score: w.final_,
            incomplete: w.incomplete,
        })
    }
}

impl Bundle {
    /// Read a bundle (`.json` or `.json.gz`) written by this or an earlier CLI.
    pub fn read(path: &Path) -> Result<Bundle> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let text = if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut s = String::new();
            flate2::read::GzDecoder::new(bytes.as_slice())
                .read_to_string(&mut s)
                .with_context(|| format!("decompressing {}", path.display()))?;
            s
        } else {
            String::from_utf8(bytes).context("bundle is not UTF-8")?
        };
        let bundle: Bundle =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if bundle.schema != BUNDLE_SCHEMA {
            bail!(
                "{} is {:?}, expected {BUNDLE_SCHEMA}",
                path.display(),
                bundle.schema
            );
        }
        Ok(bundle)
    }

    /// Pretty JSON, as printed by `--format json` and saved as a baseline.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("bundle serializes")
    }

    /// gzip-compressed JSON, as uploaded to aieo.ee.
    pub fn to_gzip(&self) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(
            serde_json::to_string(self)
                .expect("bundle serializes")
                .as_bytes(),
        )
        .expect("in-memory write");
        enc.finish().expect("in-memory gzip")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_use_the_final_key() {
        #[derive(Serialize)]
        struct W {
            #[serde(with = "scores_serde")]
            s: Scores,
        }
        let json = serde_json::to_string(&W {
            s: Scores {
                base: 1.0,
                bonus: 2.0,
                final_score: 3.0,
                incomplete: false,
            },
        })
        .unwrap();
        assert!(json.contains("\"final\":3.0"), "{json}");
    }
}
