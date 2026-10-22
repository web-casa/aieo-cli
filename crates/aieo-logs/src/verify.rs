//! Is a request that claims to be a crawler really from it?
//!
//! First the operator's published IP ranges (`{creationTime, prefixes:
//! [{ipv4Prefix|ipv6Prefix}]}`, cached under `.aieo/cache/ip-ranges` for a
//! day), then forward-confirmed reverse DNS for crawlers that document it
//! (Googlebot, Bingbot, Applebot, CCBot, YandexBot, …). A crawler without
//! either is `unverifiable` — never called a fake for lack of a list.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use ipnet::IpNet;
use serde_json::Value;

use aieo_core::spec::CrawlerSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verdict {
    Verified,
    /// claims the crawler, comes from elsewhere
    Mismatch,
    /// the operator publishes no way to check
    Unverifiable,
    /// could not check (no IP in the log, network off or failing)
    Unchecked,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Verified => "verified",
            Verdict::Mismatch => "mismatch",
            Verdict::Unverifiable => "unverifiable",
            Verdict::Unchecked => "unchecked",
        }
    }

    pub fn parse(s: &str) -> Verdict {
        match s {
            "verified" => Verdict::Verified,
            "mismatch" => Verdict::Mismatch,
            "unverifiable" => Verdict::Unverifiable,
            _ => Verdict::Unchecked,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IpList {
    pub creation_time: Option<String>,
    pub nets: Vec<IpNet>,
}

impl IpList {
    pub fn parse(text: &str) -> Option<IpList> {
        let v: Value = serde_json::from_str(text).ok()?;
        let nets: Vec<IpNet> = v
            .get("prefixes")?
            .as_array()?
            .iter()
            .filter_map(|p| {
                p.get("ipv4Prefix")
                    .or_else(|| p.get("ipv6Prefix"))
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse().ok())
            })
            .collect();
        (!nets.is_empty()).then(|| IpList {
            creation_time: v
                .get("creationTime")
                .and_then(Value::as_str)
                .map(String::from),
            nets,
        })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        self.nets.iter().any(|n| n.contains(&ip))
    }
}

/// DNS could not be asked (timeout, no resolver); not the same as "no record".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DnsUnavailable;

/// How to look a name up; DNS in production, a table in tests.
pub trait Resolver: Send + Sync {
    /// PTR name; Ok(None) = no record; Err = could not ask.
    fn reverse(&self, ip: IpAddr) -> Result<Option<String>, DnsUnavailable>;
    fn forward(&self, host: &str) -> Result<Vec<IpAddr>, DnsUnavailable>;
}

pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn reverse(&self, ip: IpAddr) -> Result<Option<String>, DnsUnavailable> {
        use dns_lookup::LookupErrorKind;
        match dns_lookup::lookup_addr(&ip) {
            Ok(name) if name.parse::<IpAddr>().is_ok() => Ok(None),
            Ok(name) => Ok(Some(name)),
            Err(e) if matches!(e.kind(), LookupErrorKind::NoName | LookupErrorKind::NoData) => {
                Ok(None)
            }
            Err(_) => Err(DnsUnavailable),
        }
    }

    fn forward(&self, host: &str) -> Result<Vec<IpAddr>, DnsUnavailable> {
        dns_lookup::lookup_host(host)
            .map(|ips| ips.collect())
            .map_err(|_| DnsUnavailable)
    }
}

/// Fetches a URL's body (blocking); swapped out in tests.
pub type Fetch = Box<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

pub fn http_fetch() -> Fetch {
    Box::new(|url: &str| {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .new_agent();
        agent
            .get(url)
            .header(
                "User-Agent",
                concat!("aieo-cli/", env!("CARGO_PKG_VERSION")),
            )
            .call()
            .map_err(|e| e.to_string())?
            .body_mut()
            .read_to_string()
            .map_err(|e| e.to_string())
    })
}

pub struct Verifier {
    cache_dir: Option<PathBuf>,
    fetch: Option<Fetch>,
    resolver: Box<dyn Resolver>,
    lists: Mutex<HashMap<String, Option<IpList>>>,
    rdns: Mutex<HashMap<(IpAddr, String), Verdict>>,
    /// one line per list that could not be fetched / fell back to a stale copy
    pub notes: Mutex<Vec<String>>,
}

