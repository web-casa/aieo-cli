---
name: aieo
description: Check a website's build output for SEO and GEO (AI-search) defects with the aieo CLI and fix them in the source files — sitemap, robots.txt, canonical, hreflang, broken links, llms.txt, structured data, framework and hosting config; read crawler logs and Search Console / Bing data against the build. Use when the user asks to audit or fix SEO / AI-search readiness of a site they build locally (Astro, Starlight, Next.js, Nuxt, Vue, TanStack, Hugo, VitePress, Docusaurus or any static build), to check a build before deploying, to see whether AI crawlers reach their pages, or why Google does not index a page.
---

# aieo — SEO / GEO checks for a site's build

`aieo` reads what the build produced and what the framework and host
declare, and reports defects with the build file and the **source file**
to change. It never edits files and never calls an LLM. Findings are facts
with evidence; each has a rule id you can explain.

## Before anything

1. `aieo --version` — if missing, ask the user before installing:
   `curl -fsSL https://raw.githubusercontent.com/web-casa/aieo-cli/main/scripts/install.sh | sh`
   or `cargo install --git https://github.com/web-casa/aieo-cli aieo-cli`.
2. Build the site first (`npm run build`, `hugo`, …). aieo checks build
   output; a stale build gives stale answers.
3. No `aieo.toml`? Run `aieo init` (detects the framework, build directory
   and production origin). If the origin is unknown, ask the user for it
   and pass `--site https://…`.

## The fix loop

```sh
aieo check --format json > /tmp/aieo.json     # or the MCP tool `check`
```

- `summary.gate.passed`, `summary.findings` (error / warning / notice).
- Each finding: `rule`, `tier`, `url`, `file` + `line` (build file),
  `source` (the source file to edit), `evidence` (what was seen).
- Work through **errors first**, then warnings. Notices are advice.
- For each finding: `aieo explain <RULE>` says what it means and cites the
  source. Edit the **source file** (never the build output — it is
  regenerated), rebuild, and run `aieo check` again until the finding is gone.
- Configuration findings (`K-…`) point at the config file and line
  (`astro.config.mjs:4`, `vercel.json:12`).
- Heuristic findings (`evidence_level: heuristic`) are judgement calls:
  explain them to the user rather than changing content on your own.

Exit codes: `0` gate passed, `1` gate failed, `2` incomplete evidence (no
homepage, unknown origin, server-rendered homepage) or a usage error —
read the message, it says what to do.

### Regressions

`aieo snapshot` on a known-good build, then `aieo diff` fails only on
what is new. Use it in CI (the GitHub Action `web-casa/aieo-cli@v1`).

## What the rule families mean

| Family | About |
|---|---|
| F01–F18 | AI-readiness modules (same results as aieo.ee) |
| P01–P04 | consistency across pages (metadata, navigation, entity) |
| T | one page: title, h1, JSON-LD (incl. Google's required properties) |
| C | across the build: sitemap ↔ robots ↔ canonical ↔ hreflang ↔ links ↔ llms.txt |
| K | framework / hosting config: origin, base path, trailing slash, redirects, header noindex, SPA shell |
| L | access logs: blocked or erroring crawlers, robots.txt availability, spoofed crawlers, AI-search coverage |
| S | search engines vs the build: not indexed, canonical overridden, production differs |
| D | regressions against the baseline |

## Logs and search engines (when the user has them)

- `aieo logs ingest <files>` then `aieo logs report` / `aieo logs gaps`:
  which crawlers (verified against their published IP ranges) fetched
  which pages; `L-BOT-BLOCKED` usually means a WAF / bot rule blocks an
  AI crawler — tell the user which rule to look at, do not guess.
- `aieo search google pull|inspect`, `aieo search report`: Google's view
  of the pages. These need the user's own credentials (`aieo search
  google login`) — ask, never create or paste credentials yourself.

## Never without the user

These change things outside the machine; propose them, let the user run
them:

- `aieo indexnow submit --yes` (show the plan from `aieo indexnow submit`
  without `--yes` first)
- `aieo push` (uploads the evidence bundle to aieo.ee)
- `aieo login`, `aieo search google login`, `aieo search bing key`

## MCP

`aieo mcp` serves the same checks as read-only tools (`check`, `diff`,
`page`, `explain_rule`, `list_rules`, `logs_report`, `logs_gaps`,
`search_report`, `indexnow_plan`, `push_plan`). Each tool takes an
optional `project` (a directory relative to where the server runs) — use
it for one site in a monorepo, e.g. `{"project": "apps/docs"}`.

## Related skills

`aieo-ci` (CI gate with a baseline), `aieo-crawlers` (access logs),
`aieo-indexing` (Search Console, Bing, IndexNow), `aieo-llms-txt`.
