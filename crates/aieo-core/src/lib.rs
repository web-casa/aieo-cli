//! Shared model for aieo-cli: the embedded rule specification (`@aieo/rules`),
//! findings and tiers, configuration, and the `aieo-bundle/v1` document.

pub mod bundle;
pub mod config;
pub mod finding;
pub mod spec;
pub mod text;

pub use finding::{
    fingerprint, Evidence, Finding, FindingStatus, Lang, Localized, Scope, Suppressed, Tier,
};
pub use spec::{spec, CrawlerSpec, RuleSpec, Spec};

/// Version of this CLI, recorded in every bundle.
pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");
