# aieo-cli 调研与方案（v1.0）

> 日期：2026-10-22 · 状态：方案已确认（8 个关键决策见第 10 节）
> 定位：**aieo.ee 从外部看网站（HTTP），aieo-cli 从内部看网站（构建产物、源码配置、服务器/CDN 日志、搜索平台数据）**。两者共用规则规格，CLI 导出的版本化证据包由 aieo.ee 识别，参与评分、排序和建议。

---

## 1. 调研结论

### 1.1 调研范围
- **外部项目**：38 个低 Star SEO/GEO CLI。
  - 找到 36 个。`webaudit`、`sitemap-gen` 无法确认对应仓库。
  - `gsc-cli(Go)`、`IndexNow CLI`、`DataForSEO API CLI` 名称有歧义，取最匹配的仓库。
  - 另以 Lighthouse CI、Unlighthouse、Screaming Frog CLI、linkinator 作为成熟工具对照。
- **内部项目**：`~/aieo-v5`、`isaieoready`（GEOReady）、`llmstxtdb`、`indexsubmit`、`geo-tools`、`webcasa-cms`（HeoPress）。
- **方法**：GitHub API 元数据 + README/源码阅读，**没有逐一安装运行**。

### 1.2 外部项目分层与代表

| 层 | 代表项目（⭐） | 借鉴点 |
|---|---|---|
| 构建产物 / CI 门禁 | Search Quality Kit (2)、Astro SEO Audit (8)、Capgo SEO Checker (6)、nurkamol/seo-audit (3) | 框架预设；`--baseline --fail-on-new`；SARIF 与 GitHub 注解；问题回指 `src/pages`；豁免需写明 owner、reason、到期日；导出 policy contract 给 Agent；把大量发现项归并为"模板级原因" |
| 跨产物一致性 | crawlcontract (0, Rust) | 把 sitemap、robots、canonical、hreflang、链接统一建成 URL 状态图；preview→production 源映射；证据不完整时 fail-closed（exit 2） |
| 线上部署陷阱 | vercel-seo-audit (78) | Next.js 308 尾斜杠、www 与 apex、Vercel 缓存头、预览域名识别 |
| 渲染差异 | Addy0 Crawler (1)、Open SEO Crawler (70) | 原始 HTML 与渲染后 DOM 的差异规则；用不同 Bot UA 请求同一页面对比 |
| 结构化数据 | **schemaorg-rs (7, Rust, MIT)** | 离线校验 JSON-LD、Microdata、RDFa，内置 Schema.org v30。**可作为 crate 直接依赖** |
| 日志 | aeo-crawl-radar (2)、crawler-log-analyser (0)、davo20019/seo-analysis (0) | 找出 sitemap 中"从未被 AI 爬虫访问"的页面；部署窗口内 Bot 遇到的 404 和 robots 故障；单 IP 轮换多个 Bot 名；日志与爬取结果关联 |
| 搜索平台数据 | nalyk/gsccli (5)、KLIXPERT gsc-cli (6)、bing-webmaster-ai-cli-mcp (1) | 本地配额账本；分页突破 25k 行；Agent 的写操作先生成 plan，由人确认后 apply；保留审计日志 |
| 推送 | jtprogru/indexnow (2) | `urls-from: git diff`，只推送本次变更的 URL |
| GEO 评估 | aiseo-audit (8)、geo-opt (0)、optifeed-radar (5) | 每个发现项标注证据等级；geo-opt 用一致性测试套件保证 JS 与 Python 两份实现结果一致（**正是本方案"规则双实现"的先例**） |
| 代码 + 数据 + LLM | serpIQ (34)、siteprobe (2) | 先读代码库理解产品；检测 → 修复 → 复验，带快照回滚 |

### 1.3 要避免的共性问题
- **过时或缺乏依据的规则**：硬性 Title/Description 长度、sitemap `priority/changefreq`、把 FAQ/HowTo 当作富结果。Capgo 的 1,230 条规则大多是同一阈值的复制。
- **只报存量问题**：只说"现在有什么问题"，不说"这次改动新增了什么问题"。
- **日志工具的爬虫验证普遍做错**：
  - aeo-crawl-radar 用反向 DNS 验证 OpenAI、Anthropic、Perplexity 的爬虫，但这些厂商只发布 IP JSON，结果真实爬虫被判为伪造。
  - Google-Extended、Applebot-Extended 只是 robots.txt 令牌，从不出现在请求 UA 中，却被当作 UA 统计。

### 1.4 无人做好的空白（即 aieo-cli 的切入点）
1. **部署前检查源码配置**：`next.config`、`vercel.json`、`_redirects`、`_headers`、middleware、`astro.config` 与 sitemap、canonical 是否一致。
2. **构建产物层的 GEO 一致性**：
   - llms.txt 中的链接是否都指向可索引的规范页；
   - `.md` 替代版本与 HTML 是否一致；
   - robots.txt 的 AI 爬虫策略与 CDN/WAF 规则是否矛盾。
3. **按 URL 的 AI 爬虫证据**：用官方 IP JSON 验证，按训练/搜索/用户触发三类区分，并接入 CI。
4. **单 URL 漏斗**：AI 爬取 → AI 引荐访问 → GSC/Bing 点击。
5. **合并报告**：SaaS 外部视角 × CLI 内部视角。市面上没有，是 aieo 独有的。

---

## 2. 现有资产盘点

