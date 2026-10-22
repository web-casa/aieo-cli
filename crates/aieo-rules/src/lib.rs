//! The rules aieo-cli runs over a build directory: readiness modules (F,
//! conformance-locked to aieo.ee), page checks (T), build contracts (C) and
//! regressions against a baseline (D).

pub mod checks;
pub mod emit;
pub mod llms;
pub mod pages;
pub mod readiness;
pub mod regress;

use std::collections::{BTreeMap, HashMap, HashSet};

use aieo_core::bundle::{Bundle, FixedFinding, PageRecord};
use aieo_core::config::{GateCondition, Settings};
use aieo_core::spec::spec;
use aieo_core::text::line_of;
use aieo_core::{Finding, FindingStatus, Tier};
use aieo_site::preset::Project;
use aieo_site::Site;

use crate::emit::{Emitter, Loc, Pending};
use crate::pages::PageKind;
use crate::readiness::{ReadinessReport, Status};

pub struct Options<'a> {
    pub settings: &'a Settings,
    pub project: Option<&'a Project>,
    pub baseline: Option<&'a Bundle>,
}

pub struct Outcome {
    pub readiness: ReadinessReport,
    pub pages: Vec<PageRecord>,
    pub sitemaps: Vec<String>,
    pub robots_txt: bool,
    pub llms_txt: bool,
    pub findings: Vec<Finding>,
    /// baseline findings that are gone (only with a baseline)
    pub fixed: Vec<FixedFinding>,
    pub incomplete_reasons: Vec<String>,
}

impl Outcome {
    pub fn indexable(&self) -> usize {
        self.pages.iter().filter(|p| p.indexable).count()
    }
}

/// F module results as findings, at the tier the spec gives a failed or
/// partial module.
fn readiness_findings(site: &Site, report: &ReadinessReport, e: &mut Emitter) {
    let home = format!("{}/", site.base_path);
    let home_file = site.resolve_exact(&home).map(str::to_string);
    // A meta-refresh homepage may never be served in production (a host
    // redirect can answer `/` first): what the build shows there is not
    // proof enough to fail a deploy, so its findings stop at warning.
    let home_stub = home_file
        .as_deref()
        .and_then(|f| site.redirect_stub(f))
        .map(str::to_string);
    for m in &report.modules {
        let rule = spec().expect_rule(m.id);
        let tier = match m.status {
            Status::Fail => rule.tier.fail,
            Status::Partial => rule.tier.partial,
            _ => None,
        };
        let Some(mut tier) = tier else { continue };
        let on_home = !matches!(m.id, "F02" | "F03" | "F05" | "F16" | "F18");
        if on_home && home_stub.is_some() {
            tier = tier.min(Tier::Warning);
        }
        let (file, url_path) = match m.id {
            "F02" | "F03" => ("robots.txt", format!("{}/robots.txt", site.base_path)),
            "F05" => ("sitemap.xml", format!("{}/sitemap.xml", site.base_path)),
            "F16" => ("llms.txt", format!("{}/llms.txt", site.base_path)),
            "F18" => ("openapi.json", format!("{}/openapi.json", site.base_path)),
            _ => ("index.html", home.clone()),
        };
        let mut evidence = m.evidence.clone();
        if let Some(fix) = &m.fix {
            evidence.push(aieo_core::Evidence::new(
                aieo_core::Localized::new("Fix", "修复"),
                fix.en.clone(),
            ));
        }
        if let (true, Some(target)) = (on_home, &home_stub) {
            evidence.push(aieo_core::Evidence::new(
                aieo_core::Localized::new("Homepage", "首页"),
                format!("meta-refresh redirect to {target} — check what production serves at /"),
            ));
        }
        e.emit(
            m.id,
            Some(tier),
            Loc {
                url_path: Some(url_path.clone()),
                url: Some(site.url_for(&url_path)),
                file: site.has_file(file).then(|| file.to_string()),
                source: None,
            },
            "",
            evidence,
            None,
        );
    }
}

