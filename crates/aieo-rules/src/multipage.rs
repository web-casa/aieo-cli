//! P01–P04, the multi-page modules of GEOReady (`rules/paid/checks.ts`),
//! run over every indexable content page of the build instead of a fetched
//! sample. Same budgets, grading and weights (a 0..100 layer of its own,
//! apart from the F score). Two deliberate differences: P01 does not count
//! a page's `lang` against the majority when the page declares hreflang
//! alternates, since a translated page is meant to differ; and a canonical
//! matches when the host answers it with the page's own file (`/about` for
//! `about/index.html`), as there is no fetched final URL to compare.

use aieo_core::spec::spec;
use aieo_core::{Evidence, Localized};

use crate::pages::{PageFacts, PageKind};
use crate::readiness::{ModuleResult, SiteType, Status};

#[derive(Debug, Clone)]
pub struct MultipageReport {
    pub modules: Vec<ModuleResult>,
    /// weighted over modules that ran, 0..100 with one decimal
    pub score: f64,
    pub pages: usize,
}

fn l(en: &str, zh: &str) -> Localized {
    Localized::new(en, zh)
}

fn ev(en: &str, zh: &str, detail: impl Into<String>) -> Evidence {
    Evidence::new(l(en, zh), detail.into())
}

/// GEOReady's `graded`: 2-decimal ratio; ≥0.99 pass, ≤0.01 fail, partial from 0.5.
fn graded(ratio: f64) -> (Status, f64) {
    let r = ((ratio * 100.0).round() / 100.0).clamp(0.0, 1.0);
    if r >= 0.99 {
        (Status::Pass, 1.0)
    } else if r <= 0.01 {
        (Status::Fail, 0.0)
    } else if r >= 0.5 {
        (Status::Partial, r)
    } else {
        (Status::Fail, r)
    }
}

fn result(
    id: &'static str,
    (status, ratio): (Status, f64),
    evidence: Vec<Evidence>,
    fix: Localized,
) -> ModuleResult {
    ModuleResult {
        id,
        status,
        ratio,
        reason: None,
        evidence,
        fix: (status != Status::Pass).then_some(fix),
    }
}

fn not_applicable(id: &'static str, evidence: Vec<Evidence>) -> ModuleResult {
    ModuleResult {
        id,
        status: Status::NotApplicable,
        ratio: 0.0,
        reason: None,
        evidence,
        fix: None,
    }
}

fn count(values: impl Iterator<Item = String>) -> std::collections::HashMap<String, usize> {
    let mut m = std::collections::HashMap::new();
    for v in values {
        *m.entry(v).or_insert(0) += 1;
    }
    m
}

fn dupes(m: &std::collections::HashMap<String, usize>) -> usize {
    m.values().filter(|n| **n > 1).map(|n| n - 1).sum()
}

fn p01(pages: &[&PageFacts], canonical_ok: &dyn Fn(&PageFacts) -> bool) -> ModuleResult {
    if pages.len() < 2 {
        return not_applicable(
            "P01",
            vec![ev(
                "Pages",
                "页面",
                format!(
                    "{} usable page(s) — consistency needs at least 2",
                    pages.len()
                ),
            )],
        );
    }
    let titles = count(
        pages
            .iter()
            .filter(|p| !p.title.is_empty())
            .map(|p| p.title.to_lowercase()),
    );
    let descs = count(
        pages
            .iter()
            .filter_map(|p| p.description.as_ref())
            .map(|d| d.to_lowercase()),
    );
    let monolingual: Vec<&&PageFacts> = pages.iter().filter(|p| p.hreflang.is_empty()).collect();
    let langs = count(
        monolingual
            .iter()
            .filter_map(|p| p.lang.as_ref())
            .map(|l| l.to_lowercase()),
    );
    // the canonical names this page (any URL the host answers with its file)
    let canonical_mismatch = pages.iter().filter(|p| !canonical_ok(p)).count();
    let (title_dupes, desc_dupes) = (dupes(&titles), dupes(&descs));
    let lang_mismatch = if langs.len() > 1 {
        monolingual.len() - langs.values().max().copied().unwrap_or(0)
    } else {
        0
    };
    let missing_title = pages.iter().filter(|p| p.title.is_empty()).count();
    let missing_desc = pages.iter().filter(|p| p.description.is_none()).count();
    let mut evidence = vec![
        ev("Pages checked", "检查页面数", pages.len().to_string()),
        ev(
            "Duplicate titles / descriptions",
            "重复标题 / 描述",
            format!("{title_dupes} / {desc_dupes}"),
        ),
        ev(
            "Missing title / description",
            "缺失标题 / 描述",
            format!("{missing_title} / {missing_desc}"),
        ),
        ev(
            "Canonical mismatches",
            "Canonical 不一致",
            canonical_mismatch.to_string(),
        ),
    ];
    if lang_mismatch > 0 {
        evidence.push(ev(
            "Language inconsistencies",
            "语言声明不一致",
            format!("{lang_mismatch} page(s) differ from the majority lang"),
        ));
    }
    let problems = title_dupes
        + desc_dupes
        + missing_title
        + missing_desc
        + canonical_mismatch
        + lang_mismatch;
    let budget = (pages.len() * 2).max(1);
    let ratio = 1.0 - (problems as f64 / budget as f64).min(1.0);
    result(
        "P01",
        graded(ratio),
        evidence,
        l(
            "Give every page unique, complete metadata",
            "为每个页面提供唯一且完整的元数据",
        ),
    )
}

