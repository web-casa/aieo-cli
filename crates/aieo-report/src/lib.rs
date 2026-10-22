//! Report formats of a check run: the `aieo-bundle/v1` document (json /
//! gzip), terminal output, SARIF 2.1.0, GitHub workflow annotations and a
//! Markdown summary.

pub mod github;
pub mod markdown;
pub mod pretty;
pub mod sarif;

use aieo_core::bundle::{
    Bundle, DiffInfo, Gate, Inventory, Privacy, Producer, Readiness, ReadinessModule, RunInfo,
    SiteInfo, Summary, TierCounts, BUNDLE_SCHEMA,
};
use aieo_core::config::GateCondition;
use aieo_core::spec::spec;
use aieo_core::{FindingStatus, CLI_VERSION};
use aieo_rules::readiness::rounded_ratio;
use aieo_rules::{gate_passes, Outcome};

/// Facts about the run that the rules do not know.
#[derive(Debug, Clone)]
pub struct RunMeta {
    pub origin: String,
    pub preset: String,
    pub project_id: Option<String>,
    pub kinds: Vec<String>,
    pub commit: Option<String>,
    pub branch: Option<String>,
    pub ci: Option<String>,
    pub duration_ms: u64,
    pub generated_at: String,
    pub fail_on: Vec<GateCondition>,
    /// baseline the run was compared with (generated_at, commit)
    pub baseline: Option<(String, Option<String>)>,
}

fn counts<'a>(findings: impl Iterator<Item = &'a aieo_core::Finding>) -> TierCounts {
    let mut c = TierCounts::default();
    for f in findings.filter(|f| f.suppressed.is_none()) {
        c.add(f.tier);
    }
    c
}

pub fn build_bundle(outcome: &Outcome, meta: &RunMeta) -> Bundle {
    let readiness = Readiness {
        rules_version: spec().rules_version.clone(),
        modules: outcome
            .readiness
            .modules
            .iter()
            .map(|m| ReadinessModule {
                id: m.id.to_string(),
                status: m.status.as_str().to_string(),
                ratio: rounded_ratio(m.ratio),
                reason: m.reason.map(str::to_string),
            })
            .collect(),
        scores: outcome.readiness.scores,
    };
    let new = meta.baseline.as_ref().map(|_| {
        counts(
            outcome
                .findings
                .iter()
                .filter(|f| f.status == FindingStatus::New),
        )
    });
    let diff = meta.baseline.as_ref().map(|(at, commit)| DiffInfo {
        baseline_generated_at: at.clone(),
        baseline_commit: commit.clone(),
        new: outcome
            .findings
            .iter()
            .filter(|f| f.status == FindingStatus::New)
            .count() as u64,
        existing: outcome
            .findings
            .iter()
            .filter(|f| f.status == FindingStatus::Existing)
            .count() as u64,
        fixed: outcome.fixed.clone(),
    });
    Bundle {
        schema: BUNDLE_SCHEMA.into(),
        producer: Producer {
            name: "aieo-cli".into(),
            version: CLI_VERSION.into(),
            rules_version: spec().rules_version.clone(),
            crawlers_version: Some(spec().crawlers_version.clone()),
        },
        generated_at: meta.generated_at.clone(),
        site: SiteInfo {
            origin: meta.origin.clone(),
            preset: meta.preset.clone(),
            project_id: meta.project_id.clone(),
        },
        run: RunInfo {
            kinds: meta.kinds.clone(),
            mode: "dist".into(),
            commit: meta.commit.clone(),
            branch: meta.branch.clone(),
            ci: meta.ci.clone(),
            duration_ms: meta.duration_ms,
        },
        readiness: Some(readiness),
        inventory: Inventory {
            pages: outcome.pages.clone(),
            sitemaps: outcome.sitemaps.clone(),
            robots_txt: outcome.robots_txt,
            llms_txt: outcome.llms_txt,
        },
        findings: outcome.findings.clone(),
        diff,
        summary: Summary {
            pages: outcome.pages.len() as u64,
            indexable: outcome.indexable() as u64,
            findings: counts(outcome.findings.iter()),
            new,
            gate: Gate {
                fail_on: meta.fail_on.iter().map(GateCondition::label).collect(),
                passed: gate_passes(&outcome.findings, &meta.fail_on),
            },
            incomplete: !outcome.incomplete_reasons.is_empty(),
            incomplete_reasons: outcome.incomplete_reasons.clone(),
        },
        logs: None,
        search: None,
        privacy: Privacy::default(),
    }
}

/// Where findings live in the repository, for formats that link to files.
#[derive(Debug, Clone)]
pub struct Paths {
    /// the build directory relative to the repository / project root ("dist")
    pub dist: String,
}

impl Paths {
    /// Repository path and line of a finding: its source file when known,
    /// else the build file.
    pub fn locate(&self, f: &aieo_core::Finding) -> Option<(String, Option<u32>)> {
        if let Some(src) = &f.source {
            return Some((src.clone(), None));
        }
        let file = f.file.as_ref()?;
        let dist = self.dist.trim_end_matches('/');
        let path = if dist.is_empty() || dist == "." {
            file.clone()
        } else {
            format!("{dist}/{file}")
        };
        Some((path, f.line))
    }
}