const MAX_AGE: Duration = Duration::from_secs(24 * 3600);

fn file_name(url: &str) -> String {
    url.trim_start_matches("https://")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl Verifier {
    /// `fetch` None = offline: lists only from the cache, no DNS.
    pub fn new(
        cache_dir: Option<PathBuf>,
        fetch: Option<Fetch>,
        resolver: Box<dyn Resolver>,
    ) -> Verifier {
        Verifier {
            cache_dir,
            fetch,
            resolver,
            lists: Mutex::new(HashMap::new()),
            rdns: Mutex::new(HashMap::new()),
            notes: Mutex::new(Vec::new()),
        }
    }

    fn online(&self) -> bool {
        self.fetch.is_some()
    }

    fn load_list(&self, url: &str) -> Option<IpList> {
        let cached = self.cache_dir.as_ref().map(|d| d.join(file_name(url)));
        let fresh = cached.as_ref().and_then(|p| {
            let age = std::fs::metadata(p).ok()?.modified().ok()?;
            let text = std::fs::read_to_string(p).ok()?;
            Some((
                SystemTime::now().duration_since(age).unwrap_or_default() < MAX_AGE,
                text,
            ))
        });
        if let Some((true, text)) = &fresh {
            return IpList::parse(text);
        }
        if let Some(fetch) = &self.fetch {
            match fetch(url) {
                Ok(text) if IpList::parse(&text).is_some() => {
                    if let Some(p) = &cached {
                        let _ = std::fs::create_dir_all(p.parent().expect("cache dir"));
                        let _ = std::fs::write(p, &text);
                    }
                    return IpList::parse(&text);
                }
                Ok(_) => self.note(format!("{url}: not an IP range list")),
                Err(e) => self.note(format!("{url}: {e}")),
            }
        }
        match fresh {
            Some((false, text)) => {
                self.note(format!("{url}: using the cached copy older than a day"));
                IpList::parse(&text)
            }
            _ => None,
        }
    }

    fn note(&self, line: String) {
        let mut notes = self.notes.lock().expect("notes");
        if !notes.contains(&line) {
            notes.push(line);
        }
    }

    /// The crawler's list (loaded once per run) and its `creationTime`.
    pub fn list(&self, url: &str) -> Option<IpList> {
        let mut lists = self.lists.lock().expect("lists");
        if let Some(l) = lists.get(url) {
            return l.clone();
        }
        drop(lists);
        let loaded = self.load_list(url);
        lists = self.lists.lock().expect("lists");
        lists.insert(url.to_string(), loaded.clone());
        loaded
    }

    fn fcrdns(&self, ip: IpAddr, suffixes: &[String]) -> Verdict {
        let key = (ip, suffixes.join(","));
        if let Some(v) = self.rdns.lock().expect("rdns").get(&key) {
            return *v;
        }
        let verdict = match self.resolver.reverse(ip) {
            Err(DnsUnavailable) => Verdict::Unchecked,
            Ok(None) => Verdict::Mismatch,
            Ok(Some(name)) => {
                let name = name.trim_end_matches('.').to_ascii_lowercase();
                let suffix_ok = suffixes
                    .iter()
                    .any(|s| name == *s || name.ends_with(&format!(".{s}")));
                if !suffix_ok {
                    Verdict::Mismatch
                } else {
                    match self.resolver.forward(&name) {
                        Ok(ips) if ips.contains(&ip) => Verdict::Verified,
                        Ok(_) => Verdict::Mismatch,
                        Err(DnsUnavailable) => Verdict::Unchecked,
                    }
                }
            }
        };
        self.rdns.lock().expect("rdns").insert(key, verdict);
        verdict
    }

    /// Verdict for a request claiming `crawler` from `ip`, and the list
    /// version it was judged against.
    pub fn verify(&self, crawler: &CrawlerSpec, ip: Option<IpAddr>) -> (Verdict, Option<String>) {
        let Some(ip) = ip else {
            return (Verdict::Unchecked, None);
        };
        let has_rdns = !crawler.verify.rdns_suffixes.is_empty();
        let mut version = None;
        if let Some(url) = &crawler.verify.ip_ranges_url {
            match self.list(url) {
                Some(list) => {
                    version = list.creation_time.clone();
                    if list.contains(ip) {
                        return (Verdict::Verified, version);
                    }
                    if !has_rdns {
                        return (Verdict::Mismatch, version);
                    }
                }
                None if !has_rdns => return (Verdict::Unchecked, None),
                None => {}
            }
        }
        if has_rdns {
            if !self.online() {
                return (Verdict::Unchecked, version);
            }
            return (self.fcrdns(ip, &crawler.verify.rdns_suffixes), version);
        }
        (Verdict::Unverifiable, None)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use aieo_core::spec::spec;

    pub struct Table(pub Vec<(&'static str, &'static str)>);

    impl Resolver for Table {
        fn reverse(&self, ip: IpAddr) -> Result<Option<String>, DnsUnavailable> {
            Ok(self
                .0
                .iter()
                .find(|(i, _)| i.parse::<IpAddr>().ok() == Some(ip))
                .map(|(_, h)| h.to_string()))
        }
        fn forward(&self, host: &str) -> Result<Vec<IpAddr>, DnsUnavailable> {
            Ok(self
                .0
                .iter()
                .filter(|(_, h)| *h == host)
                .filter_map(|(i, _)| i.parse().ok())
                .collect())
        }
    }

    pub fn offline_lists() -> Fetch {
        Box::new(|url: &str| {
            if url.contains("searchbot") {
                Ok(r#"{"creationTime":"2026-01-02T11:00:00","prefixes":[{"ipv4Prefix":"104.210.140.128/28"},{"ipv6Prefix":"2a0b::/32"}]}"#.into())
            } else if url.contains("googlebot") || url.contains("common-crawlers") {
                Ok(r#"{"creationTime":"2026-10-21T14:46:09","prefixes":[{"ipv4Prefix":"66.249.64.0/27"}]}"#.into())
            } else {
                Err("offline".into())
            }
        })
    }

    fn crawler(id: &str) -> &'static CrawlerSpec {
        spec().crawlers.iter().find(|c| c.id == id).unwrap()
    }

    #[test]
    fn verifies_against_lists_and_dns() {
        let v = Verifier::new(
            None,
            Some(offline_lists()),
            Box::new(Table(vec![
                ("66.249.70.1", "crawl-66-249-70-1.googlebot.com"),
                ("203.0.113.5", "evil.example"),
                ("198.51.100.7", "crawl.googlebot.com"),
            ])),
        );
        let ip = |s: &str| Some(s.parse().unwrap());
        let oai = crawler("oai-searchbot");
        assert_eq!(
            v.verify(oai, ip("104.210.140.130")),
            (Verdict::Verified, Some("2026-01-02T11:00:00".into()))
        );
        assert_eq!(v.verify(oai, ip("1.2.3.4")).0, Verdict::Mismatch);
        assert_eq!(v.verify(oai, ip("2a0b::1")).0, Verdict::Verified);
        let g = crawler("googlebot");
        assert_eq!(v.verify(g, ip("66.249.64.5")).0, Verdict::Verified);
        assert_eq!(
            v.verify(g, ip("66.249.70.1")).0,
            Verdict::Verified,
            "FCrDNS"
        );
        assert_eq!(
            v.verify(g, ip("203.0.113.5")).0,
            Verdict::Mismatch,
            "wrong domain"
        );
        // no PTR record at all
        assert_eq!(v.verify(g, ip("198.51.100.8")).0, Verdict::Mismatch);
        assert_eq!(
            v.verify(crawler("bytespider"), ip("1.2.3.4")).0,
            Verdict::Unverifiable
        );
        assert_eq!(
            v.verify(crawler("gptbot"), ip("1.2.3.4")).0,
            Verdict::Unchecked,
            "list unavailable"
        );
        assert_eq!(v.verify(oai, None).0, Verdict::Unchecked);
        assert_eq!(v.notes.lock().unwrap().len(), 1);
        let offline = Verifier::new(None, None, Box::new(Table(vec![])));
        assert_eq!(offline.verify(g, ip("66.249.70.1")).0, Verdict::Unchecked);
    }
}
