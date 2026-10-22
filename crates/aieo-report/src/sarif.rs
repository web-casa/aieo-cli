//! SARIF 2.1.0 for code-scanning tools (GitHub code scanning, GitLab, IDEs).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use aieo_core::bundle::Bundle;
use aieo_core::spec::{spec, EvidenceLevel};
use aieo_core::{FindingStatus, Tier, CLI_VERSION};

use crate::Paths;

fn level(t: Tier) -> &'static str {
    match t {
        Tier::Error => "error",
        Tier::Warning => "warning",
        Tier::Notice => "note",
    }
}

pub fn render(bundle: &Bundle, paths: &Paths) -> String {
    let used: BTreeSet<&str> = bundle.findings.iter().map(|f| f.rule.as_str()).collect();
    let rules: Vec<Value> = used
        .iter()
        .filter_map(|id| spec().rule(id))
        .map(|r| {
            let default_tier = r.tier.default.or(r.tier.fail).unwrap_or(Tier::Notice);
            let mut rule = json!({
                "id": r.id,
                "name": r.id.replace('-', ""),
                "shortDescription": { "text": r.title.en },
                "fullDescription": { "text": format!("{} — {}", r.description.en, r.description.zh) },
                "defaultConfiguration": { "level": level(default_tier) },
                "properties": {
                    "tags": ["seo", "geo", format!("family-{}", r.family)],
                    "precision": if r.evidence_level == EvidenceLevel::Official { "high" } else { "medium" },
                    "title_zh": r.title.zh,
                }
            });
            if let Some(url) = &r.source_url {
                rule["helpUri"] = json!(url);
            }
            rule
        })
        .collect();
    let results: Vec<Value> = bundle
        .findings
        .iter()
        .map(|f| {
            let detail = f.evidence.iter().map(|e| format!("{}: {}", e.label.en, e.detail)).collect::<Vec<_>>().join("; ");
            let url = f.url.clone().unwrap_or_default();
            let text = if detail.is_empty() { format!("{} ({url})", f.message.en) } else { format!("{} ({url}) — {detail}", f.message.en) };
            let mut result = json!({
                "ruleId": f.rule,
                "level": level(f.tier),
                "message": { "text": text },
                "partialFingerprints": { "aieo/v1": f.fingerprint },
                "properties": {
                    "url": f.url,
                    "evidence_level": f.evidence_level,
                    "status": match f.status { FindingStatus::New => "new", FindingStatus::Existing => "existing", FindingStatus::Unknown => "unknown" },
                    "build_file": f.file,
                }
            });
            if let Some((path, line)) = paths.locate(f) {
                let mut physical = json!({ "artifactLocation": { "uri": path, "uriBaseId": "%SRCROOT%" } });
                physical["region"] = json!({ "startLine": line.unwrap_or(1) });
                result["locations"] = json!([{ "physicalLocation": physical }]);
            }
            if let Some(s) = &f.suppressed {
                result["suppressions"] = json!([{
                    "kind": "external",
                    "justification": format!("{} (owner: {}, expires {})", s.reason, s.owner, s.expires),
                }]);
            }
            if f.status == FindingStatus::Existing {
                result["baselineState"] = json!("unchanged");
            } else if f.status == FindingStatus::New {
                result["baselineState"] = json!("new");
            }
            result
        })
        .collect();
    let sarif = json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": {
                "name": "aieo",
                "version": CLI_VERSION,
                "semanticVersion": CLI_VERSION,
                "informationUri": "https://aieo.ee",
                "rules": rules,
                "properties": { "rules_version": spec().rules_version },
            }},
            "automationDetails": { "id": format!("aieo/{}/", bundle.site.origin) },
            "results": results,
        }]
    });
    serde_json::to_string_pretty(&sarif).expect("sarif serializes")
}
