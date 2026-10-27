# API Server（HTTP API）

gateway daemon 启动时会启动 HTTP API 后端。该后端不是独立的文献服务，而是 REST API 的统一入口；文献管理是第一个业务模块。

## 架构

- 实现位置：`crates/api-server`
- 路由命名空间：`/api/v1`
- 默认绑定：`127.0.0.1:8765`，只监听本机回环地址
- 地址覆盖：`AUTONOMICS_HTTP_API_ADDR`
- 鉴权：`/api/v1/*` 需要凭据——`Authorization: Bearer <token>`（`AUTONOMICS_HTTP_API_TOKEN`，未设置时是 `~/.autonomics/gateway.token` 里每次启动生成的令牌）或浏览器会话 Cookie（`POST /api/session` 用令牌换取，`HttpOnly` + `SameSite=Strict`）；静态前端资源不受影响
- 令牌不落页面：`/` 返回的前端是静态壳，不注入任何令牌；浏览器只持有会话 Cookie，令牌不会出现在 HTML、JS 全局变量或 console 中
- 数据库：与 `RuntimeHost` 中的 `SharedInfra.bib` 完全共享同一个 `BibShared`，因此 API、TUI agent 工具和写作系统共享同一 Turso 连接与文献源 HTTP 客户端
- 原文存储：TUI 使用独立文献 VFS 空间 `vfs:///literature/...`；底层默认是 `state_dir/literature/` 本地目录，并自动挂载到 Agent VFS
- 生命周期：daemon 启动完成 `RuntimeHost::open` 后绑定端口；退出时先 graceful shutdown HTTP API，再关闭 agent 和共享基础设施
- 安全边界：默认只监听回环地址。跨不受信网络暴露时请在前面放一层反向代理（TLS + 自己的认证），并设置固定的 `AUTONOMICS_HTTP_API_TOKEN`

新增业务模块时，在该 crate 中实现独立 `Router`，再挂载到 `api_server::server::api_router` 的 `/api/v1/<module>` 下。不要在各业务模块内自行启动 `TcpListener`。

## 文献 API

| 方法 | 路径 | 功能 |
|---|---|---|
| `GET` | `/api/v1/bib/health` | 文献模块状态、文章数、启用的外部源 |
| `GET` | `/api/v1/bib/articles` | 搜索本地库，支持 `query`、`limit` |
| `GET` | `/api/v1/bib/articles/unfiled` | 列出所有未归入任何集合的文献，可选 `limit` |
| `POST` | `/api/v1/bib/articles` | 手动创建文献元数据 |
| `POST` | `/api/v1/bib/articles/import` | 按 DOI / PMID / arXiv 等标识符从外部源导入，并可自动获取 OA 全文 |
| `GET` / `PUT` / `DELETE` | `/api/v1/bib/articles/{id}` | 查看、更新、删除文献 |
| `GET` / `POST` / `DELETE` | `/api/v1/bib/articles/{id}/fulltext` | 查看、上传、删除全文；`GET` 支持 `offset` / `limit` 字符分页（默认 100000，最大 500000）；上传为 multipart 字段 `file`，原始文件按 SHA-256 内容寻址保存到文献 VFS，并自动抽取纯文本 |
| `GET` / `HEAD` | `/api/v1/bib/articles/{id}/fulltext/raw` | 流式返回 VFS 中的原始文件，支持单区间 HTTP Range；HTML/PDF 以安全下载语义响应 |
| `GET` / `POST` | `/api/v1/bib/articles/{id}/annotations` | 查看、新增注释 |
| `DELETE` | `/api/v1/bib/annotations/{id}` | 删除注释 |
| `GET` / `POST` | `/api/v1/bib/collections` | 列出、创建集合 |
| `GET` / `DELETE` | `/api/v1/bib/collections/{id}` | 查看、删除集合 |
| `GET` / `POST` | `/api/v1/bib/collections/{id}/articles` | 列出、添加集合成员 |
| `DELETE` | `/api/v1/bib/collections/{id}/articles/{article_id}` | 移除集合成员 |
| `PUT` | `/api/v1/bib/collections/{id}/status` | 更新集合状态 |
| `GET` | `/api/v1/bib/requests` | 列出待补全文请求 |
| `GET` | `/api/v1/bib/export` | 导出 BibTeX / RIS / Markdown / CSL JSON |
| `GET` | `/api/v1/bib/search/external` | 并发搜索 PubMed、arXiv、bioRxiv、OpenAlex、Crossref、Semantic Scholar |

