//! Evidence from search engines and the connection to aieo.ee: Google
//! Search Console (Search Analytics, URL Inspection), Bing Webmaster Tools,
//! IndexNow submissions, and uploading bundles. Credentials stay in the
//! user's config directory; every write to a third party is planned first
//! (dry run) and recorded in `.aieo/audit.jsonl`.

pub mod bing;
pub mod bing_ai;
pub mod cloud;
pub mod creds;
pub mod google;
pub mod google_auth;
pub mod http;
pub mod indexnow;
pub mod report;
pub mod store;