| 资产 | 位置 | 用法 |
|---|---|---|
| 41 条就绪度规则（F01–F18 / G01–G15 / H01–H04），中英双语，带证据与修复步骤 | `aieo-v5/packages/ai-readiness/src/vendor/rules` | 抽出规则规格（元数据、文案、权重），判定逻辑由 Rust 移植，用 fixtures 对齐 |
| 报告模型 `ScanFindings` / `ReportView` | 同上 `types.ts`、`report.ts` | 证据包中 findings 字段与之兼容，SaaS 不必新写渲染器 |
| 约 10 条技术审计规则（critical/warning/notice） | `aieo-v5/packages/crawler/src/crawl.ts` | 移植 |
| AI 爬虫分类表 | `ai-readiness/.../checks/declarations.ts:15` | 迁入规则规格，作为 SaaS 与 CLI 共用的爬虫目录 |
| GEOReady 多页规则 P01–P04、`/v1/scans` API、出站 webhook | `isaieoready/packages/scan`、`packages/api/v1` | P 系列在 `dist/` 场景天然成立；API 设计作为上传端点的范本 |
| llms.txt 校验器与错误码 | `llmstxtdb/oss/llmstxtdb-skills` | 错误码进入规则规格，由 Rust 实现 |
| IndexNow 推送与内容版本去重、GSC URL Inspection | `indexsubmit/packages/indexing` | 参考实现 |
| AI 流量识别规格 | `webcasa-cms/.../ai-traffic/spec.md` | 日志模块按此规格对齐，未来 HeoPress 内置 aieo-cli |
| API Key（`aieo_…`，按组织发放，分 read/write）、MCP 工具集 | `core/src/keys.ts`、`packages/mcp` | 用于 CLI 鉴权，MCP 工具命名与之对齐 |
| 计划中的 `page_changes` 表与 IndexNow 提交表 | `docs/plan-phase2-features.md` | 由证据包中的部署元数据写入 |

**缺口**：aieo-v5 没有 CLI、没有 GitHub Action，**也没有接收外部结果的端点**。另外，GEOReady 与 aieo-v5 之间已有约 8 个规则文件分叉，抽规格前需先统一。

---

## 3. 产品定位与原则

```
          ┌──────────────── aieo.ee (SaaS, TS) ────────────────┐
          │ 外部视角：线上抓取、AI 可见度、排名、引用、竞品          │
          │ + 证据包导入 → 内外合并、优先级排序、Agent 建议、告警     │
          └───────────▲──────────────────────▲────────────────┘
                      │ aieo push (API Key)   │ 网页拖拽上传
                      └──────────┬────────────┘
                     .aieo/bundle.json.gz（aieo-bundle/v1）
┌────────────────────────────────┴─────────────────────────────────┐
│ aieo-cli (Rust 单二进制, MIT)：本地 / CI / 服务器                    │
│ ① 构建产物  ② 源码与框架配置  ③ 服务器/CDN 日志  ④ GSC/Bing/IndexNow │
│ ⑤ 部署前后 Diff                                                    │
└──────────────────────────────────────────────────────────────────┘
              ▲ 规则规格 + fixtures（@aieo/rules，aieo-v5 内维护）
```

1. **免登录即可使用**：检查、日志分析、CI 门禁全部可以离线运行；连接 aieo.ee 是增值，不是前提。
2. **原始数据不出机器**：日志原始行、IP、源码留在本地，证据包只含聚合结果和发现项；IP 默认加盐哈希。
3. **只报确定性事实**：CLI 不调用 LLM；解释和修复建议交给 aieo.ee 的 Agent 或用户自己的 Coding Agent（通过 MCP）。
4. **单一规则规格**：同一规则 ID、文案、权重和 fixtures；TS 与 Rust 两份实现由一致性测试锁定。
5. **商业边界**：免费 CLI 覆盖"单站、本地、当次"；aieo.ee 负责"多站、历史、外部数据、关联分析、告警、团队"。

---

## 4. 规则共享：规格 + 一致性测试

在 aieo-v5 中新建 `packages/rules`（发布为 `@aieo/rules`，同时在 Git 中打 tag 供 Rust 侧拉取）。

```
packages/rules/
├─ spec/
│  ├─ rules/F03.yaml …         # id, class(scoring|bonus|info), weight, tier(error|warning|notice),
│  │                           # modes(url|dist|config|logs), title/description/fix {en,zh},
│  │                           # rationale + source_url（官方依据或标注 heuristic）
│  ├─ crawlers.yaml            # Bot 目录：UA 匹配, operator, purpose, robots_token_only,
│  │                           # verify: {ip_json[], fcrdns_suffix[]}
│  ├─ ai-referrers.yaml        # chatgpt.com, perplexity.ai, utm_source=chatgpt.com …
│  └─ bundle.schema.json       # aieo-bundle/v1 的 JSON Schema
├─ fixtures/<rule>/<case>/     # input/(html, robots.txt, sitemap.xml, headers.json …) + expected.json
└─ src/                        # TS：加载 spec、zod 类型（由 JSON Schema 生成）
```

- `packages/ai-readiness` 改为从 spec 读取元数据，TS 判定逻辑保持不变。
- Rust 侧在 `build.rs` 中把 spec 嵌入二进制（`include_str!`），类型由 JSON Schema 生成（`typify`）。
- **一致性测试**：TS runner 与 Rust runner 跑同一套 fixtures，对比 `status/ratio/evidence keys`。两边 CI 都跑；规格版本号（如 `rules/2.0.0`）写入证据包。
- **适用模式**：每条规则声明 `modes`。例如 F01 HTTPS/跳转在 `dist` 模式下为 `not_applicable`，提供 `--preview-url` 时才执行。

规则族：

| 前缀 | 来源 | 典型规则 |
|---|---|---|
| F01–F18 / H | 现有 aieo-v5 规则 | robots 对 AI 爬虫的声明矩阵、noindex/nosnippet、canonical、hreflang、JSON-LD、llms.txt、无 JS 时是否有内容 |
| P01–P04 | GEOReady 多页规则 | 多页元数据一致、跨页实体一致 |
| T | 技术审计 | 空/重复 title、H1、alt、薄内容（**不设硬长度阈值**） |
| **C** | 新增：跨产物契约 | sitemap URL 必须 200、可索引且自引用 canonical；noindex 页面不得出现在 sitemap；hreflang 互指；llms.txt 链接必须指向可索引的规范页；孤立页；`.md` 替代版本必须存在且一致 |
| **K** | 新增：配置 | `trailingSlash` 与 canonical、sitemap 一致；重定向链/循环；`_headers`/middleware 是否给 HTML 加了 `X-Robots-Tag`；`site`/`basePath` 与 canonical 域名一致 |
| **D** | 新增：回归 | 可索引 URL 数下降、页面消失却没有跳转、结构化数据丢失、内链数骤降 |
| **L** | 新增：日志 | AI 搜索爬虫覆盖率过低、Bot 遇到 5xx 或 robots.txt 抓取失败、部署窗口 404 尖峰、伪装 Bot、被 Disallow 的路径仍被抓取 |