全局端点：

```text
GET /api/health
GET /api/v1
```

## 前端

Rust 后端在根路径直接托管文献管理前端，无需另起前端服务：

```text
http://<本机IP>:8765/
```

前端为 React + TypeScript，界面组件来自本地 `src/components/ui` 下的 shadcn/ui 组件；构建与测试由 Bun 执行：

```bash
cd crates/api-server/frontend
bun install
bun test
bun run typecheck
bun run build
```

开发时可保持 Rust TUI/HTTP 后端运行，然后执行 `bun run dev`。Bun 会在 `http://127.0.0.1:5173` 启动前端开发服务，并把 `/api/*` 代理到 Rust 后端；后端地址可用 `AUTONOMICS_API_BACKEND` 覆盖。

## 示例

```bash
curl http://127.0.0.1:8765/api/v1/bib/articles?query=gwas&limit=20

# 设置 AUTONOMICS_HTTP_API_TOKEN 时：
curl -H 'Authorization: Bearer <token>' \
  http://127.0.0.1:8765/api/v1/bib/articles?query=gwas&limit=20

curl -X POST http://127.0.0.1:8765/api/v1/bib/articles/import \
  -H 'Content-Type: application/json' \
  -d '{"id_type":"doi","id":"10.1038/s41586-023-06236-2"}'

curl -X POST http://127.0.0.1:8765/api/v1/bib/articles/<article-id>/fulltext \
  -F file=@paper.pdf
```

注意：HTTP API 使用 runtime 配置的文献库路径，默认位于 `~/.autonomics/bib.db`，可用 `AUTONOMICS_BIB_DB` 覆盖。`autonomics bib` CLI 子命令沿用同一份 runtime 配置（`AUTONOMICS_BIB_DB` → `~/.autonomics/bib.db`），与 daemon 默认值一致；仅在需要时通过 `--db <PATH>` 显式覆盖。

### 本地文献检索 V2（P1）

`POST /api/v1/bib/search/v2` 接受 `SearchRequestV2`。例如：

```json
{"query":"title:\"gene editing\" AND year:2020..2026 NOT note:retracted","page_size":50,"sort":"relevance","scope":{"collection_id":"review","include_descendants":true}}
```

也可传 `expression` AST；`query` 和 `expression` 不能同时设置。文本语法支持大写 `AND`、`OR`、`NOT`、隐式 AND、括号、引号短语、`title/author/abstract/journal/identifier/doi/pmid/year/keyword/tag/note/fulltext` 字段，以及 `year:2020..2026`、`year:>=2020`、`year:<=2026`。普通文字不直接进入后端查询 DSL。裸 DOI 或 DOI URL 自动执行精确标识符检索。

响应为 `hits`、文献级 `total`、`next_cursor`、`index_generation`。下一页带上原请求和 `cursor`；查询或索引版本变化时返回 400 并提示重新检索。命中结果保留 `anchor_key` 与 `matched_field`，用于定位具体附件或批注。分页大小为 1–200；排序支持 relevance、title、newest、oldest。

`GET /api/v1/bib/search/status` 返回源版本、已索引版本、积压篇数、证据文档数和持久化状态。`POST /api/v1/bib/search/rebuild` 返回 202 和受理状态；后台构建通过 `building`、`processed_documents`、`target_revision` 报告进度，原有 `indexed_revision` 在提交成功前保持不变。提取完成和索引追平是独立状态。

发布开关 `AUTONOMICS_BIB_SEARCH_ENGINE=tantivy|legacy` 控制新引擎，默认 `tantivy`。设为 `legacy` 并重启后，旧文章/集合查询使用既有倒排词项表；V2 与重建入口返回明确的禁用错误，索引状态报告 `engine: "legacy"`。Rust 调用方也可用 `BibBase::open_with_search_engine` 显式选择引擎。派生索引和事务事件保留，恢复 `tantivy` 后可追平，不需要回退或替换权威数据库。

汉字使用词典分词和逐字位置约束；中文短语必须相邻，复合词内的词可被召回。`IL-6`、`HLA-A*02:01`、`α-synuclein` 等带符号科学词项按相邻词部分处理，`p53` 不匹配 `p530`。索引分析格式升级使用 `.search-v2` 新目录，旧目录保留以便审查和回退。

### P2 搜索工作流扩展（工作区实现）