fn locate_lines(site: &Site, pending: &mut [Pending]) {
    let mut by_file: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, p) in pending.iter().enumerate() {
        if let (Some(file), Some(_)) = (&p.finding.file, &p.needle) {
            by_file.entry(file.clone()).or_default().push(i);
        }
    }
    for (file, idxs) in by_file {
        let Some(bytes) = site.read_bytes(&file) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        for i in idxs {
            let needle = pending[i].needle.as_deref().unwrap_or_default();
            pending[i].finding.line = line_of(&text, needle);
        }
    }
}

pub fn check(site: &Site, opts: &Options) -> Outcome {
    let readiness = readiness::run(site);
    let facts = pages::collect(site, opts.project);
    let checker = checks::Checker::new(site, &facts);
    let mut e = Emitter::new(opts.settings);

    readiness_findings(site, &readiness, &mut e);
    let graph = checker.run(&mut e);

    let pages: Vec<PageRecord> = facts
        .iter()
        .filter(|p| p.kind == PageKind::Content)
        .map(|p| PageRecord {
            url: p.url.to_string(),
            file: p.file.clone(),
            source: p.source.clone(),
            indexable: checker.indexable(p),
            noindex: p.noindex,
            canonical: p.canonical.as_ref().map(|c| c.to_string()),
            hreflang: p
                .hreflang
                .iter()
                .filter_map(|h| {
                    h.url
                        .as_ref()
                        .map(|u| (h.code.to_lowercase(), u.to_string()))
                })
                .collect(),
            title: (!p.title.is_empty()).then(|| p.title.clone()),
            words: p.words,
            jsonld_types: p.jsonld_types.clone(),
            inlinks: graph.inlinks.get(&p.file).copied().unwrap_or(0),
            in_sitemap: graph.in_sitemap.contains(&p.file),
            hash: p.hash.clone(),
        })
        .collect();

    if let Some(baseline) = opts.baseline {
        regress::run(site, &checker, &pages, &facts, baseline, &mut e);
    }

    let mut pending = e.out;
    locate_lines(site, &mut pending);
    let mut seen = HashSet::new();
    let mut findings: Vec<Finding> = pending
        .into_iter()
        .map(|p| p.finding)
        .filter(|f| seen.insert(f.fingerprint.clone()))
        .collect();

    let mut fixed = Vec::new();
    if let Some(baseline) = opts.baseline {
        let before: HashSet<&str> = baseline
            .findings
            .iter()
            .map(|f| f.fingerprint.as_str())
            .collect();
        for f in &mut findings {
            f.status = if before.contains(f.fingerprint.as_str()) {
                FindingStatus::Existing
            } else {
                FindingStatus::New
            };
        }
        fixed = baseline
            .findings
            .iter()
            .filter(|f| !seen.contains(&f.fingerprint))
            .map(|f| FixedFinding {
                rule: f.rule.clone(),
                fingerprint: f.fingerprint.clone(),
                url: f.url.clone(),
            })
            .collect();
    }
    findings.sort_by(|a, b| {
        b.tier
            .cmp(&a.tier)
            .then_with(|| a.rule.cmp(&b.rule))
            .then_with(|| a.url.cmp(&b.url))
            .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });

    let mut incomplete_reasons = readiness.incomplete_reasons.clone();
    if facts.is_empty() {
        incomplete_reasons.push("the build has no HTML pages".into());
    }
    Outcome {
        readiness,
        pages,
        sitemaps: graph.sitemaps,
        robots_txt: graph.robots_txt,
        llms_txt: graph.llms_txt,
        findings,
        fixed,
        incomplete_reasons,
    }
}

/// Whether the findings pass the gate. Suppressed findings never fail it.
pub fn gate_passes(findings: &[Finding], conditions: &[GateCondition]) -> bool {
    !findings.iter().filter(|f| f.suppressed.is_none()).any(|f| {
        conditions
            .iter()
            .any(|c| f.tier >= c.tier && (!c.new_only || f.status == FindingStatus::New))
    })
}

/// Counts per tier (suppressed findings left out).
pub fn tier_counts<'a>(findings: impl Iterator<Item = &'a Finding>) -> HashMap<Tier, u64> {
    let mut m = HashMap::new();
    for f in findings.filter(|f| f.suppressed.is_none()) {
        *m.entry(f.tier).or_insert(0) += 1;
    }
    m
}
