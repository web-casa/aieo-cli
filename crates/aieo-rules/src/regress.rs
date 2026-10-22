//! D rules: what this build lost against a baseline (an earlier bundle).
//! Pages are matched by URL path, so a preview build compares cleanly with
//! a production baseline.

use std::collections::HashMap;

use url::Url;

use aieo_core::bundle::{Bundle, PageRecord};
use aieo_core::{Evidence, Localized};
use aieo_site::Site;

use crate::checks::Checker;
use crate::emit::{Emitter, Loc};
use crate::pages::PageFacts;

fn ev(en: &str, zh: &str, detail: impl Into<String>) -> Evidence {
    Evidence::new(Localized::new(en, zh), detail)
}

fn path_of(url: &str) -> String {
    Url::parse(url)
        .map(|u| u.path().to_string())
        .unwrap_or_else(|_| url.to_string())
}

fn canonical_path(c: Option<&str>) -> Option<String> {
    c.map(|c| {
        Url::parse(c)
            .map(|u| format!("{}{}", u.host_str().unwrap_or_default(), u.path()))
            .unwrap_or_else(|_| c.to_string())
    })
}

pub fn run(
    site: &Site,
    checker: &Checker,
    current: &[PageRecord],
    pages: &[PageFacts],
    baseline: &Bundle,
    e: &mut Emitter,
) {
    let now: HashMap<String, &PageRecord> = current.iter().map(|p| (path_of(&p.url), p)).collect();
    let facts: HashMap<&str, &PageFacts> = pages.iter().map(|p| (p.path.as_str(), p)).collect();
    let loc_for = |path: &str| {
        let f = facts.get(path);
        Loc {
            url_path: Some(path.to_string()),
            url: Some(site.url_for(path)),
            file: f.map(|p| p.file.clone()),
            source: f.and_then(|p| p.source.clone()),
        }
    };

    for old in baseline.inventory.pages.iter().filter(|p| p.indexable) {
        let path = path_of(&old.url);
        let Some(new) = now.get(&path) else {
            // gone from the inventory: fine if a redirect (or another file) still answers
            let url = Url::parse(&site.url_for(&path)).expect("site URL");
            if site.follow(&url).file.is_none() {
                e.emit(
                    "D-PAGE-REMOVED",
                    None,
                    Loc {
                        url_path: Some(path.clone()),
                        url: Some(url.to_string()),
                        file: None,
                        source: old.source.clone(),
                    },
                    "",
                    vec![
                        ev("Baseline file", "基线文件", old.file.clone()),
                        ev("Now", "现在", "no file and no redirect"),
                    ],
                    None,
                );
            }
            continue;
        };
        if new.noindex && !old.noindex {
            let via = facts
                .get(path.as_str())
                .and_then(|p| p.noindex_via.clone())
                .unwrap_or_default();
            e.emit(
                "D-PAGE-NOINDEXED",
                None,
                loc_for(&path),
                "",
                vec![ev("noindex", "noindex", via)],
                Some("noindex".into()),
            );
        }
        if canonical_path(old.canonical.as_deref()) != canonical_path(new.canonical.as_deref()) {
            e.emit(
                "D-CANONICAL-CHANGED",
                None,
                loc_for(&path),
                new.canonical.as_deref().unwrap_or(""),
                vec![
                    ev(
                        "Baseline",
                        "基线",
                        old.canonical.clone().unwrap_or_else(|| "(none)".into()),
                    ),
                    ev(
                        "Now",
                        "现在",
                        new.canonical.clone().unwrap_or_else(|| "(none)".into()),
                    ),
                ],
                Some("rel=\"canonical\"".into()),
            );
        }
        let lost_langs: Vec<&String> = old
            .hreflang
            .keys()
            .filter(|k| !new.hreflang.contains_key(*k))
            .collect();
        if !lost_langs.is_empty() {
            e.emit(
                "D-HREFLANG-LOST",
                None,
                loc_for(&path),
                &lost_langs
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                vec![ev(
                    "Lost",
                    "丢失",
                    lost_langs
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                )],
                None,
            );
        }
        let lost_types: Vec<&String> = old
            .jsonld_types
            .iter()
            .filter(|t| !new.jsonld_types.contains(t))
            .collect();
        if !lost_types.is_empty() {
            e.emit(
                "D-JSONLD-TYPE-LOST",
                None,
                loc_for(&path),
                &lost_types
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                vec![ev(
                    "Lost @type",
                    "丢失的 @type",
                    lost_types
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                )],
                Some("application/ld+json".into()),
            );
        }
        if old.words >= 100 && new.words * 2 <= old.words {
            e.emit(
                "D-CONTENT-SHRUNK",
                None,
                loc_for(&path),
                "",
                vec![ev(
                    "Visible words",
                    "可见文字",
                    format!("{} → {}", old.words, new.words),
                )],
                None,
            );
        }
        if old.in_sitemap && !new.in_sitemap && checker.indexable(facts[path.as_str()]) {
            e.emit(
                "D-SITEMAP-URL-DROPPED",
                None,
                loc_for(&path),
                "",
                vec![],
                None,
            );
        }
    }

    let before = baseline
        .inventory
        .pages
        .iter()
        .filter(|p| p.indexable)
        .count();
    let after = current.iter().filter(|p| p.indexable).count();
    if before >= 10 && after * 10 <= before * 9 {
        e.emit(
            "D-INDEXABLE-DROP",
            None,
            Loc {
                url_path: None,
                url: Some(site.origin_str()),
                file: None,
                source: None,
            },
            "",
            vec![ev(
                "Indexable pages",
                "可索引页面",
                format!("{before} → {after}"),
            )],
            None,
        );
    }
}
