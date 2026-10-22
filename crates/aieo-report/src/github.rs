//! GitHub Actions workflow commands (`::error file=…::…`): findings show up
//! as annotations on the run and on pull-request diffs.

use aieo_core::bundle::Bundle;
use aieo_core::{FindingStatus, Tier};

use crate::Paths;

fn escape_data(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn escape_property(s: &str) -> String {
    escape_data(s).replace(':', "%3A").replace(',', "%2C")
}

/// One annotation per unsuppressed finding; with a baseline only new ones
/// (existing debt stays in the summary, not on every diff).
pub fn render(bundle: &Bundle, paths: &Paths) -> String {
    let compared = bundle.diff.is_some();
    let mut out = String::new();
    for f in bundle.findings.iter().filter(|f| f.suppressed.is_none()) {
        if compared && f.status != FindingStatus::New {
            continue;
        }
        let command = match f.tier {
            Tier::Error => "error",
            Tier::Warning => "warning",
            Tier::Notice => "notice",
        };
        let mut props = vec![format!(
            "title={}",
            escape_property(&format!("{} {}", f.rule, f.message.en))
        )];
        if let Some((path, line)) = paths.locate(f) {
            props.insert(0, format!("file={}", escape_property(&path)));
            if let Some(line) = line {
                props.insert(1, format!("line={line}"));
            }
        }
        let detail = f
            .evidence
            .iter()
            .map(|e| format!("{}: {}", e.label.en, e.detail))
            .collect::<Vec<_>>()
            .join("\n");
        let message = format!("{}\n{}", f.url.as_deref().unwrap_or_default(), detail);
        out.push_str(&format!(
            "::{command} {}::{}\n",
            props.join(","),
            escape_data(message.trim_end())
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_workflow_commands() {
        assert_eq!(escape_property("a:b,c%"), "a%3Ab%2Cc%25");
        assert_eq!(escape_data("x\ny"), "x%0Ay");
    }
}
