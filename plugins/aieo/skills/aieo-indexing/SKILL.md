---
name: aieo-indexing
description: Work out why pages of a site are not indexed or show the wrong canonical in Google, and tell Bing and other IndexNow engines about changed pages — using the user's own Search Console and Bing Webmaster access, read against the site's build. Use when the user asks "why isn't this page on Google", "Google picked a different canonical", "submit my new pages", "set up IndexNow", or "did my clicks drop".
---

# Indexing: Search Console, Bing, IndexNow

Everything here uses the **user's** credentials, stored on their machine.
You never create, request or paste credentials: tell the user which
command to run and let them do it.

## Connect (the user runs these)

- Google: `aieo search google login` — opens a browser with the user's own
  Desktop-app OAuth client. `aieo search google properties` lists what it
  reaches.
- Bing: `aieo search bing key` (Bing Webmaster → Settings → API Access), or
  `BING_WEBMASTER_API_KEY` in the environment.

## Why is a page not indexed?

1. Build the site; the MCP `page` tool (or the page's record in
   `aieo check --format json` → `inventory.pages`) shows it as the build
   has it: `indexable`, `canonical`, `in_sitemap`, `inlinks`, and its
   findings.
2. `aieo search google inspect --urls <file>` (or without `--urls`: the
   stalest indexable pages, within Google's daily quota) records URL
   Inspection results.
3. `aieo search report` (MCP `search_report`) reads them against the build:
   - `S-NOT-INDEXED` — indexable in the build, not in Google's index. Check
     the build side first: orphan page (`C-ORPHAN-PAGE`), missing from the
     sitemap, thin or duplicate content. Then it is Google's decision; say
     so rather than promising indexing.
   - `S-CANONICAL-OVERRIDDEN` — Google chose another URL; usually
     duplicates, redirects or inconsistent internal links to the page.
   - `S-LIVE-DIFFERS` — production serves something else than the build
     (stale deploy, CDN rule, middleware).
   - `S-CLICKS-DROPPED` — pull data first: `aieo search google pull`.

## IndexNow (Bing, Yandex, Seznam, Naver, …; not Google)

1. `aieo indexnow keygen --write` puts a key file into the framework's
   static directory and prints the `aieo.toml` `[indexnow]` line. The key
   is public by design.
2. After deploying: `aieo indexnow verify` checks the key file on the live
   site.
3. `aieo indexnow submit` prints the **plan** (new, changed, removed URLs
   since the baseline). Show it to the user. Only the user runs
   `aieo indexnow submit --yes`; it is logged in `.aieo/audit.jsonl`.
4. After submitting, `aieo snapshot` so the next plan starts from here.

Do not submit unchanged pages repeatedly or use `--all` on a large site
without the user asking: engines throttle sites that over-submit.