V2 可选 `include_articles:true`，在同一读取快照中批量加载当前页文献信息；HTTP 响应另有 `attachment_summaries`。`include_facets:true` 返回分页前的分面：tags、creators、publication_types、years、fulltext_status。分面计数按不同父文献计数，忽略本分面自己的筛选，但保留查询、scope 和其他筛选；空缺年份以 `unknown` 桶展示。状态按附件判断，同一篇可能同时出现在 indexed 和 failed 桶；计数之和可能大于父文献总数。

精确筛选使用 `filters`：`tag_ids`、`creator_ids`、`publication_types`（各列表内部 OR、列表之间 AND），以及 `year_from/year_to`、`has_fulltext`、`has_annotations`、`fulltext_status`。`has_fulltext` 表示存在文件附件元数据或非空提取文本，不把 URL-only 链接作为文件；`fulltext_status` 可为 indexed/pending/failed/missing，indexed 表示至少一个附件文本已进入当前检索快照。年份有界时排除缺失年份。每个列表最多 50 个选择；`scope.library_id` 可限制文献库。

为降低连续输入和翻页成本，元数据缓存以索引 revision/token 为水位，按同一数据库快照的 outbox 逐文献失效。数据库恢复、同 revision 不同 token 会清空缓存。不会缓存全文或跳过当前范围/筛选/权限路径。

```json
{"query":"title:genetics","scope":{"collection_id":"review","include_descendants":true},"filters":{"year_from":2020,"publication_types":["Review"],"has_annotations":true},"sort":"newest","include_facets":true,"include_articles":true,"page_size":50}
```

| 方法 | 路径 | 语义 |
| --- | --- | --- |
| POST | `/api/v1/bib/search/parse` | `{query}`，返回 `{expression}`，与 V2 共用 parser，图形条件树可无损往返 |
| POST | `/api/v1/bib/search/cancel` | `{request_id}`，返回 202；配合原始请求的 `x-search-request-id`，取消后返回 499 |
| GET | `/api/v1/bib/saved-searches` | 返回 `saved_searches`，规则持久保存在数据库中 |
| POST | `/api/v1/bib/saved-searches` | `{name,request}`，返回 201 及 `saved_search`；保存时清除临时 cursor |
| PUT | `/api/v1/bib/saved-searches/{id}` | `{name,request,expected_version}`，成功后 version 加一；版本冲突返回 409 |
| DELETE | `/api/v1/bib/saved-searches/{id}?expected_version=N` | 204；不存在为 404、版本冲突为 409；不删除任何文献 |
| POST | `/api/v1/bib/articles/{id}/search-evidence` | `{search:SearchRequestV2,index_generation?:string}`，按相同查询/scope/filters 检查资格，返回 evidence/total/next_cursor/index_generation |

保存搜索版本为 schema_version 1，保存查询规则而非结果；取出 request 重新执行 `/search/v2` 即可动态获得新增匹配文献。名称要求 1–120 个可打印字符，非法 AST/过滤条件不能持久化。

证据支持独立 cursor 分页，游标与文献、查询、范围、筛选和内容代际绑定。查询结果旧代际传入 `index_generation` 后若数据已改变，返回 400 要求刷新。证据 total 是不同证据对象数，不是词出现次数。仅返回实际满足布尔分支的正向证据，NOT 不生成虚构命中片段。

每个证据携带根 item_key、可选 attachment_key、annotation_key/annotation_id、matched_field、snippet、highlight_ranges、matched_query 和 content_revision。高亮范围是片段内 **Unicode code point 半开区间**，JavaScript 使用 `Array.from(snippet)`，不能直接对字符串按 UTF-16 下标切片。只有已存批注锚点才返回 page；无可靠 PDF/SDT 位置映射时，location_status 为 text_search，调用方打开对应附件并使用 matched_query 定位，不推断页码。metadata 证据在详情中展示。


`POST /api/v1/bib/search/export` 接收 `{search:SearchRequestV2,format?:"bibtex"|"ris"|"markdown"|"csl_json"}`。查询、scope、精确筛选、排序和元数据水合在同一读取事务中执行，导出全部匹配记录；不受当前 page_size 或已加载页数限制。带 cursor 的请求返回 400，避免把一次分页误当成完整导出。旧 GET `/export?ids=...` 继续支持显式勾选记录和勾选顺序。

`default_fields` 可为无字段词/短语指定多个具体字段（字段之间 OR），显式 `title:` 等不受影响；最多 10 个字段，不接受 `any`。标题/作者/年份与元数据/笔记快捷范围通过该属性实现。旧本地搜索 API、集合和 Agent/TUI 默认使用相同 parser/AST；显式 legacy 引擎只保留基础词项兼容行为。

