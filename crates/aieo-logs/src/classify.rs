//! Who sent a request: a crawler of the shared catalog (by user-agent
//! token), another automated client, or a person — and, for people, which
//! AI answer platform referred them.

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

use aieo_core::spec::{spec, AiPlatform, CrawlerSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Actor {
    Training,
    SearchIndex,
    UserFetch,
    ClassicSearch,
    Agent,
    Other,
    /// automated, not in the catalog (or no user agent at all)
    UnknownBot,
    Human,
}

impl Actor {
    pub fn as_str(self) -> &'static str {
        match self {
            Actor::Training => "training",
            Actor::SearchIndex => "search_index",
            Actor::UserFetch => "user_fetch",
            Actor::ClassicSearch => "classic_search",
            Actor::Agent => "agent",
            Actor::Other => "other",
            Actor::UnknownBot => "unknown_bot",
            Actor::Human => "human",
        }
    }

    pub fn parse(s: &str) -> Actor {
        match s {
            "training" => Actor::Training,
            "search_index" => Actor::SearchIndex,
            "user_fetch" => Actor::UserFetch,
            "classic_search" => Actor::ClassicSearch,
            "agent" => Actor::Agent,
            "other" => Actor::Other,
            "human" => Actor::Human,
            _ => Actor::UnknownBot,
        }
    }

    fn of_purpose(purpose: &str) -> Actor {
        match purpose {
            "training" => Actor::Training,
            "search_index" => Actor::SearchIndex,
            "user_fetch" => Actor::UserFetch,
            "classic_search" => Actor::ClassicSearch,
            "agent" => Actor::Agent,
            _ => Actor::Other,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Class {
    pub actor: Actor,
    pub crawler: Option<&'static CrawlerSpec>,
}

/// Generic automation markers in user agents.
static AUTOMATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)bot\b|bot/|crawl|spider|slurp|scrap|fetch|monitor|python-|python/|curl/|wget/|go-http-client|java/|okhttp|libwww|httpclient|axios|node-fetch|headless|lighthouse|phantomjs|feed")
        .unwrap()
});

/// Catalog crawlers that appear in user agents, longest token first so a
/// specific token wins over one it contains.
fn matchers() -> &'static [(String, &'static CrawlerSpec)] {
    static M: LazyLock<Vec<(String, &'static CrawlerSpec)>> = LazyLock::new(|| {
        let mut m: Vec<(String, &'static CrawlerSpec)> = spec()
            .crawlers
            .iter()
            .filter(|c| !c.robots_only)
            .filter_map(|c| Some((c.ua_pattern.as_ref()?.to_ascii_lowercase(), c)))
            .collect();
        m.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
        m
    });
    &M
}

pub fn classify(ua: &str) -> Class {
    let lower = ua.to_ascii_lowercase();
    if let Some((_, c)) = matchers().iter().find(|(p, _)| lower.contains(p.as_str())) {
        return Class {
            actor: Actor::of_purpose(&c.purpose),
            crawler: Some(c),
        };
    }
    let actor = if ua.trim().is_empty() || AUTOMATED.is_match(ua) {
        Actor::UnknownBot
    } else {
        Actor::Human
    };
    Class {
        actor,
        crawler: None,
    }
}

/// The AI platform behind a visit: its referrer host, or a `utm_source`.
pub fn ai_platform(referrer: Option<&str>, query: Option<&str>) -> Option<&'static AiPlatform> {
    let platforms = &spec().ai_platforms;
    if let Some(host) = referrer
        .and_then(|r| Url::parse(r).ok())
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
    {
        let hit = platforms.iter().find(|p| {
            p.referrer_hosts
                .iter()
                .any(|h| host == *h || host.ends_with(&format!(".{h}")))
        });
        if hit.is_some() {
            return hit;
        }
    }
    let utm = query?
        .split('&')
        .find_map(|kv| kv.strip_prefix("utm_source="))?
        .to_ascii_lowercase();
    let utm = percent_encoding::percent_decode_str(&utm).decode_utf8_lossy();
    platforms
        .iter()
        .find(|p| p.utm_sources.iter().any(|s| *s == utm))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_user_agents() {
        let c = classify("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; OAI-SearchBot/1.3; +https://openai.com/searchbot)");
        assert_eq!(
            (c.actor, c.crawler.map(|c| c.id.as_str())),
            (Actor::SearchIndex, Some("oai-searchbot"))
        );
        let c =
            classify("Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)");
        assert_eq!(c.crawler.map(|c| c.id.as_str()), Some("googlebot"));
        assert_eq!(
            classify("Mozilla/5.0 (compatible; ClaudeBot/1.0; +claudebot@anthropic.com)").actor,
            Actor::Training
        );
        assert_eq!(classify("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; ChatGPT-User/1.0; +https://openai.com/bot").actor, Actor::UserFetch);
        assert_eq!(
            classify("Mozilla/5.0 (X11; Linux x86_64) Chrome/120.0 Safari/537.36").actor,
            Actor::Human
        );
        assert_eq!(classify("python-requests/2.31").actor, Actor::UnknownBot);
        assert_eq!(classify("").actor, Actor::UnknownBot);
        assert_eq!(
            classify("Mozilla/5.0 (compatible; AhrefsBot/7.0)").actor,
            Actor::UnknownBot
        );
    }

    #[test]
    fn finds_ai_referrals() {
        assert_eq!(
            ai_platform(Some("https://chatgpt.com/"), None).map(|p| p.id.as_str()),
            Some("chatgpt")
        );
        assert_eq!(
            ai_platform(Some("https://www.perplexity.ai/search?q=x"), None).map(|p| p.id.as_str()),
            Some("perplexity")
        );
        assert_eq!(
            ai_platform(None, Some("a=1&utm_source=chatgpt.com")).map(|p| p.id.as_str()),
            Some("chatgpt")
        );
        assert!(ai_platform(
            Some("https://www.google.com/"),
            Some("utm_source=newsletter")
        )
        .is_none());
    }
}