分档与 CI 行为：**error**（确定性故障，默认阻断）、**warning**（高风险，新增时阻断）、**notice**（建议，不阻断）。

---

## 5. 功能模块

### 5.1 `check`：构建产物与配置
- **输入**：
  - `dist` 目录，自动识别框架；
  - 可选 `--preview-url`，配合 `--public-origin` 把预览地址映射到正式域名；
  - 框架配置文件和构建清单。
- **配置解析策略**：优先读构建后的**已解析清单**；JS/TS 配置文件只用 `oxc` 静态解析字面量，解析不出来时标 `unverified`，不做猜测。

| 预设 | 产物目录 | 优先读取 | 源文件回指 |
|---|---|---|---|
| astro / starlight | `dist/` | `astro.config.*`（oxc）、Starlight `locales/sidebar`、`_redirects`/`_headers`（CF Pages） | `src/content/docs/**`、`src/pages/**` |
| next（static/hybrid） | `out/`、`.next/` | `.next/routes-manifest.json`（已解析的 redirects/headers/rewrites）、`prerender-manifest.json`、`.vercel/output/config.json` | `app/**/page.*`、`pages/**` |
| nuxt / vue（Vite） | `.output/public`、`dist/` | `.output/nitro.json`、`nuxt.config`（oxc）；纯 Vite SPA 时提示"无 JS 无内容"风险 | `pages/**`、`src/views/**` |
| tanstack（Start/Router） | `.output/`、`dist/` | 预渲染清单、`app.config`/`vite.config`（oxc） | `src/routes/**` |
| hugo | `public/` | `hugo.toml/yaml/json`（原生解析） | `content/**` |
| vitepress | `.vitepress/dist` | `.vitepress/config.*`（oxc） | `docs/**/*.md` |
| docusaurus | `build/` | `docusaurus.config.*`（oxc） | `docs/**`、`blog/**` |
| static（通用） | 任意目录 | `_redirects`、`_headers`、`vercel.json`、`robots.txt`、sitemap | 无 |

- **输出**：每个发现项包含规则 ID、证据（文件:行 或 URL）、回指的源文件、修复步骤（中英）、证据等级（`official` / `heuristic`）。
- **归并**：同一模板产生的大量发现项归并为一个"原因"，参考 nurkamol 的做法。
- **SPA 限制**：Rust 不内置浏览器，CSR 页面只能报"原始 HTML 缺内容"；渲染对比交给 aieo.ee 的线上扫描。

### 5.2 `diff`：回归门禁
- `aieo snapshot` 保存 `.aieo/baseline.json`，内容包括：URL 清单、可索引状态、canonical、hreflang、结构化数据类型、内链图摘要、字数、内容哈希。
- `aieo diff --baseline … --fail-on new:error,new:warning` 报告新增、已修复和 D 系列损失。
- 退出码：`0` 通过；`1` 门禁失败；`2` 证据不完整或运行错误（fail-closed）。
- 豁免：`aieo.toml` 中 `[[suppress]] rule, url_glob, reason, owner, expires`，过期自动失效；不接受过宽的模式。

### 5.3 `logs`：爬虫与 AI 引荐证据
- **输入**（v1 全部支持，自动识别格式，`.gz` 流式读取）：
  - Nginx/Apache combined，以及自定义 `log_format`；
  - Caddy JSON；
  - Cloudflare：Logpush JSONL 文件，以及 AI Crawl Control GraphQL API（`--cf-zone`，token 由用户提供；部分字段需要付费套餐，缺失时降级）；
  - Vercel Log Drain NDJSON；
  - CloudFront 标准日志。ALB 没有 referrer 字段，文档中会说明。
- **分类**：training / search_index / user_fetch / classic_search / agent / human / unknown。robots-only 令牌只用于策略分析，不计入流量。
- **验证**：
  - 先用官方 IP JSON 验证（OpenAI ×3、Anthropic、Perplexity ×2、Google、Bing、Apple、Amazon、DuckAssist、CCBot），本地缓存并记录 `creationTime`；
  - Googlebot、Bingbot、Applebot、CCBot 不在列表中时，用 FCrDNS 兜底；
  - 结果：`verified` / `mismatch` / `unverifiable` / `unchecked`。厂商没有公布 IP 段时只标 `unverifiable`，**不判为伪造**。
- **报告**：
  - 覆盖率与"从未被抓取"页面（结合 sitemap 或 dist 的 URL 清单）；
  - 新鲜度：从 lastmod 或部署到首次被 AI 抓取的时间；
  - Bot 遇到的错误、部署窗口异常；
  - 伪装检测、robots 合规；
  - AI 引荐落地页（referrer 与 `utm_source`）。
- **存储**：`.aieo/logs.db`（SQLite），用 `hit_id` 去重，支持增量导入；IP 用本地盐值哈希。
- **每条命中的字段**：时间、host、路径、规范化 URL、状态码、字节数、缓存状态、edge/origin、IP 哈希、ASN、UA、actor_class、bot、operator、验证结果与列表版本、referrer、AI 平台、`deploy_id`。

### 5.4 `search`：GSC / Bing / IndexNow（本机凭据）
- **GSC**：
  - OAuth（loopback 或 device flow）或 service account；
  - Search Analytics 按 `startRow` 分页拉取；
  - URL Inspection 批量检查，本地配额账本限制为 2,000 次/日/站、600 次/分钟；
  - 两期对比。
- **Bing**：读取类 API；AI Performance 报告只能导出 CSV，用 `aieo search bing import-ai-csv` 导入。
- **IndexNow**：生成并校验 key 文件；`--from-git <ref>` 或 `--from-diff` 只提交变更 URL；每批最多 10,000 个；按响应码处理。
- **不做**：用 Google Indexing API 提交普通页面（违反官方限制）。
- **安全**：所有对外写操作默认 `--dry-run` 显示计划；MCP 模式下必须由人确认；写入 `.aieo/audit.jsonl`。

### 5.5 证据包与 aieo.ee 联动

**`aieo-bundle/v1`**（gzip JSON，Schema 在 `@aieo/rules/spec/bundle.schema.json`）：

