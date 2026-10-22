# aieo 生态与增长方案：Agent Harness 融合（v0.1，2026-10-22）

> 本文是 `research-and-plan.md` 之外**新增**的独立方案。主方案管"做什么、怎么做"（规则、模块、里程碑）；本文管"怎么让人用上、说好、找得到"。
> 技术底座已经在主方案 5.6 / 1.0 中完成：`aieo mcp`（10 个只读工具）、`skills/aieo/SKILL.md`、npm / Homebrew / install.sh、GitHub Action、GitLab 模板。本文不再重复这些，只规划在此基础上的分发、运营与增长。

---

## 0. 补充还是新增：结论

**新增一份独立方案，在主方案 5.6 加一行链接。** 理由：

1. **对象不同。** 主方案的读者是写 Rust 和规则的人；本文的读者是做上架、文档、运营的人，节奏按平台审核周期走，不按版本号走。
2. **范围跨两个产品。** 增长的关键缺口在 aieo.ee（远程 MCP 的 OAuth），不在 CLI。放进 CLI 的工程方案里会被当成"CLI 的附属"。
3. **有外部依赖与合规事项。** 国内平台需要实名或企业认证、7 天左右审核，上架文案受 aieo.ee 产品事实红线约束。这些需要单独跟踪。
4. **技术上是补充。** 不需要新模块。需要的是：打包格式、上架清单、少量新 Skill、aieo.ee 的 OAuth 和两三个 MCP 工具。

---

## 1. 有赞做了什么（调研结论）

### 1.1 三种形态

| 形态 | 面向 | 接入位置 | 说明 |
|---|---|---|---|
| **有赞龙虾**（托管的 AI 数字员工） | 付费商家 | 飞书、钉钉、企微、个人微信、桌面端 | 2026-06-17 上线。能写内容、做主图和详情页、答数据问题，也能**改库存、改价、上下架**（写操作）。没有公开工具清单。 |
| **有赞连接器** | 付费商家 | WorkBuddy、豆包工作、千问办公 | 2026-09 见报。16 个技能分 5 组：商品与供应链、交易与履约、会员与导购、营销与增长、店铺与企助。三家平台上都没找到公开的上架页。 |
| **CLI + Skill** | 开发者 / ISV | Codex 插件市场（`youzan/youzanyun-toolkit`）、npm `youzan-lite-cli` | toolkit 面向有赞云开发（脚手架、日志、RDS、API 权限申请），0 star。lite-cli 只读（店铺、会员、订单），每月 14 次下载，是否官方未证实。 |

官方口径是"超过 8000 个开放 API 变成 AI 可直接调用的 CLI 工具和完整的 Skill 体系"，但公开可见的只有约 3 个业务域、只读。**没有 MCP 服务**，ClawHub、skills.sh、MCP 目录里也没有官方条目；ClawHub 上的两个有赞 Skill 是第三方做的（OOMOL 的凭据代理、个人开发者的说明型 Skill）。