fn p02(pages: &[&PageFacts]) -> ModuleResult {
    if pages.len() < 2 {
        return not_applicable(
            "P02",
            vec![ev(
                "Pages",
                "页面",
                format!("{} usable page(s)", pages.len()),
            )],
        );
    }
    let n = pages.len();
    let nav = pages.iter().filter(|p| p.has_nav).count();
    let thin = pages.iter().filter(|p| p.text_len < 400).count();
    let no_h1 = pages.iter().filter(|p| p.h1_count == 0).count();
    let multi_h1 = pages.iter().filter(|p| p.h1_count > 1).count();
    let dead_end = pages.iter().filter(|p| p.links.is_empty()).count();
    let mut evidence = vec![
        ev(
            "Pages with nav landmark",
            "含导航标记的页面",
            format!("{nav}/{n}"),
        ),
        ev(
            "Thin pages (<400 visible chars)",
            "内容过少页面（可见字符 <400）",
            format!("{thin}/{n}"),
        ),
        ev(
            "h1 discipline (missing / multiple)",
            "h1 规范（缺失 / 多个）",
            format!("{no_h1} / {multi_h1}"),
        ),
    ];
    if dead_end > 0 {
        evidence.push(ev(
            "Dead-end pages (no internal links)",
            "死胡同页面（无站内链接）",
            format!("{dead_end}/{n}"),
        ));
    }
    let problems = thin + no_h1 + multi_h1 + dead_end + (n - nav);
    let ratio = 1.0 - (problems as f64 / (n * 2) as f64).min(1.0);
    result(
        "P02",
        graded(ratio),
        evidence,
        l(
            "Make every page self-navigable and substantive",
            "让每个页面可导航且内容充实",
        ),
    )
}

fn p03(pages: &[&PageFacts], site_type: SiteType, openapi: bool, plugin: bool) -> ModuleResult {
    if !matches!(site_type, SiteType::Api | SiteType::Ecommerce) {
        return not_applicable(
            "P03",
            vec![ev(
                "Site type",
                "站点类型",
                format!(
                    "{} — API/ecommerce completeness not applicable",
                    site_type.as_str()
                ),
            )],
        );
    }
    let types: std::collections::BTreeSet<String> = pages
        .iter()
        .flat_map(|p| p.entities.types.iter().map(|t| t.to_lowercase()))
        .collect();
    if site_type == SiteType::Api {
        let docs_nav = pages.iter().any(|p| p.has_nav);
        let evidence = vec![
            ev(
                "OpenAPI document",
                "OpenAPI 文档",
                if openapi { "found" } else { "not found" },
            ),
            ev(
                "AI plugin manifest",
                "AI 插件清单",
                if plugin { "found" } else { "not found" },
            ),
            ev(
                "Structured types across pages",
                "页面中的结构化类型",
                if types.is_empty() {
                    "none".to_string()
                } else {
                    types.iter().take(6).cloned().collect::<Vec<_>>().join(", ")
                },
            ),
        ];
        let score = 0.6 * f64::from(u8::from(openapi))
            + 0.2 * f64::from(u8::from(plugin))
            + 0.2 * f64::from(u8::from(docs_nav));
        return result(
            "P03",
            graded(score),
            evidence,
            l(
                "Publish machine-readable API information",
                "发布机器可读的 API 信息",
            ),
        );
    }
    let product_pages = pages
        .iter()
        .filter(|p| {
            p.entities
                .types
                .iter()
                .any(|t| t.eq_ignore_ascii_case("product"))
        })
        .count();
    let has_product = types.contains("product");
    let has_offer = types.contains("offer") || types.contains("aggregateoffer");
    let has_org = types.contains("organization") || types.contains("brand");
    let evidence = vec![
        ev(
            "Product structured data",
            "商品结构化数据",
            if product_pages > 0 {
                format!("{product_pages} page(s) with Product JSON-LD")
            } else {
                "none".into()
            },
        ),
        ev(
            "Offer / price signals",
            "报价 / 价格信号",
            if has_offer { "found" } else { "not found" },
        ),
        ev(
            "Organization / brand",
            "组织 / 品牌",
            if has_org { "found" } else { "not found" },
        ),
    ];
    let score = 0.5 * f64::from(u8::from(has_product))
        + 0.25 * f64::from(u8::from(has_offer))
        + 0.25 * f64::from(u8::from(has_org));
    result(
        "P03",
        graded(score),
        evidence,
        l(
            "Expose product and brand structured data",
            "提供商品与品牌结构化数据",
        ),
    )
}