```jsonc
{
  "schema": "aieo-bundle/v1",
  "producer": { "cli": "0.4.0", "rules": "rules/2.0.0" },
  "site": { "origin": "https://ubuntu.fan", "project_id": "…?", "framework": "starlight" },
  "run": { "kind": ["check","diff","logs","search"], "commit": "…", "branch": "…", "deploy_id": "…", "at": "…" },
  "inventory": [{ "url": "…", "indexable": true, "canonical": "…", "hreflang": {…}, "source": "src/content/docs/…", "hash": "…" }],
  "findings": [/* 兼容 ScanFindings：id, status, ratio, tier, evidence[], source_location, evidence_level */],
  "diff": { "added": [], "fixed": [], "losses": [] },
  "logs": { "window": {…}, "per_url_actor_day": [/* url, actor, bot, day, hits, status_buckets, verified */],
            "referrals": [/* landing_url, platform, day, visits */], "anomalies": [] },
  "search": { "gsc": {/* 汇总，可选 */}, "indexnow": {/* 提交记录 */} },
  "privacy": { "ip": "salted-hash", "raw_logs_included": false },
  "integrity": { "sha256": "…" }
}
```

**aieo.ee 侧（aieo-v5 新增工作）**：
- 端点 `POST /api/v1/projects/:id/bundles`：需要 write 权限的 API Key，幂等键为 `commit + kinds`。
- 网页拖拽上传入口，使用同一校验器。
- 解析后写入 `cli_runs`、`crawler_daily`、`ai_referrals_daily`，并写入计划中的 `page_changes`。

**辅助决策的方式**（SaaS 侧逻辑）：
1. **内外对账**：dist 中可索引但线上扫描为 404/noindex，说明是部署、CDN 或缓存问题；线上存在但 dist 中没有，说明是遗留页或重定向缺失。
2. **优先级**：发现项 × 页面重要性（GSC 点击、AI 引荐、AI 爬虫访问频率）× 修复成本，替代单一分数。
3. **GEO 归因**：AI 爬虫覆盖率与 AI 引用/提及趋势并列展示，解释"没被引用"的原因，例如"根本没被 OAI-SearchBot 抓取过"。
4. **触发动作**：根据部署的变更 URL，自动对这些页面做线上复检，并按需提交 IndexNow。
5. **Agent 输入**：AI Growth Agent 和 MCP 的 `get_inside_view` 读取证据包，生成带证据的修复任务。

### 5.6 交付：CI 与 Agent
- **输出格式**：`pretty`、`json`、`sarif`（`serde-sarif`）、`github`（注解）、`markdown`、`junit`、`bundle`。
- **GitHub Action**：`web-casa/aieo-cli@v1`，一个 Action 完成 check、diff、可选 push 和 IndexNow；另提供 GitLab CI 模板。
- **MCP 与 Agent Skill**：`aieo mcp` 是基于 `rmcp` 的 stdio 服务，只读工具优先，写操作生成 plan；同时提供 `SKILL.md`。
- **其他命令**：`aieo explain <RULE_ID>` 输出证据与修复说明；`aieo rules list --mode dist`。
- **分发与增长**：各 Agent 平台的打包、上架、Skill 拆分、aieo.ee 远程 MCP 的 OAuth，以及用户、口碑、收录三条增长线，见独立方案 [`ecosystem-and-growth.md`](ecosystem-and-growth.md)。

---

## 6. 命令

```bash
aieo init                                     # 识别框架，生成 aieo.toml 与 .aieo/
aieo check [./dist] [--preset starlight] [--preview-url URL --public-origin URL]
aieo snapshot && aieo diff --baseline .aieo/baseline.json --fail-on new:error
aieo logs ingest <files…|--cf-zone ID|--vercel-drain FILE> [--format auto]
aieo logs report --since 30d [--sitemap dist/sitemap-index.xml]
aieo logs gaps --actor search_index
aieo logs referrals --since 30d
aieo search gsc login | pull --days 28 | inspect --from-sitemap
aieo search bing pull | import-ai-csv FILE
aieo indexnow keygen | verify | submit --from-git HEAD~1 [--dry-run]
aieo export -o .aieo/bundle.json.gz           # 生成证据包
aieo login && aieo push                       # 上传到 aieo.ee
aieo mcp | explain RULE | rules list
```

---

## 7. Rust 技术架构

```
aieo-cli/ (cargo workspace, MIT)
├─ crates/aieo-spec      # 嵌入 @aieo/rules 的 spec，并生成 bundle 类型（typify）
├─ crates/aieo-core      # Finding/Report/Bundle 模型、tier、退出码、豁免
├─ crates/aieo-site      # 站点模型：dist 遍历、HTML 解析（scraper/html5ever）、sitemap（quick-xml）、
│                        # robots（texting_robots，RFC 9309）、URL 规范化、内链图
├─ crates/aieo-presets   # 框架识别与配置解析（oxc_parser 静态提取、toml/yaml/json、Next/Vercel/Nitro 清单）
├─ crates/aieo-rules     # F/P/T/C/K/D 规则实现 + fixtures 一致性测试 runner
├─ crates/aieo-schema    # 包装 schemaorg-rs（JSON-LD/Microdata/RDFa）
├─ crates/aieo-logs      # 解析器（按格式）→ 分类 → 验证（ipnet + hickory-resolver）→ rusqlite → 聚合
├─ crates/aieo-search    # GSC/Bing/IndexNow（reqwest + rustls，OAuth），配额账本
├─ crates/aieo-report    # pretty/json/sarif/github/md/junit/bundle
├─ crates/aieo-cloud     # login/push、凭据存储（keyring）
├─ crates/aieo-mcp       # rmcp
└─ crates/aieo-cli       # clap 入口
```

- **并发**：rayon 并行解析 HTML 与日志；网络请求使用 tokio，并设置限速。
- **分发**：
  - `cargo-dist`，覆盖 GitHub Releases、Homebrew 和安装脚本；
  - npm 包 `@aieo/cli`，内含预编译二进制，可用 `npx aieo` 运行；
  - Docker 镜像，便于在服务器上分析日志。
- **测试**：
  - 规则使用 fixtures 一致性测试（与 TS 共用）；
  - 用自有站点的 `dist` 做快照测试；
  - 日志使用合成数据加脱敏后的真实样本。

---

## 8. 里程碑

所选 4 个模块都在 v1 范围内，按依赖关系分版本发布。以下是单人加 AI Coding 的粗估。

