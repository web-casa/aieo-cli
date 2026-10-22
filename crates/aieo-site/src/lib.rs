//! A build directory seen the way a static host serves it, and the page
//! facts the rules read from it.

pub mod hosting;
pub mod html;
pub mod preset;
pub mod robots;
pub mod scan;
pub mod site;
pub mod sitemap;

pub use site::{Followed, Resolution, Site, SiteOptions};
