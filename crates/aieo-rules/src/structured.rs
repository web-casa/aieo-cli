//! JSON-LD beyond "does it parse": the schema.org `@context`, the
//! properties Google requires for a rich result, rich result types Google
//! has retired, and the organization identity P04 compares across pages.
//!
//! Required properties follow Google Search Central's structured-data
//! pages (checked 2026-10): only top-level items and `@graph` members are
//! judged, as a nested item (a Review inside a Product) follows its
//! parent's rules. Types without required properties (Article,
//! Organization) are not listed.

use serde_json::Value;

const DOCS: &str = "https://developers.google.com/search/docs/appearance/structured-data/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub rule: &'static str,
    /// stable within the page: rule-specific key for the fingerprint
    pub key: String,
    pub ty: String,
    pub detail: String,
    /// documentation for this type
    pub doc: String,
}

/// A value is present when it is not null, an empty string or an empty array.
fn present(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(Value::Array(a)) => a.iter().any(|x| present(Some(x))),
        Some(_) => true,
    }
}

fn items(v: Option<&Value>) -> Vec<&Value> {
    match v {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(v @ Value::Object(_)) => vec![v],
        _ => Vec::new(),
    }
}

fn types_of(node: &Value) -> Vec<&str> {
    match node.get("@type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// `a.b` on a node whose `a` may be one object or an array of them.
fn present_path(node: &Value, path: &str) -> bool {
    match path.split_once('.') {
        None => present(node.get(path)),
        Some((head, rest)) => items(node.get(head)).iter().any(|n| present_path(n, rest)),
    }
}

/// Missing required properties of `node` judged as `ty`, or None when
/// Google lists no requirements for the type.
fn missing(ty: &str, node: &Value) -> Option<(Vec<String>, &'static str)> {
    let need = |props: &[&str]| -> Vec<String> {
        props
            .iter()
            .filter(|p| !present_path(node, p))
            .map(|p| p.to_string())
            .collect()
    };
    let one_of = |props: &[&str]| -> Option<String> {
        (!props.iter().any(|p| present_path(node, p))).then(|| props.join(" | "))
    };
    let (mut out, doc) = match ty {
        "Product" => {
            let mut m = need(&["name"]);
            m.extend(one_of(&["review", "aggregateRating", "offers"]));
            for offer in items(node.get("offers")) {
                let t = types_of(offer);
                if t.contains(&"AggregateOffer") {
                    for p in ["lowPrice", "priceCurrency"] {
                        if !present(offer.get(p)) {
                            m.push(format!("offers.{p}"));
                        }
                    }
                } else if !present(offer.get("price"))
                    && !present_path(offer, "priceSpecification.price")
                {
                    m.push("offers.price".into());
                }
            }
            (m, "product-snippet")
        }
        "BreadcrumbList" => {
            let list = items(node.get("itemListElement"));
            let mut m = Vec::new();
            if list.is_empty() {
                m.push("itemListElement".into());
            }
            // every ListItem but the last links to its page
            for (i, li) in list.iter().enumerate().take(list.len().saturating_sub(1)) {
                if !present(li.get("item")) {
                    m.push(format!("itemListElement[{i}].item"));
                }
            }
            (m, "breadcrumb")
        }
        "Event" => (need(&["name", "startDate", "location"]), "event"),
        "Recipe" => (need(&["name", "image"]), "recipe"),
        "JobPosting" => {
            let mut m = need(&["datePosted", "description", "hiringOrganization", "title"]);
            if !present(node.get("jobLocation"))
                && !present(node.get("applicantLocationRequirements"))
            {
                m.push("jobLocation".into());
            }
            (m, "job-posting")
        }
        "LocalBusiness"
        | "Restaurant"
        | "Store"
        | "FoodEstablishment"
        | "CafeOrCoffeeShop"
        | "BarOrPub"
        | "Bakery"
        | "Dentist"
        | "AutoRepair"
        | "ProfessionalService"
        | "Hotel" => (need(&["name", "address"]), "local-business"),
        "SoftwareApplication" | "MobileApplication" | "WebApplication" | "VideoGame" => {
            let mut m = need(&["name", "offers.price"]);
            m.extend(one_of(&["aggregateRating", "review"]));
            (m, "software-app")
        }
        "VideoObject" => (need(&["name", "thumbnailUrl", "uploadDate"]), "video"),
        "Review" => (
            need(&["author", "itemReviewed", "reviewRating.ratingValue"]),
            "review-snippet",
        ),
        "AggregateRating" => {
            let mut m = need(&["itemReviewed", "ratingValue"]);
            m.extend(one_of(&["ratingCount", "reviewCount"]));
            (m, "review-snippet")
        }
        "Dataset" => (need(&["name", "description"]), "dataset"),
        "Course" => (need(&["name", "description"]), "course"),
        "QAPage" => (need(&["mainEntity"]), "qapage"),
        "DiscussionForumPosting" => {
            let mut m = need(&["author", "datePublished"]);
            m.extend(one_of(&["text", "image", "video", "url"]));
            (m, "discussion-forum")
        }
        "ProfilePage" => (need(&["mainEntity"]), "profile-page"),
        _ => return None,
    };
    out.dedup();
    Some((out, doc))
}

/// Types Google no longer shows rich results for, and since when.
fn retired(ty: &str) -> Option<(&'static str, &'static str)> {
    match ty {
        "HowTo" => Some((
            "deprecated September 2023",
            "https://developers.google.com/search/blog/2023/08/howto-faq-changes",
        )),
        "FAQPage" => Some((
            "no longer shown since May 2026 (government and health sites only from 2023)",
            "https://developers.google.com/search/updates",
        )),
        "SpecialAnnouncement" => Some((
            "deprecated July 31, 2025",
            "https://developers.google.com/search/blog/2025/06/simplifying-search-results",
        )),
        "ClaimReview" => Some((
            "being phased out since June 2025",
            "https://developers.google.com/search/blog/2025/06/simplifying-search-results",
        )),
        _ => None,
    }
}

fn names_schema_org(ctx: &Value) -> bool {
    match ctx {
        Value::String(s) => s.contains("schema.org"),
        Value::Array(a) => a.iter().any(names_schema_org),
        Value::Object(o) => o.values().any(names_schema_org),
        _ => false,
    }
}

/// Issues of a page's JSON-LD blocks (those that parsed).
pub fn check(blocks: &[Value]) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut n = 0usize;
    for block in blocks {
        for top in items(Some(block)) {
            let has_context = top.get("@context").is_some_and(names_schema_org);
            if !has_context {
                let ty = types_of(top)
                    .first()
                    .map_or("(no @type)", |t| t)
                    .to_string();
                out.push(Issue {
                    rule: "T-JSONLD-NO-CONTEXT",
                    key: format!("{ty}#{n}"),
                    detail: match top.get("@context") {
                        None => "no @context".into(),
                        Some(c) => format!("@context is {c}"),
                    },
                    ty,
                    doc: format!("{DOCS}intro-structured-data"),
                });
            }
            let mut nodes = vec![top];
            nodes.extend(items(top.get("@graph")));
            for node in nodes {
                n += 1;
                for ty in types_of(node) {
                    if let Some((m, doc)) = missing(ty, node) {
                        if !m.is_empty() {
                            out.push(Issue {
                                rule: "T-JSONLD-MISSING-REQUIRED",
                                key: format!("{ty}#{n}"),
                                ty: ty.to_string(),
                                detail: m.join(", "),
                                doc: format!("{DOCS}{doc}"),
                            });
                        }
                    }
                    if let Some((when, doc)) = retired(ty) {
                        out.push(Issue {
                            rule: "T-JSONLD-RETIRED-TYPE",
                            key: ty.to_string(),
                            ty: ty.to_string(),
                            detail: when.to_string(),
                            doc: doc.to_string(),
                        });
                    }
                }
            }
        }
    }
    out.dedup_by(|a, b| a.rule == b.rule && a.key == b.key);
    out
}

/// Organization identity and types across a page's JSON-LD, as GEOReady's
/// P-rule sampler collects them (`collectGraph`): depth ≤ 4, arrays cut at
/// 20, descending into `@graph` and common value containers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entities {
    pub types: Vec<String>,
    pub names: Vec<String>,
    pub logos: Vec<String>,
    pub same_as: Vec<String>,
}