| 版本 | 内容 | 周期 | 验收 |
|---|---|---|---|
| **M0**（aieo-v5） | 统一 GEOReady 与 aieo-v5 的规则分叉；建 `@aieo/rules`（spec、fixtures、bundle schema）；ai-readiness 改为读取 spec | 1–2 周 | SaaS 测试全绿；fixtures 覆盖 F01–F18 |
| **0.1** | Rust 骨架；generic static 与 Astro/Starlight 预设；可离线执行的 F/T/C 规则；snapshot/diff；json/sarif/github 输出；GitHub Action；`export` 证据包 | 3–4 周 | 在 2–3 个自有 Starlight 站的 CI 中连续运行 2 周，无误报阻断；与 TS 的一致性测试全部通过 |
| **0.2** | K 配置规则；Next.js、Nuxt/Vue、TanStack、Hugo、VitePress、Docusaurus 预设；P 多页规则；schemaorg-rs 集成 | 3 周 | 每个框架有示例站快照测试 |
| **0.3** | logs：Nginx/Apache、Caddy → Cloudflare（Logpush 与 GraphQL）、Vercel、CloudFront；IP 验证；覆盖率、引荐、异常报告 | 3 周 | 对自有站真实日志产出可解释报告；验证结果抽样人工核对 |
| **0.4** | search：GSC、Bing、IndexNow；`login/push`；**aieo-v5 侧完成 bundles 端点、上传 UI 和"内部视角"面板** | 3 周 | 证据包在 aieo.ee 上完成内外对账与优先级排序 |
| **1.0** | MCP 与 SKILL；文档站（中英）；npm/Homebrew 分发；稳定的规则版本策略 | 2 周 | 公开发布 |

合计约 15–17 周。如需更早发布，可以只发 0.1（构建检查 + 回归门禁），它单独就有完整价值。

---

## 9. 风险与对策

| 风险 | 对策 |
|---|---|
| TS 与 Rust 双实现漂移 | 规格单一来源；fixtures 一致性测试在两边 CI 都要求通过；证据包记录规则版本，SaaS 发现版本不一致时提示升级 |
| Rust 解析不了 JS 配置文件 | 优先读构建清单（Next routes-manifest、Vercel output config、Nitro）；oxc 只提取字面量；解析失败标 `unverified`，不做猜测 |
| 误报导致团队关掉 CI | 默认只阻断 error 和新增 warning；每条规则要求有 `source_url`，否则标为 heuristic 且不得设为 error |
| 爬虫 IP 列表变化 | 带 TTL 的缓存加内置兜底快照；拿不到列表时标 `unverifiable` |
| 日志隐私 | IP 加盐哈希；证据包不含原始行；文档写明数据流向 |
| Cloudflare GraphQL 字段受套餐限制 | 能力探测后降级，在报告中注明缺失字段 |
| CLI 太强削弱 SaaS 付费意愿 | CLI 只覆盖单站、单次；历史、多站、外部数据关联、告警、团队协作放在 aieo.ee |

---

## 10. 已确认决策

| # | 问题 | 决定 |
|---|---|---|
| 1 | 实现语言 | **Rust 单二进制** |
| 2 | 代码组织 | 在 aieo-v5 中抽出 `@aieo/rules`；CLI 为独立仓库（`~/aeo-website/aieo-cli`）；CLI 导出的文件由 aieo.ee 识别并辅助决策 |
| 3 | 商业模式 | **MIT 开源 CLI + aieo.ee 增值** |
| 4 | MVP 模块 | 构建产物检查 + 回归 Diff、源码配置检查、AI 爬虫日志分析、GSC/Bing/IndexNow（全部纳入，按 0.1–0.4 分版本发布） |
| 5 | 规则同步 | **规格共享 + 一致性测试**（TS 与 Rust 各自实现判定逻辑） |
| 6 | 数据回传 | **版本化证据包 `aieo-bundle/v1` + API 推送 + 网页上传** |
| 7 | 日志来源 | Nginx/Apache、Caddy JSON、Cloudflare、Vercel/CloudFront（全部） |
| 8 | 框架预设 | Astro/Starlight、Next.js、Hugo/VitePress/Docusaurus、通用静态，以及 **Vue（Nuxt/Vite）/ TanStack** |

## 11. 下一步
1. M0：在 aieo-v5 中对齐 GEOReady 规则分叉，建立 `packages/rules` 的 spec、fixtures 和 bundle schema。
2. 在本目录初始化 cargo workspace，先打通 `aieo check ./dist --preset starlight` → json/sarif → `export`。
3. 选 1–2 个自有 Starlight 站作为长期验证样本。

---

## 12. 进度（2026-10-22）

### M0 已完成（aieo-v5，分支 `feat/aieo-rules`，工作区 `~/aieo-v5-rules`）
- 新包 `packages/rules`（`@aieo/rules` 2.0.0，MIT）：
  - `spec/rules.json`：79 条规则，含 F01–F18 / G01–G15 / H01–H04（由现有注册表导出，文本一字不差），以及新增的 T 12 条、C 21 条、D 8 条；
  - `spec/crawlers.json`：21 个爬虫，含官方 IP 列表、反向 DNS、用途分类；
  - `spec/ai-referrers.json`：AI 引荐来源；
  - `spec/bundle.schema.json`：`aieo-bundle/v1`；
  - 以上各有对应的 JSON Schema。
- 34 个一致性用例 `fixtures/conformance/*`：期望值由 TS 引擎生成，逐条人工复核。
- `@aieo/ai-readiness` 改为从规格读取：F 模块的元数据和 F03 爬虫矩阵都来自 spec；新增 `spec-sync.test.ts` 和 `conformance.test.ts`。全套 490 个测试通过，api / worker / crawler 类型检查通过。
- GEOReady 分叉核对：`8296e87..a94e853` 只是等价的线性扫描加固，没有判定变化，结论已写入 `VENDORED_FROM.md`。
- 5 个 Dockerfile 加入 `packages/rules`；CI 增加 `@aieo/rules` 测试；`export:cli` 脚本把 spec 同步到本仓库，带内容哈希校验。

