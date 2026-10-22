//! Crawler and AI-referral evidence from access logs: parse (Nginx,
//! Apache, Caddy, Cloudflare, Vercel, CloudFront), classify against the
//! shared crawler catalog, verify claimed crawlers by IP range or
//! forward-confirmed reverse DNS, keep it in `.aieo/logs.db`, report.

pub mod classify;
pub mod cloudflare;
pub mod hit;
pub mod ingest;
pub mod parse;
pub mod report;
pub mod store;
pub mod verify;

#[cfg(test)]
mod scenario;
