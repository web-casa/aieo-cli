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
aieo search google inspect      # Google's view of the build's pages (URL Inspection)
aieo indexnow submit --yes      # tell Bing & co. what changed since the baseline
aieo login && aieo push         # upload the bundle to the aieo.ee project
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
| **S-…** | search engines (`aieo search`) | indexable page not indexed, canonical overridden by Google, production answers differently than the build, clicks halved, IndexNow key not served |
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

## Search engines

Everything runs with your own credentials, stored in your config
directory (`~/.config/aieo/credentials.json`, owner-only) or taken from
the environment in CI. Data lands in `.aieo/search.db`.

```sh
# Google Search Console — your own "Desktop app" OAuth client from Google Cloud
aieo search google login --client-secret client_secret.json   # browser, loopback redirect
aieo search google pull --days 28        # clicks per page and day, top queries
aieo search google inspect [--limit 200] # URL Inspection of indexable pages, stalest first
# Bing Webmaster Tools — API key from Settings → API Access
aieo search bing key                     # or BING_WEBMASTER_API_KEY
aieo search bing pull
aieo search bing import-ai export.csv    # AI Performance overview export (Date, Citations, Cited Pages)
aieo search report
```

- **CI**: set `GOOGLE_APPLICATION_CREDENTIALS` to a service account key
  whose e-mail is a user of the Search Console property (the device flow
  does not allow Search Console scopes).
- **URL Inspection** keeps a local quota ledger (2,000 per property a
  day, 600 a minute) and inspects the URLs it looked at longest ago first.
- **Findings (S)**, read against the build: an indexable page Google has
  not indexed, a canonical Google overrode, a page the build serves that
  Google could not fetch / found blocked / saw noindexed (production is
  not what was built), clicks halved against the previous period, an
  IndexNow key the build does not serve.
- Bing's AI Performance and Google's AI Overviews report have no API; the
  Bing overview export is imported with the same rules as aieo.ee.

### IndexNow

```sh
aieo indexnow keygen --write             # key file into public/ (static/ for Hugo, Docusaurus)
aieo indexnow verify                     # in the build? served live?
aieo indexnow submit                     # plan: new, changed and removed pages since the baseline
aieo indexnow submit --yes               # send (Bing, Yandex, Seznam, Naver, … share it; not Google)
```

Changes are found by comparing page hashes with the baseline
(`aieo snapshot`), so only what a deploy touched is sent. Nothing is
sent without `--yes`, and every request is recorded in
`.aieo/audit.jsonl`.

## aieo.ee

```sh
aieo login                 # an organisation API key with write access; finds the project for `site`
aieo push                  # export the bundle (check + logs + search) and upload it
```

`aieo push` uploads to `[aieo] project` in `aieo.toml` (or `--to <id>`).
The same commit and kinds uploaded twice are recognised as one run. The
bundle carries findings, page inventory and aggregates — no page source,
raw logs or IP addresses — and can also be dropped on the project's
"Inside view" page.

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

[aieo]
project = "…"                    # aieo.ee project id for `aieo push`

[search]
google_property = "sc-domain:example.com"   # default: the property covering `site`
bing_site = "https://example.com/"

[indexnow]
key = "…"                        # public; served as /<key>.txt
```

## CI

```yaml
- run: npm run build
- uses: web-casa/aieo-cli@v0.4     # composite action, see action.yml
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
verification, SQLite store, Cloudflare GraphQL) · `aieo-search` (Search
Console, Bing Webmaster, IndexNow, credentials, aieo.ee upload) ·
`aieo-cli` (the `aieo` binary).

MIT licensed.