来源：[官方发布文章](https://www.youzan.com/blog/articles/77149)、[龙虾权益公告](https://bbs.youzan.com/thread-800239-1-1.html)、[连接器报道](https://finance.sina.com.cn/roll/2026-09-28/doc-initkhxe2900887.shtml)、[youzanyun-toolkit](https://github.com/youzan/youzanyun-toolkit)、[youzan-lite SKILL.md](https://unpkg.com/youzan-lite-cli@1.1.37/skills/youzan-lite/SKILL.md)。

### 1.2 商业模式

- 龙虾不单独收费，扣共享"算力"额度；每个套餐含 3 个渠道，额外渠道 ¥1,188/个/年；算力包 ¥4,000/4 万、¥12,000/12 万。基础版和 06-17 前购买的商家在上线时不能用。
- 2025 年约 18,220 家商家用过有赞智能体；2026 上半年 2 万多家在用有赞 AI 产品（付费商家 51,633 家）。龙虾单独的使用数据没有公布。
- 推广靠发布会、港股上市叙事（2026 年 AI 直接收入目标过亿）、媒体通稿和大量 SEO 教程页，**不靠技能/MCP 目录**。

### 1.3 对我们的启示

**照着做：**

1. **去用户已经在聊天的地方。** 有赞把重心放在办公和 IM 类 Agent（WorkBuddy、豆包、千问、飞书），而不是写代码的 Agent。我们的营销/站长用户同理。
2. **分层：API → CLI → Skill。** Skill 让 Agent 先跑 `--help`、`schema`、`auth status` 再动手；原子数据调用与分析分开。这和 `aieo` 的 SKILL.md 思路一致。
3. **写操作显式闸门。** 读随便跑，写要过 write gate 并经用户确认。我们已经做到（plan-first + `--yes`），应该把它**说出来**当卖点。
4. **按业务分组的多个技能**，而不是一个大技能。
5. **给开发者单独一套**（toolkit），教 Agent 怎么在平台上开发。

**避开：**

1. **口号大于产物。** "8000 API" 没有对应的公开清单。我们只说能核对的事：规则数、规则 ID、开源代码、可复现的检查。
2. **分发碎片化，目录空白让第三方占了。** 我们要自己先把官方条目占住（MCP Registry、ClawHub、skills.sh、Glama、Smithery）。
3. **门槛挡人。** 有赞上线时把老客户和基础版挡在外面。我们的 CLI 无需账号；aieo.ee 侧用"未订阅时可用"的功能承接。
4. **效果数据前后不一致**（同一篇稿子里 GMV 写了"突破 2.3 亿"又写"近 3 亿"）。我们公开的任何数字都要能从数据重算。

---

## 2. 定位：两类用户 × 两种接入

| | **本地接入**（stdio MCP + SKILL.md） | **远程接入**（HTTP MCP + OAuth） |
|---|---|---|
| **开发者 / 建站者**（改源码的人） | **aieo-cli**：Claude Code、Codex、Cursor、OpenClaw、Hermes、Kimi、Trae、WorkBuddy（技能）。已具备。 | aieo.ee 远程 MCP 中的 `get_inside_view`：CI 推上去的证据包，在任何 Agent 里都能读。 |
| **营销 / 站长 / 内容团队**（不碰构建的人） | 不适用 | **aieo.ee 远程 MCP**：claude.ai 连接器、ChatGPT/Codex 应用、豆包工作自定义连接器、WorkBuddy 连接器、千问办公。**缺 OAuth，这是最大缺口。** |

结论：**CLI 已经能覆盖开发者 Agent；办公类 Agent 和各大连接器目录都要求远程 + OAuth**（claude.ai 目录只收远程 OAuth；OpenAI 拒收 API Key 认证；豆包工作只支持远程 HTTP 自定义连接器）。aieo.ee 的 `/mcp` 目前是 Bearer Key，需要补 OAuth 2.1。

---

## 3. 要造的东西

### 3.1 一个"枢纽"仓库：一次编写，多处上架

放在 `web-casa/aieo-cli` 里（仓库公开后），不另建仓库，避免两边版本不同步：

| 文件 | 给谁 | 备注 |
|---|---|---|
| `skills/*/SKILL.md` | Claude Code、OpenClaw/ClawHub、Hermes、skills.sh、Kimi | 已有 `aieo`，按 3.2 扩充 |
| `.claude-plugin/marketplace.json` + `plugin.json` | Claude Code 插件市场；Claude 目录插件 | 插件内含 Skill + `npx -y @aieo/cli mcp -C .` |
| `.codex-plugin/` | Codex 插件市场 | 同有赞 toolkit 的安装方式 |
| `.cursor-plugin/` | Cursor 市场 | 要求开源，需仓库公开 |
| `server.json` | 官方 MCP Registry | 名称 `ee.aieo/cli`，通过 aieo.ee 的 DNS 验证所有权 |
| `glama.json` | Glama | |
| MCPB 包（发布时构建） | Smithery、Claude Desktop | 内含对应平台的二进制 |
| WorkBuddy 包（发布时构建） | WorkBuddy 开放平台 | 平台需要 `description_zh`/`description_en`/`version`/`author`。**不改通用 SKILL.md 的 frontmatter**，发布时生成带这些字段的 zip，避免其他平台的校验器报未知字段 |
| `listings/*.md` | 所有目录的上架文案（中英） | 单一事实来源；CI 用 aieo.ee 的 `check-site-config` 禁用词正则检查 |

aieo.ee 侧：

- `https://aieo.ee/.well-known/skills/index.json`：Hermes 等会自动发现。
- `https://aieo.ee/llms.txt` 中加入 CLI 文档与 Skill 入口。

`release.yml` 增加：构建 MCPB 和 WorkBuddy 包作为 release 附件；`server.json` 的版本号随 tag 更新并发布到 MCP Registry。

> 进度（1.1）：Claude Code / Codex 插件市场、5 个技能、MCPB、`server.json` 已完成；Registry 名称改用 `io.github.web-casa/aieo`（GitHub OIDC，无需 DNS），由仓库变量控制是否发布。Cursor、Glama、WorkBuddy 包与 `/.well-known/skills/index.json` 尚未做。

### 3.2 技能拆分：一个技能只做一件事

参考有赞"16 技能 5 组"和 claude-seo 一类合集的传播方式（合集比单个技能更容易被收藏、转发）。控制在 6 个以内，每个的 `description` 写清触发场景：

| Skill | 做什么 | 依赖 |
|---|---|---|
| `aieo`（已有） | 检查构建 → 改源码 → 复查的修复循环 | CLI |
| `aieo-ci` | 给项目加 GitHub Action / GitLab 模板，设置基线和 `diff` 门禁 | CLI |
| `aieo-crawlers` | 导入访问日志，回答"AI 爬虫有没有来、被谁挡了" | CLI |
| `aieo-indexing` | GSC / Bing / IndexNow：查未收录原因、生成提交计划（只出计划，执行交给用户） | CLI + 用户凭据 |
| `aieo-llms-txt` | 生成和校验 llms.txt，并引用 llmstxtdb 的同类站点示例 | CLI |
| `aieo-report` | 面向营销人员：读 aieo.ee 的 AI 就绪度报告和 Inside 视图，写成可执行的修复清单 | 远程 MCP |

### 3.3 aieo.ee 远程 MCP 升级

1. **OAuth 2.1**（保留现有 Bearer Key 给脚本用）：支持动态客户端注册，满足 claude.ai 目录、ChatGPT/Codex、Smithery URL 上架要求；scope 默认只读。
2. **新工具**：
   - `get_inside_view`：读取 CLI 推送的证据包（构建页面、爬虫日、AI 引荐）。主方案 5.5 已预留。
   - 把**未订阅时可用**的三项能力（技术审计、GSC/Bing 导入、AI 可读性检查）暴露为工具。登录即可用，作为办公类 Agent 里的入口。
3. **所有工具标注 `readOnlyHint` / `destructiveHint`**；写操作沿用 plan-first。
4. 文档页：在豆包工作、WorkBuddy、千问办公、claude.ai、ChatGPT 中添加 aieo 连接器的分步教程（中英）。豆包工作没有公开上架流程，**教程页本身就是渠道**。

---

## 4. 增长三条线

### 4.1 扩展用户

**漏斗：** Agent 里装上（无需账号） → 本地 `aieo check` 有结果 → CI 门禁常驻 → `aieo push` 到 aieo.ee 看 Inside 视图 → 订阅。

1. **先占目录**（按投入产出排序，详见第 6 节清单）：MCP Registry、skills.sh、ClawHub、Glama/Smithery/mcp.so、Claude Code 插件市场、WorkBuddy、魔搭。
2. **框架生态入口**：Astro integrations 目录、Starlight 社区插件列表、Docusaurus / VitePress / Hugo 的社区工具列表、awesome-mcp-servers、awesome-claude-skills。每个都以"该框架的预设"切入，而不是泛泛的 SEO 工具。
3. **CI 模板即分发**：GitHub Action 上架 Marketplace；GitLab 模板提交到社区模板集合。门禁一旦常驻 CI，就是持续使用。
4. **CLI 内的引导要克制**：不加广告行。只在用户真正需要 aieo.ee 能力时给出下一步（例如 `aieo push` 的计划输出、`L-*` 规则里"在 aieo.ee 看长期趋势"的说明链接），链接带 `utm_source=aieo-cli`。
5. **CLI 不加遥测。** 用分发渠道统计与 aieo.ee 服务端数据衡量（第 7 节）。这本身也是口碑点。

### 4.2 扩展口碑

1. **把安全讲出来**：ClawHub 恶意技能事件、Snyk ToxicSkills 之后，"只读、不调用 LLM、写操作先出计划再 `--yes`、npm provenance、校验和"是差异点。在 README、上架文案和文档放一页"安全模型"。
2. **可分享的结果**：
   - README 徽章（门禁通过 / AI 就绪度分数），由 aieo.ee 提供徽章端点。徽章即外链和曝光。
   - 用户自愿公开的报告页（GEOReady 承载公开检查报告），只在站点所有者授权后公开。
3. **修上游默认模板**（最有说服力的一招）：用 `aieo check` 跑主流框架的官方 starter/默认模板，把发现的真实缺陷（sitemap 与 canonical 不一致、llms.txt 缺失、尾斜杠重定向等）以 issue/PR 提交上游，并在 aieo.ee 发布一份可复现的"框架默认模板 AI 就绪度"对比，附命令和版本号。每个被合并的 PR 都是一次有据可查的口碑。
4. **自家案例**：aieo.ee 自己的构建跑出 277 个误报断链，原因是 Next 中间件把无前缀路径改写到 `/en`，加 `[[hosting.rewrites]]` 后归零、就绪度 88.0。写成案例，说明"为什么要读托管配置"。
5. **规则开放共建**：`@aieo/rules` 是公开规格；规则提案走 GitHub issue 模板，被采纳的规则在 `rules-history.json` 和文档里署名贡献者。

### 4.3 带来收录

"收录"有两层，都要做：

**（a）aieo 自己被收录**：被搜索引擎收录，被 AI 回答引用，被目录列出。

1. **每条规则一页**：现在 `cli/rules.mdx` 是一张总表。改为由 `gen-cli-rules` 生成总表 + 107 个规则页（中英），每页包括：含义、为什么重要（附官方出处链接）、各框架里怎么改、`aieo explain` 输出。规则 ID 会出现在用户的 CI 日志里，用户会拿它去搜，规则页就是落点。这是长尾搜索与 AI 引用的主力页面。
2. **框架专题页**："Astro/Starlight 的 sitemap、canonical、hreflang 一致性""Next.js 中间件改写与断链误报"等。内容来自真实规则与预设，不是模板拼接。
3. **为 LLM 写的文档**：`llms.txt` 指向 CLI 文档；每页开头一段可直接引用的结论；命令和输出用代码块。
4. **结构化实体一致**：aieo.ee、GitHub、npm、各目录的名称、描述、作者一致，`Organization.sameAs` 列出这些官方条目。
5. **目录条目本身**：MCP Registry、Glama 等页面会被搜索引擎和 AI 抓取，等同于高质量引用来源。

**（b）帮用户被收录**：这是产品价值，也是增长故事。

1. 闭环：`aieo check` 修复 → 部署 → `aieo indexnow submit` / indexsubmit 自动提交 → `aieo search google inspect` → `S-NOT-INDEXED` 消失。文档用一个真实站点演示全过程。
2. aieo.ee 在用户授权且数据足够时，发布匿名聚合统计（例如修复后到被收录的天数分布）。**没有数据就不发数字**（对照有赞的反面教训）。

---

## 5. 产品组合协同

| 产品 | 在增长中的角色 | 与 aieo 的衔接 |
|---|---|---|
| **GEOReady**（geoready.org） | 公开检查与报告：最宽的入口 | 报告里线上检查不到的问题（构建、配置、日志）引导到 CLI；规则来自同一份 `@aieo/rules` |
| **llmstxtdb** | Agent 可用的 llms.txt 索引 | `aieo-llms-txt` Skill 引用其同类示例；aieo 校验通过的站点可提交收录 |
| **indexsubmit** | GSC + IndexNow 托管自动化 | 边界：aieo 是本地、先出计划；indexsubmit 是托管、定时执行。CLI 文档里给出"要定时自动提交用 indexsubmit" |
| **drank** | DR 面板 | 跟踪上架与上游 PR 带来的外链和 DR 变化，作为 4.2/4.3 的度量 |
| **geo-tools** | 单点工具集合 | 每个工具页给出"在构建阶段自动检查"的 CLI 命令 |

交叉链接只放在上下文相关的位置，避免整站互链显得像链接网络。

---

## 6. 上架清单

| 平台 | 形式 | 前置条件 | 优先级 |
|---|---|---|---|
| 官方 MCP Registry | `server.json`（`ee.aieo/cli`，之后 `ee.aieo/remote`） | aieo.ee DNS 验证 | G0 |
| skills.sh | GitHub 仓库中的 SKILL.md | 仓库公开 | G0 |
| ClawHub（同时覆盖腾讯 QClaw、火山 ArkClaw） | `clawhub skill publish` | 账号 | G0 |
| Glama / Smithery / mcp.so | `glama.json` / MCPB / 表单 | 仓库公开 | G0 |
| Claude Code 插件市场 | `.claude-plugin/marketplace.json` | 仓库公开 | G0 |
| GitHub Actions Marketplace | `action.yml` | 仓库公开 | G0 |
| WorkBuddy 手动导入教程 | 文档页：下载 Skill zip，在 WorkBuddy 中导入 | 无；WorkBuddy 是否支持本地导入**待核实**，不支持则删去此行 | G0 |
| WorkBuddy 开放平台 | Skill zip（中英描述） | **实名或企业认证**，约 7 天审核 | 主体确定后 |
| 魔搭 MCP 广场 | 表单 | 很可能要实名（未核实） | 主体确定后 |
| Hermes | `/.well-known/skills/index.json`；MCP 目录 PR | aieo.ee 部署 | G1 |
| claude.ai 连接器目录 | 远程 MCP | **OAuth** | G1 |
| 豆包工作 / 千问办公 / WorkBuddy 连接器 | 远程 MCP + 教程页 | **OAuth**；豆包无公开上架流程 | G1 |
| Cursor 市场 | `.cursor-plugin/` | 开源 | G1 |
| ChatGPT / Codex 应用 | 远程 MCP | **OAuth**（不收 API Key） | G2 |
| 扣子技能商店 | 按其规范打包 | 规则严格；可付费；很可能要实名（未核实） | 主体确定后，视需求 |
| 腾讯云 MCP 广场 | — | 不对第三方开放 | 不做 |

### 6.1 国内平台的实名认证

**为什么要实名。** 国内平台把审核责任落到开发者身上：

- 法规：《网络安全法》第二十四条（信息发布类服务要求真实身份）、《互联网用户账号信息管理规定》（2022）、生成式 AI 与算法推荐相关规定（平台对接入的第三方能力负责）、《个人信息保护法》（技能能读到用户办公数据，出事要能指认处理者）。
- 平台：安全追责（ClawHub 恶意技能事件之后尤其如此；WorkBuddy 约 3235 万月活、豆包约 4.19 亿）、付费商店的开票与结算、配合审核期压制批量垃圾上架。

**哪些渠道需要。** 只有"上架国内平台官方商店"这一步需要。ClawHub（同时覆盖腾讯 QClaw、火山 ArkClaw）、skills.sh、MCP Registry、Glama、Smithery、Claude 插件市场不需要；豆包工作的自定义连接器由用户自己填写 aieo.ee 地址，我们这边不需要。

**主体问题。** 个人认证要中国大陆身份证，企业认证要大陆营业执照。Web-Casa 若不是大陆公司，企业认证可能走不通，只能由大陆个人或合作方做；主体确定后很难更换。

**做法。** 实名不挡 1.0：G0 先上不需要实名的渠道，并发布 WorkBuddy 手动导入、豆包工作自定义连接器的教程（后者在 G1 OAuth 上线后可用）；主体确定后再提交 WorkBuddy 开放平台、魔搭和扣子。

---

## 7. 衡量指标

CLI 不做遥测，用以下来源：

| 线 | 指标 | 来源 |
|---|---|---|
| 用户 | 安装量 | npm 下载、GitHub release 下载、Homebrew analytics、skills.sh / ClawHub 安装数 |
| 用户 | 常驻使用 | GitHub Action 的 dependents、aieo.ee 收到的证据包数和项目数 |
| 用户 | 转化 | 带 `utm_source=aieo-cli` / `mcp` 的注册数；远程 MCP 的活跃连接数；其后的订阅 |
| 口碑 | 认可 | GitHub star、上游 PR 合并数、徽章端点的请求量、外部提及 |
| 收录 | 被收录 | aieo.ee 文档在 GSC 的收录页数和展示量（规则页单独分组） |
| 收录 | 被引用 | 用 aieo.ee 自己的 AI 引用监测跟踪 CLI 文档和规则页 |
| 收录 | 被列出 | 已上线的目录条目数（第 6 节） |

---

## 8. 合规与红线

1. **aieo.ee 产品事实**：所有上架文案、教程和文档都不得出现"免费套餐 / 免费版 / Free plan / $0/月"；可用"开源 CLI，无需账号"和"未订阅时可用"（仅限技术审计、GSC/Bing 导入、AI 可读性检查）。不出现定价加成类用词。GEO 不写成定时任务；引用与检索的措辞按 `product-facts.md`。`listings/` 进 CI 检查（3.1）。
2. **写操作**：所有 Skill 和 MCP 工具只读；外部写入（IndexNow 提交、push、登录）只给计划，由用户执行。远程 MCP 的 OAuth scope 默认只读。
3. **凭据**：Skill 明确禁止 Agent 生成或粘贴用户凭据（已写入 SKILL.md）。
4. **公开数据**：用户报告只在所有者授权后公开；聚合统计匿名且可重算。
5. **国内平台主体**：WorkBuddy、魔搭、扣子需要实名或企业认证，主体一经确定后续难以更换（原因与做法见 6.1）。

---

## 9. 分阶段计划

| 阶段 | 时间 | 内容 | 依赖 |
|---|---|---|---|
| **G0**：随 1.0 发布 | 约 2 周 | 仓库公开；npm / Homebrew 首发；枢纽文件（3.1）；`listings/` 和禁用词检查；G0 目录全部提交（均无需实名）；WorkBuddy 手动导入教程；规则页拆分（4.3-1）；安全模型页；aieo.ee 自家案例 | 仓库公开、NPM_TOKEN、Homebrew tap |
| **G1**：1.1 | 约 4–6 周 | aieo.ee OAuth + `get_inside_view` + 未订阅可用工具；claude.ai 目录提交；豆包 / WorkBuddy / 千问连接器教程；Skill 扩到 6 个；徽章端点；Hermes、Cursor | aieo.ee 排期 |
| **国内商店**：不绑版本 | 主体确定后 | WorkBuddy 开放平台、魔搭、扣子的认证与提交（约 7 天审核） | 认证主体（6.1） |
| **G2**：1.2+ | 持续 | 框架默认模板对比与上游 PR；ChatGPT / Codex 应用；扣子；"修复到收录"的聚合统计；框架专题页 | G1 的数据积累 |

---

## 10. 待决定

1. **国内平台认证主体**：大陆个人、大陆公司，还是找合作方（WorkBuddy、魔搭、扣子共用）。Web-Casa 若非大陆公司则无法做企业认证。不影响 G0/G1，见 6.1。
2. **仓库公开时间**：skills.sh、Cursor、Glama、Actions Marketplace 都以公开仓库为前提，建议与 1.0 发布同步。
3. **aieo.ee 远程 MCP 的 OAuth 排期**：G1 的全部连接器渠道都卡在这一项。
4. **规则页拆分**：是否把 107 条规则各自生成独立页面（建议做）。
