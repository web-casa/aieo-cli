# aieo-cli

SEO/GEO checks for a site's **build output**, before it ships — the inside
view for [aieo.ee](https://aieo.ee).

aieo.ee checks what a site serves over HTTP. `aieo` reads what the build
produced (`dist/`) and what the framework declares, and checks that the
pieces agree with each other: sitemap ↔ robots.txt ↔ canonical ↔ hreflang ↔
internal links ↔ llms.txt. With a baseline it fails a build only on what the
change made worse.

```sh
aieo init                       # detect the framework, write aieo.toml
aieo check                      # check ./dist; exit 1 when the gate fails
aieo snapshot                   # save .aieo/baseline.json
aieo diff                       # fail only on findings new since the baseline
aieo export                     # .aieo/bundle.json.gz (aieo-bundle/v1) for aieo.ee
aieo rules / aieo explain C-SITEMAP-URL-NOINDEX
```

Presets in 0.1: `static` (any build directory, `_headers` / `_redirects`
honoured), `astro`, `starlight` (pages are traced back to
`src/content/docs/**` / `src/pages/**`). Next.js, Nuxt/Vue, TanStack, Hugo,
VitePress and Docusaurus follow in 0.2.

## What it checks

| Family | | Examples |
|---|---|---|
| **F01–F18** | aieo.ee's AI-readiness modules, judged from the build | robots.txt AI-crawler matrix, noindex, sitemap, canonical, llms.txt, content without JS |
| **T-…** | per page | missing title, duplicate titles (hreflang alternates excepted), malformed JSON-LD |
| **C-…** | across the build | sitemap lists a page that is missing / noindex / non-canonical / on another host / blocked by robots.txt; hreflang without return link; canonical to a missing page or another host; broken internal links; orphan pages; llms.txt links to missing pages; `Disallow: /` shipped to production; AI search crawlers blocked |
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
- uses: web-casa/aieo-cli@v0.1     # composite action, see action.yml
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
`aieo-bundle/v1` (`spec/bundle.schema.json`): findings, page inventory and
scores — never page source, raw logs or IP addresses.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
# refresh spec/ from aieo.ee's @aieo/rules (in the aieo-v5 monorepo)
pnpm --filter @aieo/rules export:cli <path to this repo>
```

Crates: `aieo-core` (spec, findings, config, bundle) · `aieo-site` (build
directory as a static host: routes, `_headers`, `_redirects`, robots,
sitemaps, HTML, presets) · `aieo-rules` (F / T / C / D) · `aieo-report`
(formats) · `aieo-cli` (the `aieo` binary).

MIT licensed.
