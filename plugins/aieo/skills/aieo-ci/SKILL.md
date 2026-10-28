---
name: aieo-ci
description: Add an aieo SEO / AI-search check to a site's CI so pull requests fail only on problems they introduce — GitHub Actions (annotations, SARIF code scanning) or GitLab CI (JUnit report on merge requests), with a baseline snapshot. Use when the user wants to stop SEO regressions (broken links, sitemap, canonical, hreflang, noindex, redirects) from shipping, or asks to "add aieo to CI", "check the build on every PR". Also for CI 里加 SEO 检查, 防止 SEO 回归.
---

# Put aieo in CI

The aim is a gate that fails only on what a change makes worse. Existing
problems are recorded in a baseline and reported, not failed on.

## 1. Make the local check pass first

1. `aieo --version` (missing: see the `aieo` skill), build the site,
   `aieo init` if there is no `aieo.toml`.
2. `aieo check` — read the result with the user. Decide together which
   rules to turn down or off in `aieo.toml` `[rules]`; never silence a
   rule without telling the user why.
3. `aieo snapshot` writes `.aieo/baseline.json`. Commit it (and
   `aieo.toml`). From now on `aieo diff` fails only on new error and
   warning findings.

## 2. GitHub Actions

Add a step after the build in the workflow that builds the site:

```yaml
permissions:
  contents: read
  security-events: write          # only if upload-sarif is "true"

steps:
  # … checkout, install, build …
  - uses: web-casa/aieo-cli@v1
    with:
      version: 1.3.0              # prebuilt binary; the default builds from source
      command: diff               # "check" when there is no baseline
      upload-sarif: "true"
```

Findings appear as annotations on the pull request and a summary in the
job. Check the Action's inputs in `action.yml` of web-casa/aieo-cli before
adding others.

## 3. GitLab CI

```yaml
include:
  - remote: https://raw.githubusercontent.com/web-casa/aieo-cli/v1.3.0/ci/gitlab-ci.yml

aieo:
  extends: .aieo
  needs: [build]                  # the job that produces the build directory
  variables:
    AIEO_COMMAND: diff            # the template's default is check
```

Findings show as a JUnit test report on the merge request.

## 4. Keep the baseline honest

- After fixing findings, run `aieo snapshot` again and commit, so fixed
  problems cannot come back unnoticed.
- After upgrading aieo, findings of newly added rules show as `unknown`
  (never fail the diff); review them, then snapshot to adopt them.

## Do not, without the user

- Add `push: "true"` (uploads to aieo.ee) or `indexnow: "true"` (contacts
  search engines). Both need the user's decision and secrets
  (`AIEO_API_KEY`) that only they can create.
- Create or paste secrets. Tell the user which repository secret to add.
