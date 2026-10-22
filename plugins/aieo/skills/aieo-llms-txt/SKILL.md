---
name: aieo-llms-txt
description: Write or fix a site's llms.txt (and llms-full.txt) from the pages the build actually contains, and check that every link in it resolves to an indexable, canonical page. Use when the user asks to "add llms.txt", "generate llms.txt", "is my llms.txt correct", or aieo reports F16, C-LLMS-LINK-BROKEN or C-LLMS-LINK-NOINDEX.
---

# llms.txt from the build

Be straight with the user about what llms.txt does: it is a proposed
convention (llmstxt.org) that some AI tools and agents read. Google Search
does not use it, and there is no evidence that it raises citations in AI
answers. Its value is a clean, current map of the site for tools that do
read it. Do not present it as a ranking or visibility fix.

## Write it

1. Build the site; `aieo check --format json` and read
   `inventory.pages`: only pages with `indexable: true` whose `canonical`
   is their own `url` belong in llms.txt; `title` gives the link text.
2. Put the file in the framework's static directory so it is served at
   `/llms.txt` (`public/` for Astro, Next, Vite, Nuxt; `static/` for Hugo
   and Docusaurus; `public/` inside the VitePress source directory) — or use the framework's
   llms.txt plugin if the project already has one.
3. Format (Markdown):

   ```markdown
   # Site or product name

   > One or two sentences: what the site is and who it is for.

   ## Docs
   - [Getting started](https://example.com/start/): what the page covers
   - [Configuration](https://example.com/config/): …

   ## Optional
   - [Changelog](https://example.com/changelog/)
   ```

   Absolute URLs on the production origin, exactly as canonical (trailing
   slash included). Group by the site's own sections; describe each page
   in a few words taken from its content, not invented.
4. Rebuild and run `aieo check`:
   - `C-LLMS-LINK-BROKEN` — the link points at no page in the build.
   - `C-LLMS-LINK-NOINDEX` — the page is noindex or canonicalises
     elsewhere; link the canonical URL or drop it.
   - `F16` — presence and structure of llms.txt / llms-full.txt.

## Keep it current

If the site has many pages or changes often, prefer generating llms.txt in
the build (a plugin or a small script over the content collection) to a
hand-written file, and let `aieo diff` in CI catch links that go stale.
