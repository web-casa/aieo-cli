use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::spec::EvidenceLevel;

/// CI tier of a finding: `error` fails a build by default, `warning` fails
/// it when new, `notice` is advice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Notice,
    Warning,
    Error,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Error, Tier::Warning, Tier::Notice];

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Error => "error",
            Tier::Warning => "warning",
            Tier::Notice => "notice",
        }
    }

    pub fn parse(s: &str) -> Option<Tier> {
        match s {
            "error" => Some(Tier::Error),
            "warning" => Some(Tier::Warning),
            "notice" => Some(Tier::Notice),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Site,
    Page,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Localized {
    pub en: String,
    pub zh: String,
}

impl Localized {
    pub fn new(en: impl Into<String>, zh: impl Into<String>) -> Self {
        Localized {
            en: en.into(),
            zh: zh.into(),
        }
    }

    pub fn get(&self, lang: Lang) -> &str {
        match lang {
            Lang::En => &self.en,
            Lang::Zh => &self.zh,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Zh,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub label: Localized,
    pub detail: String,
}

impl Evidence {
    pub fn new(label: Localized, detail: impl Into<String>) -> Self {
        Evidence {
            label,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingStatus {
    New,
    Existing,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suppressed {
    pub reason: String,
    pub owner: String,
    pub expires: String,
}

/// One defect, located as precisely as the evidence allows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub rule: String,
    pub fingerprint: String,
    pub tier: Tier,
    pub scope: Scope,
    pub url: Option<String>,
    /// build-output file, relative to the build directory
    pub file: Option<String>,
    pub line: Option<u32>,
    /// source file the page was built from, relative to the project root
    pub source: Option<String>,
    pub message: Localized,
    pub evidence: Vec<Evidence>,
    pub evidence_level: EvidenceLevel,
    pub status: FindingStatus,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub suppressed: Option<Suppressed>,
}

/// Stable identity of a finding across runs: rule + URL path (not the origin,
/// so a preview build and production agree) + a rule-specific key.
pub fn fingerprint(rule: &str, url_path: &str, key: &str) -> String {
    let mut h = Sha256::new();
    h.update(rule.as_bytes());
    h.update([0]);
    h.update(url_path.as_bytes());
    h.update([0]);
    h.update(key.as_bytes());
    let digest = h.finalize();
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_are_stable_and_short() {
        let a = fingerprint("C-LINK-BROKEN", "/a/", "/b");
        assert_eq!(a, fingerprint("C-LINK-BROKEN", "/a/", "/b"));
        assert_ne!(a, fingerprint("C-LINK-BROKEN", "/a/", "/c"));
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn tiers_order_by_severity() {
        assert!(Tier::Error > Tier::Warning && Tier::Warning > Tier::Notice);
    }
}
