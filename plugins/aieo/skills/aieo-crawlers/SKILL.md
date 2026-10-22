---
name: aieo-crawlers
description: Find out from server or CDN access logs whether search and AI crawlers (Googlebot, Bingbot, GPTBot, OAI-SearchBot, ClaudeBot, PerplexityBot, …) reach a site's pages, which ones are blocked, erroring or spoofed, which pages they never fetch, and which pages visitors reach from AI answers. Use when the user asks "do AI crawlers visit my site", "is Cloudflare / the WAF blocking GPTBot", "why is my site not in ChatGPT / Perplexity", or has Nginx, Apache, Caddy, Cloudflare, Vercel or CloudFront logs.
---

# Crawler evidence from access logs

aieo reads logs locally into `.aieo/logs.db`, verifies each crawler claim
against the operator's published IP ranges or reverse DNS, and reads the
result against the build. Raw logs and IP addresses never leave the
machine.

## Get the logs in

Ask the user where the logs are; do not go looking on servers yourself.

- Files (Nginx, Apache, Caddy JSON, Cloudflare Logpush, Vercel, CloudFront;
  `.gz` too): `aieo logs ingest <files or directory>` — the format is
  detected; `--format nginx:'<log_format>'` for a custom Nginx format.
- Cloudflare without Logpush: `aieo logs pull cloudflare --zone <id>` with
  an API token (Analytics:Read) the user puts in `CLOUDFLARE_API_TOKEN`.
  These are sampled hourly counts, so page-level coverage is not available.

Build the site before reporting: coverage compares crawled paths with the
build's indexable pages.

## Read it

- `aieo logs report` (or the MCP tool `logs_report`): per crawler —
  requests, verified or not, status codes; coverage of indexable pages;
  AI referrals; `L-*` findings.
- `aieo logs gaps --actor search_index` (MCP `logs_gaps`): indexable pages
  that a class of crawler never fetched. Actors: `search_index`,
  `classic_search`, `user_fetch`, `training`.
- `aieo logs referrals`: landing pages visitors reached from AI answer
  platforms.

## What the findings mean and what to tell the user

| Rule | Usually means | Say |
|---|---|---|
| `L-BOT-BLOCKED` | a WAF / bot-management rule or a server rule answers 403/429 | which crawler, which status, and where such rules live (Cloudflare Security → Bots / WAF, server config); do not guess the exact rule |
| `L-BOT-SERVER-ERRORS` | crawlers get 5xx | the paths and times, so the user can match server logs |
| `L-ROBOTS-TXT-UNAVAILABLE` | robots.txt errors for crawlers | crawlers may treat the whole site as disallowed |
| `L-SPOOFED-BOT` | requests claim a crawler's name from IPs it does not use | they are not that crawler; blocking them is safe |
| `L-DISALLOWED-CRAWLED` | a crawler fetched paths robots.txt disallows | check whether it was verified before calling it misbehaviour |
| `L-AI-SEARCH-COVERAGE-LOW` | AI search crawlers reach few indexable pages | combine with `gaps` and the internal-link / sitemap findings of `aieo check` |

Training crawlers (`GPTBot`, `ClaudeBot`, `CCBot`, …) and AI *search*
crawlers (`OAI-SearchBot`, `PerplexityBot`, `Claude-SearchBot`, …) are
different decisions. Never change robots.txt or a WAF rule to allow or
block one without the user choosing it.