### 0.1 已完成（本仓库）
- Rust workspace 共 5 个 crate，40 个测试全部通过，`clippy -D warnings` 干净。
- **一致性**：34 个用例 × 16 个模块，共 544 项比对，与 TS 引擎完全一致；测试确认能检出不一致。
- 命令：`init / check / snapshot / diff / export / rules / explain`。
- 输出：pretty / json / sarif / github / markdown / bundle(gz)；退出码 0/1/2，证据不完整时 fail-closed。
- 预设：static（含 `_headers` / `_redirects` / meta refresh）、astro、starlight（回指到源文件）。
- 交付：GitHub Action（`action.yml`）、CI 和 release 工作流。
- 实站试跑：webcasa-docs、centosfan、ols-extra/docs、endoflife、portcyou（11,403 页，2.5 秒）。根据试跑结果做了三处修正：
  - hreflang 互为备选的页面不再算重复标题，portcyou 的误报从 1,717 条降到 0；
  - robots.txt 缺失时 sitemap 同样算"未声明"；
  - 首页是 meta refresh 跳转页时给出说明，且相关发现最高只到 warning。

### 0.2 已完成
- **规格 `@aieo/rules` 2.1.0**：新增 P01–P04（权重 30/30/20/20，独立的 0–100 分层）、K 9 条、T 3 条（JSON-LD），共 95 条；`aieo-bundle/v1` 增加可选的 `readiness.multipage`。TS 侧规格测试和 ai-readiness 490 个测试仍全部通过。
- **托管规则**：除 `_redirects` / `_headers` 外，读取 `vercel.json`（含 `cleanUrls`、`trailingSlash`）、`netlify.toml`、Next.js `routes-manifest.json`（path-to-regexp 语法）、Nuxt `routeRules` 重定向；每条规则记录所在文件和行号；带请求条件（has/missing、Country=）的规则跳过并在说明里计数。已有文件优先于未强制的改写规则（与 Netlify / Cloudflare Pages 一致）。
- **服务端构建**：Next.js `.next/`（预渲染 HTML + `public/` + 清单）、Nuxt 和 TanStack Start 的服务端产物，其余路由按"请求时渲染"处理：链接到它们不算断链，HTML 不检查；首页由服务端渲染时首页模块记为 unverified。开发构建（无 `BUILD_ID`）直接报错并提示先 `next build`；中间件存在时给出说明。
- **预设**：next、nuxt、vue、tanstack、hugo、vitepress、docusaurus（加上 0.1 的 static、astro、starlight 共 10 个），都能回指到源文件；配置读取器支持嵌套键、布尔/数字字面量、方法写法和行号。
- **规则**：
  - K：站点来源、base 路径、尾斜杠与实际 URL 不符；重定向循环 / 链 / 目标不存在；响应头规则给 sitemap 页加 noindex；Next 静态导出忽略 redirects/headers/rewrites；客户端渲染空壳应答多个 URL。
  - P01–P04：从 GEOReady 移植，作用于所有可索引内容页；P01 对声明了 hreflang 的页面不比较 lang，canonical 按"是否解析到本页文件"判断。
  - JSON-LD：缺 schema.org `@context`；缺 Google 富媒体结果必需属性（Product、BreadcrumbList、Event、Recipe、JobPosting、LocalBusiness、SoftwareApplication、VideoObject、Review、AggregateRating、Dataset、Course、QAPage、DiscussionForumPosting、ProfilePage，2026-10 按官方文档核对）；已停止展示的类型（HowTo、FAQPage、SpecialAnnouncement、ClaimReview）。
- **示例站快照**：同一个 4 页站点分别用 9 种构建方式真实构建（Next export / server、Nuxt generate / SSR、Vue SPA、TanStack Start、Hugo、VitePress、Docusaurus），去掉资源文件后放在 `fixtures/presets/`，每个都有快照测试。
- **实站试跑**（只读）：
  - ubuntu.fan（Next export，1,351 页）：导航里的 `/docs/guide` 在 8 种语言的所有页面上都是断链（目录没有 index 页）；
  - debian.club（Next export，580 页）：`/en/administration/firewall` 等页面不存在，正文里还有指向 `.md` 文件的链接；
  - web.casa：SoftwareApplication 缺 `offers.price` 和评分，不具备富媒体结果资格；
  - eol.wiki、portcyou、almalinux.com.cn：全站使用 FAQPage 标记（Google 已于 2026 年 5 月停止展示）；
  - 试跑中修掉的误报：多语言站各语言首页 `/ko/` 被判为尾斜杠不符（现在要求同时与实际文件的 URL 形式不符）；SPA 的 `/* /index.html 200` 遮住真实 robots.txt / sitemap（现在已有文件优先）。
  - 性能：portcyou 11,403 页 3.2 秒（0.1 为 2.5 秒）。
- Rust 测试共 69 个，`clippy -D warnings` 干净。

### 0.3 已完成
- **规格 `@aieo/rules` 2.2.0**：新增 L 系列 7 条规则；爬虫目录增加 DuckDuckBot、YandexBot、Baiduspider、PetalBot、MistralAI-User（共 26 个，均不进 F03，aieo.ee 评分不变）；`aieo-bundle/v1` 的 `logs` 从占位改为正式结构（时间窗、来源、每个爬虫的验证与状态码、按 URL×爬虫×天的抓取、AI 引荐、覆盖率、异常）。
- **新 crate `aieo-logs`**：
  - 解析：Nginx/Apache combined（含 vhost、common）、自定义 `nginx:` / `apache:` 格式串、Caddy JSON、Cloudflare Logpush（5 种时间戳格式）、Vercel Drains（按 `requestId` 去重，跳过非请求行）、CloudFront 标准日志 v1（按 `#Fields` 解析、UA 解码、`000` 跳过）和 v2 JSON；`.gz` 流式读取；按文件自动识别格式。字段格式均按官方文档核对（2026-10）。
  - 分类：按规格里的爬虫目录匹配 UA（长令牌优先），其余自动化记为 unknown_bot；AI 引荐按 referrer 主机或 `utm_source`。
  - 验证：官方 IP 段（缓存一天，记录 `creationTime`；所有厂商都用 `{creationTime, prefixes}` 格式）+ FCrDNS（Googlebot、Bingbot、Applebot、CCBot、YandexBot、Baiduspider、PetalBot）；结果 verified / mismatch / unverifiable / unchecked；`--offline` 不联网。
  - 存储：`.aieo/logs.db`；文件按前 4 KiB 内容识别，增长的日志从上次位置续读、轮转副本不重复计数；爬虫请求存加盐 IP 哈希，普通访问只存每日计数，不存 IP 和浏览器 UA；自动写 `.aieo/.gitignore`。
  - Cloudflare GraphQL（`aieo logs pull cloudflare`）：从 `settings` 读套餐限制，按 `maxDuration` 分段，满页自动二分；状态码不能分组时按状态区间分别查询；采样计数以 `weight` 存储，重复拉取覆盖而不累加。token 只从环境变量读取。