fn p04(pages: &[&PageFacts]) -> ModuleResult {
    if pages.len() < 2 {
        return not_applicable(
            "P04",
            vec![ev(
                "Pages",
                "页面",
                format!("{} usable page(s)", pages.len()),
            )],
        );
    }
    let with_entity = pages
        .iter()
        .filter(|p| !p.entities.names.is_empty() || !p.entities.logos.is_empty())
        .count();
    let mut names: Vec<String> = Vec::new();
    let mut logos = std::collections::BTreeSet::new();
    let mut same_as_sets = std::collections::BTreeSet::new();
    for p in pages {
        for n in &p.entities.names {
            let n = n.to_lowercase();
            if !names.contains(&n) {
                names.push(n);
            }
        }
        for lg in &p.entities.logos {
            logos.insert(lg.trim().to_lowercase());
        }
        if !p.entities.same_as.is_empty() {
            let mut set: Vec<String> = p
                .entities
                .same_as
                .iter()
                .map(|s| s.trim().to_lowercase())
                .collect();
            set.sort();
            same_as_sets.insert(set.join("|"));
        }
    }
    let mut evidence = vec![
        ev(
            "Pages with entity JSON-LD",
            "含实体 JSON-LD 的页面",
            format!("{with_entity}/{}", pages.len()),
        ),
        ev(
            "Distinct organization names",
            "不同组织名数量",
            if names.is_empty() {
                "none".to_string()
            } else {
                names.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
            },
        ),
    ];
    if logos.len() > 1 {
        evidence.push(ev(
            "Distinct logo URLs",
            "不同 logo URL 数量",
            logos.len().to_string(),
        ));
    }
    if same_as_sets.len() > 1 {
        evidence.push(ev(
            "Conflicting sameAs sets",
            "冲突的 sameAs 集合",
            same_as_sets.len().to_string(),
        ));
    }
    let coverage = with_entity as f64 / pages.len() as f64;
    let penalty = if names.len() > 1 { 0.3 } else { 0.0 }
        + if logos.len() > 1 { 0.2 } else { 0.0 }
        + if same_as_sets.len() > 1 { 0.2 } else { 0.0 };
    result(
        "P04",
        graded((coverage - penalty).max(0.0)),
        evidence,
        l(
            "Keep one consistent entity identity site-wide",
            "全站保持一致的实体身份",
        ),
    )
}

/// Pages the P modules read: indexable content pages that decoded.
pub fn eligible(pages: &[PageFacts]) -> Vec<&PageFacts> {
    pages
        .iter()
        .filter(|p| p.kind == PageKind::Content && !p.noindex && !p.undecodable)
        .collect()
}

pub fn run(
    pages: &[PageFacts],
    canonical_ok: &dyn Fn(&PageFacts) -> bool,
    site_type: SiteType,
    openapi: bool,
    plugin: bool,
) -> MultipageReport {
    let usable = eligible(pages);
    let modules = vec![
        p01(&usable, canonical_ok),
        p02(&usable),
        p03(&usable, site_type, openapi, plugin),
        p04(&usable),
    ];
    let (mut active, mut weighted) = (0.0, 0.0);
    for m in &modules {
        if m.status != Status::NotApplicable {
            let w = spec().expect_rule(m.id).weight;
            active += w;
            weighted += w * m.ratio.clamp(0.0, 1.0);
        }
    }
    let score = if active > 0.0 {
        (weighted / active * 1000.0).round() / 10.0
    } else {
        0.0
    };
    MultipageReport {
        modules,
        score,
        pages: usable.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grades_like_georeday() {
        assert_eq!(graded(0.995), (Status::Pass, 1.0));
        assert_eq!(graded(0.005), (Status::Fail, 0.0));
        assert_eq!(graded(0.75), (Status::Partial, 0.75));
        assert_eq!(graded(0.4), (Status::Fail, 0.4));
    }
}
