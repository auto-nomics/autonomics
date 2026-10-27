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