- **命令**：`aieo logs ingest | report | gaps | referrals | pull cloudflare`；`aieo export` 在有日志时附带 `logs` 段和 L 发现。
- **L 规则**：爬虫多数请求被 401/403/429 拦截（WAF/CDN）、爬虫 5xx、robots.txt 返回 5xx/429、爬虫 404 激增、冒充爬虫、已验证爬虫抓取被禁止路径、AI 搜索爬虫覆盖率低。
- **验证**：合成两周日志（真实官方 IP 段 + 冒充者）端到端跑通，在线验证正确识别冒充的 Googlebot / OAI-SearchBot 和被 WAF 拦截的 GPTBot；导出的证据包通过 Schema 校验且不含任何 IP 和浏览器 UA。100 万行日志导入 6.3 秒，报告 0.9 秒。
- Rust 测试共 82 个，clippy 干净；版本号升到 0.3.0。
- **待验收**：本机没有真实访问日志，"对自有站真实日志产出可解释报告"还需要一份真实日志或 Cloudflare token。Cloudflare GraphQL 只用模拟服务器测试过，尚未连真实 API。

### 0.4 已完成
- **规格 `@aieo/rules` 2.3.0**：新增 S 系列 5 条（可索引页未被 Google 收录、Google 改选规范网址、线上与构建产物不一致、点击腰斩、IndexNow 密钥未提供）；证据包 `search` 段正式化（GSC 汇总/热门页面与查询/URL 检查结果、Bing 统计与 AI 引用、IndexNow 提交记录）。
- **新 crate `aieo-search`**：
  - Google：桌面应用回环授权（PKCE + state，用户自己的 OAuth 客户端，不内置客户端）、刷新令牌、服务账号 JWT（用已链接的 ring 签名，不引入有时序公告的 rsa crate）；Search Analytics 按 `startRow` 分页；URL 检查带本地配额账本（每资源每天 2,000、每分钟 600），优先检查最久没查过的 URL。
  - Bing：API key 读取查询、页面、抓取问题统计；AI Performance 总览 CSV 按 aieo.ee 现有导入器的同一套规则解析。
  - IndexNow：密钥生成（写进框架静态目录）、构建与线上双重校验、按基线比较页面哈希只提交新增/变更/删除的 URL，默认只出计划，`--yes` 才发送，所有请求写入 `.aieo/audit.jsonl`。
  - 凭据放在用户配置目录（仅本人可读），CI 用环境变量（`AIEO_API_KEY`、`BING_WEBMASTER_API_KEY`、`GOOGLE_APPLICATION_CREDENTIALS`）。
- **命令**：`aieo search google login|properties|pull|inspect`、`aieo search bing key|pull|import-ai`、`aieo search report`、`aieo indexnow keygen|verify|submit`、`aieo login`、`aieo push`；`aieo export` 自动附带 `search` 段和 S 发现。
- **aieo.ee 侧（aieo-v5，分支 `feat/cli-bundles`）**：
  - `POST /api/projects/:id/cli/bundles`（gzip 或 JSON，20 MB / 解压 100 MB 上限，Schema 校验，站点域名必须匹配项目，`commit + kinds` 幂等，单事务 + 项目级咨询锁，每组织每小时 60 次）；`GET …/cli/runs`、`GET …/cli/overview`。
  - 新表 `cli_runs`、`cli_pages`、`crawler_daily`、`ai_referrals_daily`；新页面"内部视角"（拖拽上传、最近一次运行、发现列表、爬虫活动图、AI 引荐），中英文案。
  - 42 项 API 测试、Web 测试、类型检查、生成文档检查全部通过。
- **端到端验证**：本地起 aieo.ee API（专用 fixtest 库），`aieo login` 自动找到匹配项目，`aieo push` 上传成功，概览接口返回页面清单、爬虫与引荐汇总；只读 key 被正确拒绝。
- 与方案的出入：上传端点用现有的 `/api/projects/...` 而不是 `/api/v1/...`（aieo.ee 目前没有版本化 API）；没有用系统钥匙串，凭据文件按 gh/gcloud 的做法存放；设备授权流程不支持 Search Console 权限，所以 CI 用服务账号。
- Rust 测试共 102 个，clippy 干净；版本号升到 0.4.0（1.0 升到 1.0.0）。
- **待验收**：Google / Bing / IndexNow 只用模拟响应测过，尚未用真实凭据跑通；aieo.ee 的"内部视角"页面只跑了组件测试，没有在浏览器里看过。

### 1.0 已完成
- `aieo mcp`：基于 rmcp 的 stdio MCP 服务，10 个只读工具；写操作只提供 `*_plan`。
- `SKILL.md`：Agent 修复循环，随 release 附带（1.1 起移到 `plugins/aieo/skills/aieo/`）。
- `--format junit` 与 GitLab 模板 `ci/gitlab-ci.yml`；Action 新增 `push`、`aieo-api-key`、`indexnow` 输入。
- `aieo.toml` 的 `[[hosting.rewrites]]` / `[[hosting.redirects]]`：描述构建看不到的路由（Next 中间件等）。在 aieo.ee 自身构建上，277 个断链误报降为 0。
- 规则版本策略：`@aieo/rules` 2.4.1 起附 `rules-history.json`；基线之后新增规则的发现记为 `unknown`，升级不会让 `diff` 失败。
- 发布：musl 静态 Linux、macOS 双架构、Windows；`install.sh` 校验 sha256。npm 与 Homebrew 的打包已就绪，但首发只走 GitHub Release（缺 token 时对应 job 跳过）。
- aieo.ee 文档站新增中英 CLI 文档（`/docs/cli`），规则参考页由 `@aieo/rules` 生成。
- 分发与增长另见 [`ecosystem-and-growth.md`](ecosystem-and-growth.md)。