pub fn entities(blocks: &[Value]) -> Entities {
    fn push(v: &mut Vec<String>, s: &str) {
        let s = s.trim();
        if !s.is_empty() && !v.iter().any(|x| x == s) {
            v.push(s.to_string());
        }
    }
    fn visit(node: &Value, e: &mut Entities, depth: usize) {
        if depth > 4 {
            return;
        }
        match node {
            Value::Array(a) => a.iter().take(20).for_each(|i| visit(i, e, depth + 1)),
            Value::Object(o) => {
                let types = types_of(node);
                for t in &types {
                    push(&mut e.types, t);
                }
                let identity = types.iter().any(|t| {
                    matches!(
                        t.to_lowercase().as_str(),
                        "organization" | "corporation" | "brand"
                    )
                });
                if identity {
                    if let Some(name) = o.get("name").and_then(Value::as_str) {
                        push(&mut e.names, name);
                    }
                }
                match o.get("logo") {
                    Some(Value::String(l)) => push(&mut e.logos, l),
                    Some(l @ Value::Object(_)) => {
                        if let Some(u) = l.get("url").and_then(Value::as_str) {
                            push(&mut e.logos, u);
                        }
                    }
                    _ => {}
                }
                match o.get("sameAs") {
                    Some(Value::Array(a)) => a
                        .iter()
                        .filter_map(Value::as_str)
                        .for_each(|s| push(&mut e.same_as, s)),
                    Some(Value::String(s)) => push(&mut e.same_as, s),
                    _ => {}
                }
                if let Some(g) = o.get("@graph") {
                    visit(g, e, depth + 1);
                }
                for key in [
                    "offers",
                    "offer",
                    "brand",
                    "publisher",
                    "author",
                    "manufacturer",
                    "mainEntity",
                    "itemListElement",
                ] {
                    if let Some(v) = o.get(key) {
                        visit(v, e, depth + 1);
                    }
                }
            }
            _ => {}
        }
    }
    let mut e = Entities::default();
    for b in blocks {
        visit(b, &mut e, 0);
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rules(v: Value) -> Vec<(&'static str, String)> {
        check(&[v])
            .into_iter()
            .map(|i| (i.rule, i.detail))
            .collect()
    }

    #[test]
    fn context_is_required_per_top_level_item() {
        assert!(rules(
            json!({"@context": "https://schema.org", "@type": "Organization", "name": "A"})
        )
        .is_empty());
        assert!(
            rules(json!({"@context": {"@vocab": "http://schema.org/"}, "@type": "Thing"}))
                .is_empty()
        );
        let r = rules(
            json!([{"@context": "https://schema.org", "@type": "WebSite"}, {"@type": "Organization"}]),
        );
        assert_eq!(r, vec![("T-JSONLD-NO-CONTEXT", "no @context".to_string())]);
    }

    #[test]
    fn product_and_breadcrumb_requirements() {
        let ctx = "https://schema.org";
        assert_eq!(
            rules(json!({"@context": ctx, "@type": "Product", "name": "W"})),
            vec![(
                "T-JSONLD-MISSING-REQUIRED",
                "review | aggregateRating | offers".to_string()
            )]
        );
        assert!(rules(json!({"@context": ctx, "@type": "Product", "name": "W", "offers": {"@type": "Offer", "price": 0}})).is_empty());
        assert_eq!(
            rules(
                json!({"@context": ctx, "@type": "Product", "name": "W", "offers": [{"@type": "AggregateOffer", "lowPrice": 1}]})
            ),
            vec![(
                "T-JSONLD-MISSING-REQUIRED",
                "offers.priceCurrency".to_string()
            )]
        );
        let crumbs = json!({"@context": ctx, "@type": "BreadcrumbList", "itemListElement": [
            {"@type": "ListItem", "position": 1, "name": "Docs"},
            {"@type": "ListItem", "position": 2, "name": "Here"}]});
        assert_eq!(
            rules(crumbs),
            vec![(
                "T-JSONLD-MISSING-REQUIRED",
                "itemListElement[0].item".to_string()
            )]
        );
        assert!(
            rules(json!({"@context": ctx, "@type": "Article"})).is_empty(),
            "Article has no required properties"
        );
    }

    #[test]
    fn graph_members_and_retired_types() {
        let v = json!({"@context": "https://schema.org", "@graph": [
            {"@type": "FAQPage", "mainEntity": []},
            {"@type": "Event", "name": "E", "startDate": "2026-01-01"},
            {"@type": "SoftwareApplication", "name": "App", "offers": {"price": "0"}, "aggregateRating": {"ratingValue": 4}}]});
        assert_eq!(
            rules(v),
            vec![
                (
                    "T-JSONLD-RETIRED-TYPE",
                    "no longer shown since May 2026 (government and health sites only from 2023)"
                        .to_string()
                ),
                ("T-JSONLD-MISSING-REQUIRED", "location".to_string()),
            ]
        );
    }

    #[test]
    fn collects_entities_like_georeday() {
        let e = entities(&[json!({"@graph": [
            {"@type": "Organization", "name": " Example Inc ", "logo": {"url": "https://e.com/l.png"}, "sameAs": ["https://x.com/e"]},
            {"@type": "WebPage", "name": "Not an org", "publisher": {"@type": "Brand", "name": "Ex"}}]})]);
        assert_eq!(e.names, vec!["Example Inc", "Ex"]);
        assert_eq!(e.logos, vec!["https://e.com/l.png"]);
        assert_eq!(e.same_as, vec!["https://x.com/e"]);
        assert_eq!(e.types, vec!["Organization", "WebPage", "Brand"]);
    }
}
