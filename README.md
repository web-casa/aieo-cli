# aieo-cli

SEO/GEO checks for a site's **build output**, before it ships — the inside
view for [aieo.ee](https://aieo.ee).

aieo.ee checks what a site serves over HTTP. `aieo` reads what the build
produced (`dist/`), what the framework declares and what the deployment
config adds (`_redirects`, `_headers`, `vercel.json`, `netlify.toml`,
Next.js manifests), and checks that the pieces agree with each other:
sitemap ↔ robots.txt ↔ canonical ↔ hreflang ↔ internal links ↔ llms.txt ↔
redirects ↔ framework settings. With a baseline it fails a build only on
what the change made worse.

```sh
aieo init                       # detect the framework, write aieo.toml
aieo check                      # check ./dist; exit 1 when the gate fails
aieo snapshot                   # save .aieo/baseline.json
aieo diff                       # fail only on findings new since the baseline
aieo export                     # .aieo/bundle.json.gz (aieo-bundle/v1) for aieo.ee
aieo rules / aieo explain C-SITEMAP-URL-NOINDEX
aieo logs ingest /var/log/nginx/  # crawler & AI-referral evidence from access logs
aieo logs report                # coverage, verification, blocking, errors, AI referrals
```

## Frameworks

The preset is detected from `package.json` and config files (`--preset` to
force one). Findings point at the source file a page was built from.

| Preset | Build checked | Read from the config (literals only) | Pages traced to |
|---|---|---|---|
| `static` | any directory | — | — |
| `astro`, `starlight` | `dist/` (`outDir`) | `site`, `base`, `trailingSlash` | `src/pages/**`, `src/content/docs/**` |
| `next` | `out/` (`output: 'export'`) or `.next/` | `basePath`, `trailingSlash`, `output`, `metadataBase` | `app/**/page.*`, `pages/**` |
| `nuxt` | `.output/public` | `site.url`, `app.baseURL`, `ssr`, `routeRules` redirects | `app/pages/**`, `pages/**` |
| `vue` | `dist/` (`build.outDir`) | `base` | `src/pages/**` (file routing) |
| `tanstack` | `dist/client`, `.output/public`, `dist/` | `base` | `src/routes/**` |
| `hugo` | `public/` (`publishDir`) | `baseURL`, `uglyURLs`, languages | `content/**` |
| `vitepress` | `.vitepress/dist` | `sitemap.hostname`, `base`, `cleanUrls`, `srcDir` | `**/*.md` |
| `docusaurus` | `build/` | `url`, `baseUrl`, `trailingSlash` | `docs/**`, `blog/**`, `src/pages/**` |

Server builds (Next.js `.next/`, Nuxt and TanStack Start with a server
bundle) are served as production would: prerendered HTML, `public/`, the
redirects/rewrites/headers of `routes-manifest.json`, and every other route
as *rendered on request* — links to it count as working, its HTML is not
checked, and a server-rendered homepage makes the homepage modules
unverified (exit 2 unless `--allow-incomplete`). Client-rendered SPAs are
checked for what crawlers without JavaScript see (`K-SPA-SHELL`).
`fixtures/presets/` holds a real build of the same site per framework with
its snapshot (`cargo test -p aieo-cli --test presets`).

## What it checks

| Family | | Examples |
|---|---|---|
| **F01–F18** | aieo.ee's AI-readiness modules, judged from the build | robots.txt AI-crawler matrix, noindex, sitemap, canonical, llms.txt, content without JS |
| **P01–P04** | multi-page modules (GEOReady's), over every indexable page, scored 0–100 on their own | unique metadata, navigation and substance, API / product data, one organization identity |
| **T-…** | per page | missing title, duplicate titles (hreflang alternates excepted), malformed JSON-LD, JSON-LD without schema.org `@context`, missing properties Google requires for a rich result, retired rich result types (HowTo, FAQPage, …) |
| **C-…** | across the build | sitemap lists a page that is missing / noindex / non-canonical / on another host / blocked by robots.txt; hreflang without return link; canonical to a missing page or another host; broken internal links; orphan pages; llms.txt links to missing pages; `Disallow: /` shipped to production; AI search crawlers blocked |
| **K-…** | configuration | framework `site` / base path / trailing-slash setting contradicted by canonical and sitemap URLs; redirect loops, chains and dead targets (with file and line); a header rule that noindexes sitemap pages; Next.js `redirects()` ignored by static export; routes answered by one client-rendered shell |
| **L-…** | access logs (`aieo logs`) | crawler blocked by WAF/CDN, crawler 5xx, robots.txt unavailable, 404 spike after a deploy, spoofed crawlers, disallowed paths crawled, low AI-search coverage |
| **D-…** | against a baseline | page removed without redirect, page turned noindex, canonical changed, hreflang or structured data lost, content shrank |

Every rule has an id, a tier and a source: `aieo rules`, `aieo explain <id>`.

**Tiers**: `error` = a deterministic defect (fails the default gate),
`warning` = high risk (fails a `diff` when new), `notice` = advice. Rules
without an official source are marked heuristic and are never errors by
default; length-based title rules are off unless enabled.

**Exit codes**: `0` gate passed · `1` gate failed · `2` incomplete evidence
(no homepage, origin unknown, undecodable HTML) or a usage error — a check
that could not see enough never reports "all clear".

## Same rules as aieo.ee

The rule metadata, crawler catalog and bundle schema in `spec/` are exported
from `@aieo/rules`, the package aieo.ee's scanner reads. F02–F18 are
implemented twice (TypeScript in aieo.ee, Rust here) and held together by
the conformance fixtures in `spec/fixtures/conformance`: the same tiny sites,
the same module status, ratio and reason (`cargo test -p aieo-rules --test
conformance`). F01 (HTTPS, redirects) needs a live origin; F15 reads a `.md`
twin of the homepage instead of content negotiation.

## Access logs

```sh
aieo logs ingest access.log access.log.1.gz   # Nginx/Apache, Caddy, Cloudflare Logpush,
                                              # Vercel drains, CloudFront — detected per file
aieo logs ingest app.log --format "nginx:\$host \$remote_addr [\$time_local] \"\$request\" \$status …"
CLOUDFLARE_API_TOKEN=… aieo logs pull cloudflare --zone <zone id> --since 7d
aieo logs report [--since 30d] [--format json]
aieo logs gaps --actor search_index           # indexable pages AI search crawlers never fetched
aieo logs referrals                           # landing pages of visits from ChatGPT, Perplexity, …
```

- **Who**: user agents are matched against the shared crawler catalog
  (`spec/crawlers.json`: OAI-SearchBot, ChatGPT-User, GPTBot, Claude-*,
  PerplexityBot, Googlebot, Bingbot, Applebot, …) and classed as
  `search_index`, `user_fetch`, `training`, `classic_search`, other
  automation or people. Visits whose referrer (or `utm_source`) is an AI
  answer platform are AI referrals.
- **Verified or not**: a claimed crawler is checked against its
  operator's published IP ranges (downloaded, cached a day under
  `.aieo/cache/`, list `creationTime` recorded), then by
  forward-confirmed reverse DNS where the operator documents it.
  `mismatch` = spoofed; a crawler without a published method is
  `unverifiable`, never called fake. `--offline` skips the network.
- **Kept locally**: `.aieo/logs.db` (SQLite). A file is recognised by its
  first bytes, so a growing log resumes and a rotated copy is not counted
  twice. Crawler requests keep a salted IP hash (`.aieo/salt`), people only
  per-day counts — no IPs, no browser user agents. `.aieo/.gitignore`
  keeps the salt, database and caches out of git.
- **Findings (L)**: a crawler mostly answered 401/403/429 (WAF / bot
  protection), crawler 5xx, robots.txt unavailable (5xx/429), a crawler
  404 spike, spoofed crawlers, verified crawlers on disallowed paths, AI
  search crawlers reaching few indexable pages. With a build next to the
  logs, coverage is measured against its indexable pages.
- **Cloudflare GraphQL** returns sampled hourly counts without IPs: rows
  carry their count, verification stays `unchecked`, and the plan's
  limits are read from the zone's `settings`.
- `aieo export` adds the log aggregates (`logs` in `aieo-bundle/v1`) when
  `.aieo/logs.db` exists.

## Configuration (`aieo.toml`)

```toml
site = "https://example.com"     # production origin
dist = "dist"
preset = "starlight"

[check]
# fail_on = ["error"]            # default; with a baseline ["new:error", "new:warning"]
baseline = ".aieo/baseline.json"
ignore = ["/drafts/**"]          # URL paths left out of page rules

[rules]
"C-ORPHAN-PAGE" = "notice"       # error | warning | notice | off | on

[[suppress]]                     # every suppression has an owner, a reason and an expiry
rule = "C-LINK-BROKEN"
url = "/legacy/**"
reason = "old docs, removed in Q1"
owner = "docs-team"
expires = "2026-12-31"
```

## CI

```yaml
- run: npm run build
- uses: web-casa/aieo-cli@v0.3     # composite action, see action.yml
  with:
    command: diff                  # or check
    upload-sarif: "true"           # GitHub code scanning (security-events: write)
```

Findings appear as annotations on the pull request, a Markdown summary is
added to the job summary, SARIF and the evidence bundle are written next to
the project. Without the action:
`aieo diff --format github --report sarif=aieo.sarif --report bundle=.aieo/bundle.json.gz`.

## Output and data

`--format pretty | json | sarif | github | markdown`, plus any number of
`--report FORMAT=PATH` (`bundle` = gzip JSON). JSON and the bundle are
`aieo-bundle/v1` (`spec/bundle.schema.json`): findings, page inventory,
readiness and multi-page scores — never page source, raw logs or IP
addresses. Configuration findings carry the config file and line
(`astro.config.mjs:4`, `vercel.json:12`), so SARIF and GitHub annotations
land on the setting to change.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
# refresh spec/ from aieo.ee's @aieo/rules (in the aieo-v5 monorepo)
pnpm --filter @aieo/rules export:cli <path to this repo>
```

Crates: `aieo-core` (spec, findings, config, bundle) · `aieo-site` (build
directory as production serves it: routes, host rules, robots, sitemaps,
HTML, framework presets) · `aieo-rules` (F / P / T / C / K / D) ·
`aieo-report` (formats) · `aieo-logs` (parsers, classification, IP
verification, SQLite store, Cloudflare GraphQL) · `aieo-cli` (the `aieo`
binary).

MIT licensed.