### 1.1 已完成（Agent 分发，只依赖 GitHub）
- 仓库本身即插件市场：`plugins/aieo/` 同时供 Claude Code（`.claude-plugin/marketplace.json`）与 Codex（`.agents/plugins/marketplace.json`）安装，含 5 个技能与 MCP 服务；`claude plugin validate` 通过，两边均在隔离配置中实测安装，Claude Code 中 MCP 连接成功。
- 技能拆为 5 个：`aieo`（修复循环）、`aieo-ci`、`aieo-crawlers`、`aieo-indexing`、`aieo-llms-txt`；`npx skills add web-casa/aieo-cli` 可发现全部。
- MCP 工具均新增可选参数 `project`，一个服务可检查 monorepo 中的多个站点（MCP roots 已被 SEP-2577 弃用，未采用）。
- Release 新增：跨平台 MCP Bundle（`aieo-v<版本>.mcpb`，macOS 通用二进制 + Linux x86_64 + Windows x64，工具清单由 `aieo mcp` 生成）、`server.json`、`aieo-skills.zip`。
- MCP Registry（`io.github.web-casa/aieo`，GitHub OIDC）发布 job 已就绪，仓库变量 `MCP_REGISTRY_PUBLISH=true` 时才运行。

### 1.1.1：技能实测
- 用 `claude -p --plugin-dir` 在临时站点上跑真实会话（提示词不提 aieo，用户的其他 SEO 技能同时在场，对外操作由包装脚本拦截），5 个场景（修复、CI、爬虫日志、收录与 IndexNow、llms.txt）结果均正确，未出现越权操作。
- 修复两处：中文提问时技能未触发（描述加中文触发词）；Agent 误以为 `aieo logs ingest` 需用户执行（报错与 MCP 说明写明它只读本地文件、不外发）。

### 1.2 已完成
- CI 新增 `action-release`：在 Linux、macOS、Windows 上用最新 release 的二进制跑 Action，并确认门禁在 error 时失败；`install.sh` 支持 Windows（Git Bash）。
- 所有 MCP 工具的 `project` 参数都有测试；仓库同时是 Cursor 插件（`.cursor-plugin/`），新增 `glama.json`。
- aieo.ee 规则页面：`packages/rules/docs/{en,zh}/<ID>.md` 手写内容，`gen-cli-rules` 生成每条规则一页；首批 C、K 两类 31 条（中英 62 页）。没有内容的规则只进总表，不生成薄页面。
- 写规则文档时发现并修复 7 处检测问题：Hugo 配置键大小写导致缺行号；Next.js `metadataBase` 指向布局文件；`_redirects` 文件遮蔽（循环、链、目标缺失）；`! X-Robots-Tag`；`C-ROBOTS-BLOCKS-ALL` 按全站判断；`C-ROBOTS-AI-SEARCH-BLOCKED` 也报"只留首页"；hreflang 按 ISO 代码表校验。
- `@aieo/rules` 3.0.0（判断放宽属于 major）：`rules-history.json` 新增 `changed`；`aieo diff` 把基线之后新增或放宽的规则的发现记为 `unknown`，升级仍不会让 `diff` 失败。

### 1.2.1：其余规则页与随之发现的问题
- aieo.ee 规则页补齐 F、P、T、D、L、S 六类 56 条，CLI 运行的 87 条规则都有中英文页面；规则参考侧边栏按类别分组，计分模块显示权重。
- 写文档时发现并修复：`aieo diff` 未配置基线时不读 `.aieo/baseline.json`；`L-ROBOTS-TXT-UNAVAILABLE` 在有 base path 时看错了 robots.txt 位置；`L-404-SPIKE` 把日志开始之前的天数按 0 计入基线；Cloudflare 无法按状态码分组时 401、429 被并入其他区间；`T-MISSING-TITLE` 读到 SVG 的 `title`；meta `name` 大小写；`template` 里的 h1 被计数；`D-PAGE-REMOVED` 不认跨域重定向；`S-INDEXNOW-KEY-INVALID` 忽略 `key_location`；P04 证据文字。
- 同时发现、留待下一版的 F 模块问题（TS 与 Rust 两边一致，需改规格并升 `@aieo/rules`）：F10 无脚本短页判为部分达标；F04 漏 `googlebot` 写法；站点类型识别把 JSON-LD 类型转小写后再比较，`Product` 等永远匹配不上。

### 与方案的偏差
- 框架配置先用一个字面量词法扫描器读取，没有引入 oxc。只读 `site / base / outDir / trailingSlash` 的字面量，计算出来的值一律视为未知。0.2 扩展了这个扫描器（嵌套键、布尔、方法、行号），仍未引入 oxc：现有 9 种框架的示例配置都能读出需要的值。
- 没有采用 schemaorg-validate（"schemaorg-rs"）：它把 Article 的 headline/image/datePublished/author 当作必需属性，与 Google 文档"Article 没有必需属性"相矛盾，词表也不完整。改为自维护一张按官方文档核对过的必需属性表；Microdata / RDFa 暂不检查。
- 日志没有用 hickory-resolver，FCrDNS 走系统解析器（dns-lookup），足够且不引入异步运行时；IP 验证在导入时完成（之后不再保存原始 IP），换列表版本后旧记录不会重新验证。
- Cloudflare GraphQL 不提供 IP 和 AI 引荐（引荐维度仅付费套餐），只用于爬虫流量；需要逐条数据时用 Logpush。
- P01–P04 不进 aieo.ee 的 TS 引擎（它们来自 GEOReady 的付费层），规格里标为仅 Rust 实现，没有一致性测试。
- 证据包里没有 `integrity` 字段：跨语言的规范化哈希不划算，传输时用 gzip + HTTPS。
- F15 在构建模式下检查首页的 `.md` 副本，不受一致性测试锁定。
- `T-THIN-CONTENT`、`T-TITLE-LENGTH` 属于启发式规则，默认关闭。

### 待办
- aieo-v5 的 `feat/aieo-rules` 合并时，注意与主分支上未提交的 page-changes 改动在 `.github/workflows/ci.yml` 可能冲突。
- 已确认：GitHub 仓库为 `web-casa/aieo-cli`，两份 LICENSE 的版权人为 Web-Casa。