取消适用于 V2、证据和完整搜索导出，支持取消先于注册到达；request ID 为 1–128 个 ASCII 字母、数字或 `-_.`。取消会结束等待、丢弃接收端并跳过尚未运行的搜索任务；正在执行的 CPU 索引/查询不会被强制中断。浏览器通过 AbortSignal 和取消 POST，Tauri 通过 request ID 与取消命令中止底层 HTTP 请求。

附件证据带可选 `attachment_hash`。读取 `/articles/{id}/attachments/{attachment_key}/raw?expected_hash=...` 时，hash 与当前原始附件不符返回 409，避免点击旧证据后打开已替换的文件。hash 必须由实际快照返回，调用方不自行猜测。

### P3 外部检索会话（已实现并通过本地阶段验收）

会话、库内匹配、导入、全文后台任务和独立发现 GUI 已实现。事件使用经过同一鉴权的 JSON 长轮询，浏览器与 Tauri 均复用现有 gateway HTTP 传输，不依赖仅浏览器可用的 EventSource。

当前发现适配器包括 PubMed、OpenAlex、arXiv、Crossref、Semantic Scholar（`semantic_scholar`）。`/search/sources` 的 `authentication` 表示上游认证要求（`none/optional/required/required_for_continuation/unknown`），与本系统 HTTP 鉴权分开；`credentials_configured` 只表示已配置，不代表已验证有效。arXiv 无需 key，PubMed/Crossref/S2 的 key 可选，OpenAlex 的后续游标页需要 key。

arXiv 支持元数据字段、正向条件和直接 AND 排除、精确 arXiv ID（保留版本）及原生 `search_query/id_list`；不把出版年份、类型或 OA 条件替换成其他含义的筛选。提交日期使用原生 `submittedDate`。Newest/Oldest 在该源按首次提交时间排序。服务端保存 offset，发现总数变化时返回逐源 `cursor_expired`；offset 本身不能保证上游结果快照。上游窗口最多 30000 条，本地会话缓存预算也可能更早停止继续获取。每个共享 arXiv 客户端串行连接、请求开始间隔至少 3 秒；HTTP 正文有 16 MiB 上限，长查询使用 POST form。HTTP 200 错误 Atom、损坏 XML、错误页和错版本记录分别报告失败，不伪装成零结果。

Crossref 支持精确 DOI、出版年份和可映射的类型条件；单个 DOI 使用 singleton 端点，DOI OR 分组可与年份等条件求交，矛盾的 DOI 条件明确拒绝。文本需要 `allow_approximate: true` 或显式原生查询，保留 `approximate_text` 提示：该源不保证严格词项/短语匹配，标题转为范围更广的 `query.bibliographic`。跨字段 OR、NOT、OA、review/clinical_trial 等无法保真的条件不丢弃，报告 unsupported。原生参数白名单为 `doi/query/query.bibliographic/query.author/query.container-title/query.affiliation/filter`，单独 DOI 不能与其他参数混用；凭据、分页及已弃用 `query.title` 不接受。无文本时默认相关性排序降级为出版时间降序并说明。真实 provider cursor 与取得数量绑定保存，允许 raw cursor 重复；短页结束，不声称外部快照稳定。超过支持大小的游标返回明确错误。

