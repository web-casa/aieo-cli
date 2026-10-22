//! The agent plugin in `plugins/aieo` ships with each release: its manifests
//! carry the release version and every skill is named after its directory.

use std::path::Path;

use serde_json::Value;

fn repo() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn json(rel: &str) -> Value {
    let text = std::fs::read_to_string(repo().join(rel)).unwrap();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn plugin_manifests_carry_the_release_version() {
    for rel in [
        "plugins/aieo/.claude-plugin/plugin.json",
        "plugins/aieo/.codex-plugin/plugin.json",
    ] {
        assert_eq!(
            json(rel)["version"],
            env!("CARGO_PKG_VERSION"),
            "{rel} must match Cargo.toml"
        );
    }
    let mcp = json("plugins/aieo/.mcp.json");
    assert_eq!(mcp["mcpServers"]["aieo"]["command"], "aieo");
    for rel in [
        ".claude-plugin/marketplace.json",
        ".agents/plugins/marketplace.json",
    ] {
        assert_eq!(json(rel)["plugins"][0]["name"], "aieo", "{rel}");
    }
}

#[test]
fn every_skill_is_named_after_its_directory() {
    let dir = repo().join("plugins/aieo/skills");
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        let text = std::fs::read_to_string(path.join("SKILL.md"))
            .unwrap_or_else(|_| panic!("{name}/SKILL.md missing"));
        let front = text
            .strip_prefix("---\n")
            .and_then(|t| t.split_once("\n---\n"))
            .map(|(f, _)| f)
            .unwrap_or_else(|| panic!("{name}: no frontmatter"));
        assert!(
            front.lines().any(|l| l == format!("name: {name}")),
            "{name}: frontmatter name differs from the directory"
        );
        let description = front
            .lines()
            .find_map(|l| l.strip_prefix("description: "))
            .unwrap_or_else(|| panic!("{name}: no description"));
        assert!(
            description.len() <= 1024,
            "{name}: description over 1024 characters"
        );
        n += 1;
    }
    assert!(n >= 5);
}