共享 Crossref SDK clone 串行连接并按端点类型限速：未观察响应头时，公共 list/singleton 默认分别为 1/5 次每秒，配置联系邮箱的 polite pool 为 3/10；随后遵循 `x-rate-limit-*` 观测值，保留 `Retry-After` 秒数/HTTP 日期及共享冷却。成功正文上限 16 MiB，错误正文上限 64 KiB；错误封装、缺失列表计数/记录等不会被当成零结果。后端配置使用可选 `CROSSREF_MAILTO` 和 `CROSSREF_PLUS_API_TOKEN`，不进入公开执行参数与发现错误。速率依据 [2026 年 7 月官方调整](https://community.crossref.org/t/refining-rest-api-limits-for-improved-stability-and-reliability/16137)，额度余额未知时不作推断。

Semantic Scholar 分为三个执行端点：`bulk` 保留支持的布尔/短语与元数据条件，没有源相关性排序，默认按 paper ID；`relevance` 使用普通文本匹配、最多取得前 1000 条；`lookup` 精确获取单个 DOI、PMID、未指定版本的 arXiv ID、S2 hash 或 CorpusId，并核对响应身份。显式 arXiv 版本不替换为其他版本。普通查询默认使用 bulk；开启 `allow_approximate` 后，可表达为正向文本提示且未指定其他源排序的查询使用 relevance。排除条件仍使用 bulk，不能因近似匹配被删除。标题/摘要单字段限制不能严格执行时必须显式同意扩大范围，并返回说明；作者名、MeSH 等不折成正文词。独立纯否定分支、元数据 OR 与正文的不可分发组合或过大布尔展开明确 unsupported。

S2 的元数据支持年份、venue 和可映射类型；通用 OA 条件不能替换成更窄的 public-PDF 条件。原生参数白名单为 `endpoint/query/paper_id/year/publicationDateOrYear/venue/publicationTypes/fieldsOfStudy/openAccessPdf/minCitationCount`（仍受通用参数个数上限限制）。`endpoint` 是适配层的 bulk/relevance/lookup 路由选择，不发送为上游参数；lookup 仅接受 paper_id，openAccessPdf 必须为空字符串的 presence flag。分页、fields 和凭据不能覆盖，token/offset 由服务端保存。`sort` 使用请求字段：bulk 可按出版日期或引用量排序，relevance 不支持这些源排序。

上游 bulk 没有页大小参数。S2 适配器将最多 1000 条的上游批次拆成请求的 1–100 条小页，每个 S2Source 实例最多保留 32 个批次、32 MiB 序列化条目预算，30 分钟后按需淘汰；这不是进程 RSS 上限，也不属于会话 128 MiB 候选预算。淘汰后续页可重取原批次并比对上游元数据指纹，变化则返回 cursor_expired；指纹不含本地转换时钟。`executed_parameters` 的 batch_offset/page_size 为本地拆页信息，batch_fetched_at 为实际取得该上游批次的时间，避免把缓存读取误认为刚抓取的源数据。源总数为估计值，provider token 决定是否续页；上游 bulk 最多 10000000 条，本地会话预算可能更早结束。响应记录重复/损坏、无效计数、重复 token 和错误对象都不当成零结果。

共享 S2 SDK 请求起始间隔至少 1050ms，不累积空闲额度，也不为等待中的取消操作预占未来时间；保留完整 Retry-After 秒数/日期和共享冷却。发现适配关闭 SDK 内部重试，由会话统一调度预算。默认连接/请求超时为 10/90 秒，外层查询预算可更早取消；自定义 reqwest 客户端的超时保留。成功/错误正文上限分别为 16 MiB/64 KiB，公开发现错误及 SDK Debug 不含配置的 S2_API_KEY。协议依据 [Academic Graph API](https://api.semanticscholar.org/api-docs/graph) 和 [官方教程](https://webflow.semanticscholar.org/product/api/tutorial)，本地资格记录不代表线上供应商始终可用。

| 方法 | 路径（均在 `/api/v1/bib` 下） | 语义 |
| --- | --- | --- |
| GET | `/search/sources` | 源能力、凭据是否配置、最近观察的上游预算以及并发槽位/冷却状态；未知预算为 null，不推断余额 |
| POST | `/search/sessions` | 创建后立即返回 201 `{session}`，各来源独立执行；可带 `x-discovery-session-id: UUID` |
| GET | `/search/sessions/{id}` | 会话快照、逐源状态/总数/计划/实际执行参数/查询翻译，及已取得的去重候选数 |
| GET | `/search/sessions/{id}/events?after=N&wait_ms=20000` | 最多等待 25 秒；返回 sequence、events、reset_required。事件过旧时必须取新快照，不静默跳过 |
| GET | `/search/sessions/{id}/results?offset=0&limit=50` | 按候选分页（limit 1–100）；order 支持 first_seen/relevance/newest/source_rank，source 可限制来源 |
| GET | `/search/sessions/{id}/candidates/{candidate}` | 完整源记录、字段冲突、版本关系、疑似重复及候选 RRF；合并前的旧候选 ID 可解析到当前根 |
| POST | `/search/sessions/{id}/sources/{source}/next` | 使用服务端保存的原生游标继续该源，其他源不重跑 |
| POST | `/search/sessions/{id}/sources/{source}/retry` | 重试失败/已取消的单源；同源已有操作时返回 409 |
| POST | `/search/sessions/{id}/cancel` | `{source?:string}`，取消指定源或全部源，保留已经取得的结果 |
| DELETE | `/search/sessions/{id}` | 幂等关闭并释放缓存；有效 UUID 即使尚未创建也返回 204，登记有界提前取消记录 |
| POST | `/search/sessions/{id}/import/preview` | 校验候选指纹和来源选择，返回库内已有项、版本关系、字段差异、目标库/集合及可写状态；不写入 |
| POST | `/search/sessions/{id}/import` | 按项事务导入，返回 imported/already_present/failed；可选创建全文获取任务 |
| GET | `/articles/{id}/discovery-provenance?offset=0&limit=20` | 持久化来源、实际执行查询与导入所选版本，独立于检索会话有效期 |
| GET | `/search/acquisitions/{id}` | 全文任务及逐项 queued/running/available/unavailable/failed/cancelled 状态 |
| POST | `/search/acquisitions/{id}/cancel` | 取消尚未开始的全文获取；已运行的文件导入完成后停止后续项 |

创建请求使用 `DiscoveryRequest`：query、expression、native 三者择一，也允许只提供 filters；sources 默认 PubMed/OpenAlex，page_size 默认 25、上限 100。`source_timeout_ms` 默认 20000，`deadline_ms` 默认 30000（范围均为 100–120000），预算包括排队、冷却和自动重试。原始请求和每个源的实际计划分别保留。未知条件/不支持操作不能静默降级成宽查询。

`allow_approximate` 默认 false；显式开启后只允许有说明的来源文本近似匹配，不放弃精确标识符、筛选或排除条件。GUI 普通查询提供对应选择，原生模式使用所选单源的原始语义，并分别保存各源草稿。

GUI 的 Source order 控制发给供应商的排序，改变后提交新查询；结果区的 First obtained / Combined source rank / Newest obtained / Source rank 仅排列已取得候选，不能用本地候选重排冒充已按该顺序检索整个上游数据源。

客户端应先生成 UUID，再发起带该 ID 的创建请求；同 ID、同请求不会重复执行，不同请求返回 409。即使创建响应被中断，DELETE 该 ID 仍能阻止迟到的创建。提前取消记录保留 60 秒、最多 1024 条。默认最多 32 个会话，TTL 为 30 分钟；保留 256 条事件、每会话最多 10000 条源贡献，所有会话合计最多 50000 条及 128 MiB 序列化贡献预算。预算不是 RSS 承诺。失效会话查询返回 404；容量不足返回 429 或逐源 capacity，已有结果不丢弃。

默认结果按首次取得顺序稳定展示；RRF 仅融合已取得候选的源内排名。跨源 total 分开报告，`obtained_candidates` 不是全网去重总数。结果请求可携带 `sequence`，与当前快照不同返回 409，避免去重合并期间 offset 漏项。DOI/PMID 等强身份可合并，冲突 DOI、预印本/正式版或仅标题相似只产生关系/审查提示；所有源贡献保留，较晚到达者不直接覆盖已有字段。

导入请求的 `selections` 为 1–100 项 `{candidate_id,expected_fingerprint,record_index?:0}`，可带 `library_id`、`collection_id`、`fetch_fulltext:false`。较晚来源改变候选后，旧指纹返回 409，要求重新确认预览。导入已有项保留其元数据、集合角色和笔记，只添加缺少的集合成员关系及审计来源；多个已有项同时匹配时返回逐项失败，要求先人工审查。预印本与正式版分别保存并建立可追溯关联；一次失败不回滚同批已成功的其他项。每条持久化 provenance 最多 2 MiB，查询包含所有来源贡献、所选来源、原始请求、源计划和实际查询参数，不保存密钥。

`fetch_fulltext:true` 在写元数据前检查 VFS 配置和队列容量；缺文件存储返回 503，队列满返回 429。成功响应的 `acquisition` 为任务快照或 null（没有可获取的成功项）。后台复用现有 PMC PDF/VFS/附件提取链路，已有 PDF 优先复用；`available` 只说明原始附件可用，提取/索引另由附件状态报告。全进程最多 16 个活动任务、2 个同时获取，每项最长 120 秒、整个任务预算 15 分钟。取消不会删除元数据或已保存附件。进度为进程内状态，重启不自动重放；完成后保留最多 30 分钟，最多保留 64 个任务，容量回收时较旧完成任务可能提前失效。删除检索会话不取消全文任务，也不删除已存来源。所有这些接口均继承鉴权并返回 `Cache-Control: no-store`。
