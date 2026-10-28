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

### P4 语义索引配置与后台服务（工作区实现）

`GET /api/v1/bib/search/semantic` 返回持久配置、configuration_generation、phase、向量 generation/signature、已索引 revision/chunks 和 last_error。phase 为 disabled/loading/indexing/ready/paused/failed；当前 indexed_chunks 是向量块数，不是全文覆盖比例。默认关闭，模型不会自动下载。仅 Tantivy 引擎支持该服务。

`PUT /api/v1/bib/search/semantic` 接受完整配置并返回 202、配置 generation 和当前状态。省略字段取默认值，因此调用方修改配置前应先读取当前值并提交完整对象；未知顶层字段拒绝。受理表示配置已持久化，不代表构建完成。配置示例：

```json
{"enabled":true,"origin":"http://127.0.0.1:11434","allow_remote":false,"model":"qwen3-embedding:0.6b","model_digest":"<已安装模型的64位小写SHA256 manifest digest>","profile":{"dimensions":1024,"query_instruction":"Given a scientific claim, retrieve scientific papers that provide relevant evidence.","chunk_chars":null,"overlap_chars":0,"include_annotations":false,"include_fulltext":false},"batch_size":16,"timeout_seconds":120}
```

model_digest 的占位值必须替换为实际 digest。启用需要已安装模型及匹配 digest，服务创建独立 digest 别名并逐批核验。origin 必须是无凭据、路径、查询参数的 HTTP(S) origin；默认只允许 loopback，非本机地址要求显式 allow_remote=true。模型会收到配置所选的本地文本，默认仅标题与摘要；全文/批注索引另需窗口策略。

可选 `inference` 接受 `{"num_ctx":8192,"num_batch":8192}`，分别为模型 context token 数和 token 批大小，区别于 `batch_size` 的每请求文献输入数。当前应用支持范围分别为 1–131072 和 1–8192；未知字段及非法数值拒绝，具体可用值仍受模型和服务器限制。省略、空对象或 null 子字段使用服务器默认，不发送这些覆盖项，并保留原无覆盖配置的 provider 签名。任一显式覆盖值变化都会改变 provider 签名，隔离向量缓存；旧配置可继续读取，不能将新旧参数下的缓存混用。GUI 将它们放在高级模型选项，留空即可恢复默认。参考 [Ollama embedding API](https://docs.ollama.com/api/embed) 和 [官方 Runner 类型](https://github.com/ollama/ollama/blob/main/api/types.go)。这只增加参数传输与版本隔离，不代表任意模型或长文输入均已验证。

配置和派生向量分别持久化在文献数据库及 `<数据库路径>.semantic-v1`。配置任务、索引任务和查询 embedding 不占用主存储/词法 actor；停用或模型切换会取消旧构建与旧查询准备，其他句柄定期观察持久配置。损坏配置报告 failed，可通过替换配置修复，库内词法检索继续可用。接口沿用现有 API 认证策略。

可选 `limits` 持久保存以下资源策略，缺省沿用表中默认值。它不进入模型/分块 signature，持久库可复用已保存的有效向量；配置 generation 仍照常更新，旧游标失效。内存库按其连接/worker 生命周期重建缓存。

| 字段 | 默认值 | 支持范围与统计对象 |
| --- | ---: | --- |
| `max_chunks` | 1,000,000 | 1–10,000,000；当前完整索引 generation 的块数 |
| `max_input_bytes` | 262,144 | 1–16,777,216；UTF-8 字节，文献输入含标题、换行与窗口，查询含 instruction |
| `max_vector_bytes` | 2,147,483,648 | 1–1,099,511,627,776；单个物理向量库中所有 profile、可恢复批次的向量 BLOB 载荷 |
| `max_disk_bytes` | 8,589,934,592 | 1–1,099,511,627,776；该文献库主语义文件及规范 UUID 修复文件，含各自 WAL/共享文件 |

载荷计数通过派生库中的事务触发器维护，旧库第一次打开时在写事务中初始化；忽略重复插入、删除、修复向量与回滚均更新同一计数。embedding 前预检，提交前在同一写事务再次检查载荷预算，防止并发 worker 同时耗尽剩余额度。单输入和块数超限不静默截断，不发布不完整 generation。额度不是进程 RAM、GPU 或硬盘的操作系统硬配额。

磁盘占用按文件大小协作检查，包含旧模型、待恢复与私有修复文件；在批次边界及新修复库创建前检查。并发批次、打开数据库和 WAL/checkpoint 的临时写入可能超过预算，不能保证文件系统即时不超额。不会为满足额度自动删除已发布缓存或压缩文件；提高预算可继续，减少索引内容不保证磁盘文件立即缩小。

普通增量资源错误在状态中报告 `failed` 和具体资源、需要量与上限；源版本及相关额度的剩余空间未改善时保持等待，避免重复规划和 GUI 闪动。源版本或配置变化后重新尝试；载荷/磁盘错误也在对应占用减少后重试，其他 worker 的 WAL 增长不会唤醒块数受限的构建。词法与文献写入继续独立工作，不交付陈旧语义结果。查询输入资源错误返回 503。

只调整资源预算时，配置与未完成维护任务在同一事务重绑定到新 generation，保留原 request_id 和私有修复库，旧 worker 的心跳/激活被隔离。资源失败的维护任务重新排队并复用已完成批次；用户明确暂停的任务仍保持暂停。其他模型、预处理或连接配置变化继续使用原替换规则，不沿用旧修复尝试。`resources` 保存最近一次成功就绪时的 `vector_bytes` 与 `disk_bytes`，后者在内存库为 null，不是逐次 GET 的实时磁盘测量。GUI 高级设置提供额度输入，主状态显示最近观测的缓存占用。

正常同步检测到向量块身份完全不变时，新逻辑 generation 可复用旧物理绑定集；年份、标签等元数据变化无需复制全库绑定。引用被压平至根绑定集，回收保护仍被活动版本引用的根；内容替换、删除及强制重建继续物化绑定。该优化不改变查询、游标或证据的版本校验。

离线恢复数据库后，派生索引不能仅凭更高的 revision 推断来源于旧 head 的同一分支。新句柄首次同步时，若源 revision/token 与持久 head 不一致，词法及语义索引重新枚举权威投影；语义部分复用通过校验的向量。版本完全一致的重开仍直接复用，进程内正常同步仍按 outbox 增量执行。离线变更较多时，这会增加首次追平成本；不支持在仍打开的数据库连接下直接替换数据库文件。

状态新增 progress、maintenance 和 store_generation。progress 的 planning/embedding/publishing/complete 表示本轮阶段；processed_articles/chunks 是已处理数量，embedded_chunks 是本轮新增向量输入数，cached_chunks 是复用绑定数，重复文本使这两个数不一定相加等于块总数。进度在批次提交后更新，不把正在计算的批次算作完成，也不把 ready 当作全部文献内容均已覆盖。后台服务初次资格见 [P4 服务记录](benchmarks/literature-search-p4-service-linux.json)，后续主链路资格见 [P4 混合查询记录](benchmarks/literature-search-p4-hybrid-linux.json)，GUI 与真实 Linux 原生验证见 [P4 GUI 记录](benchmarks/literature-search-p4-gui-linux.json)。

`POST /api/v1/bib/search/semantic/maintenance` 沿用 API 鉴权，接受以下请求并返回 202、`{maintenance,status}`：

```json
{"action":"repair","expected_configuration_generation":"<GET 返回的当前配置 generation>","request_id":"<本次操作的小写规范 UUID>"}
```

- `rebuild` 从权威文献重新枚举，同一模型/预处理下复用校验通过的向量；`repair` 在独立派生文件 `.semantic-v1.repair-<UUID>` 构建，完成后事务切换活动存储，保留旧文件供隔离诊断。
- `cancel` 暂停后台索引并持久保存 paused 状态，重启不会自动恢复；`resume` 继续同步，并复用未完成修复已提交的批次。暂停不删除原文或向量。重启后的 paused 服务等待 resume 才重新连接模型。
- 每次新操作推进配置 generation，旧查询准备和续页失效。当前操作的同一 request_id、action 和原配置 generation 重试幂等；配置过期或 ID 被其他操作占用返回 409，非法参数返回 400。客户端应保留旧列表，清除续页游标并提示刷新。
- 维护任务持久化 queued/running/complete/failed/cancelled。跨进程通过 15 秒所有权租约及约 1 秒心跳接管；过期 worker 不能发布或激活结果。配置替换会取代旧维护任务。健康旧快照仍须通过源版本与活动存储校验才能提供查询。
- 私有修复文件清理保护活动模型、运行租约与待恢复任务，租约到期后再留 5 分钟宽限，每轮最多处理 16 个候选；原始派生库及多模型载荷受上述预算约束，已发布模型不会自动逐出。运行中的临时 generation 会在模型请求等待期间续期，常规同步也清理过期的未发布代；多附件容量及真实长批次压力仍须单独验收。

维护资格包括真实 SIGKILL 后重用 16 块、仅补算 4 块，以及实际 Qwen/Linux 原生 GUI 操作，见 [维护与恢复收据](benchmarks/literature-search-p4-maintenance-linux.json)。这些恢复用例不代表 100k 容量或中文相关性验收。

`POST /api/v1/bib/search/semantic/models` 接受 `{ "origin": "http://127.0.0.1:11434", "allow_remote": false }`，返回已安装模型的 name 与 manifest digest。它只读取 Ollama 兼容服务的模型元数据，不保存配置、不下载模型、不创建别名，也不发送文献文本。origin 遵循相同的格式、2,048 字节长度与显式远端选择约束；响应限制为 1 MiB、最多 4,096 个模型，超时 10 秒，禁止重定向。模型存在不代表支持 embeddings，连接和实际维度仍单独验证。接口沿用 API 认证；上游不可用或身份无效返回 503，失败消息不包含上游正文或地址。

`POST /api/v1/bib/search/v2` 现支持显式 hybrid 请求，例如：

```json
{"mode":"hybrid","semantic_query":"immune cells and gene expression","query":"year:2020..2026 NOT tag:excluded","page_size":20,"include_articles":true}
```

semantic_query 是主题描述，不经字段或布尔 parser；query/expression 继续是硬条件，二选一。主题词法召回按 OR 连接：中文连续片段先用与索引相同的固定词典分词，再连接词项；非中文科研符号片段保留原样，英文重复词和原有表达式保持一致。此转换只用于主题召回。主题限制为非空、最多 16,384 UTF-8 字节和生产解析预算内的词项/子句；纯标点拒绝。lexical 模式不能携带非空主题。default_fields 继续作用于无字段的硬查询词，语义输入内容由索引配置选择。

查询 embedding 在进入检索队列前准备，不占用词法队列。两路候选先受相同硬条件、集合、库与精确过滤约束，再各取 100 篇，按冻结的词法权重 1、语义权重 4、常数 10 执行 RRF。标题/摘要等原始证据按当前数据库内容校验，模型不重新解释 NOT 或范围。服务未启用、模型故障或索引未就绪返回 503；准备快照落后、配置切换或源历史分歧返回 409。不会静默回退成更宽查询。

混合响应新增 retrieval：algorithm、total_kind=candidate_pool、eligible_articles、candidate_depth、权重/常数、模型配置与向量 generation、indexed_chunks，以及仅当前页的 hits 召回说明。各文献说明包含 lexical_rank、semantic_rank 和可选 semantic_evidence。total 是已取得两路候选的去重数，至多 200，不是所有语义相关文献的总数；eligible_articles 是满足硬条件与筛选的文献数。facets_basis=hard_query_disjunctive_filters 表示分面沿用硬条件/范围下的析取过滤统计，不冒充语义候选池统计。排序作用于已取得的候选池。

纯语义命中使用实际原文窗口，不生成逐字高亮；semantic_evidence 的 start_byte/end_byte 指规范摘要/正文的 UTF-8 窗口，不是 PDF 坐标，snippet 是该窗口的真实摘录。证据 API 同样接受主题请求，校验主列表 generation，并提供语义证据与原有附件/批注定位信息。游标同时绑定主题、硬条件、源版本、向量代、配置代和融合策略；过期返回 400，要求重新搜索。

保存规则保留 mode、semantic_query 和原有硬条件；旧规则缺少新字段时保持 lexical。`POST /search/export` 的混合导出遵循相同候选与硬条件，响应 result_basis=candidate_pool；词法导出为 complete_matches。导出仍拒绝页游标，混合模式导出的是整个已召回候选池，不是全库语义相关全集。

主 GUI 现有显式 Keywords/Topic 模式：主题输入提交后执行，IME 期间 Enter 不提交；必需条件保留独立输入与高级条件树。切换模式保留条件，不把主题当布尔语法。保存规则、URL/重载和历史分别保留含义，主题与关键词历史分开。结果显示候选池、两路召回和原文片段；取消、故障或新请求等待保留旧结果并显示状态，用户可主动切回关键词。模型配置可读已安装模型并固定 digest，展示文本范围及处理位置，远端处理需要显式选择。浏览器、模拟桥接与真实 Linux WebKitGTK 已验证；完整 P4 容量/恢复、中文真实质量、其他操作系统与人工可访问性仍待收口。

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


## P5 引文与相似论文列表（当前开发树）

`POST /api/v1/bib/search/explore` 继承现有鉴权；浏览器和桌面均可带 `x-search-request-id`，通过既有 `/search/cancel` 取消。请求示例：

```json
{"seed":"DOI:10.1234/example","kind":{"kind":"references"},"page_size":25,"cursor":null,"deadline_ms":15000}
```

`kind` 可为 `{"kind":"references"}`、`{"kind":"citations"}` 或 `{"kind":"similar","pool":"recent"}` / `{"kind":"similar","pool":"all-cs"}`。种子支持 S2、DOI、PMID、未指定版本的 arXiv 和 CorpusId；后四者先走与 P3 相同的精确身份验证。页大小 1–100；整体期限 100–120000ms，缺省 15000，包含共享限流等待与身份查找。每页不进行隐藏重试；推荐端点没有下一页。

响应包含 `seed`（规范 S2 ID）、`kind`、`source`、`retrieved_at`、`items`、`unresolved_records`、`next_cursor`、`upstream_snapshot:false`。每项保留上游 `position`、可导入结构的 `article` 或 `unresolved` 元数据、引用 `citation`、原始 `contexts` / `intents` / `is_influential`。未解析身份没有 article 或引用边；推荐没有引用边。引用边明确记录 citing/cited ID、来源和抓取时间。items 是来源窗口，不能当作全网总数；后续页可能受上游更新影响。

无效输入 400，游标不匹配 409，取消 499，上游限流 429，期限耗尽 504，认证配置不可用 503，其他上游错误 502；非法 JSON/枚举由框架返回 4xx。上游失败额外提供结构化 `failure`（kind、retryable、retry_after_seconds），不回传上游原始 body。429 保留 Retry-After。该接口不写入文献，不接受客户端回传条目直接导入；导入预览交接、执行快照与订阅接口仍在 P5 后续工作中。

## P5 固定检索快照（当前开发树）

磁盘模式快照保存在规范文献数据库中；原始附件文件不复制。所有接口返回 `Cache-Control: no-store`。

| 方法 | 路径（`/api/v1/bib` 下） | 用途 |
| --- | --- | --- |
| POST | `/search/snapshots` | 由服务器执行并捕获；行动键幂等，成功及已完成重试均返回 201 |
| GET | `/search/snapshots?limit=50&cursor=…` | 1–100 条分页；游标绑定存储身份、列表水位与锚点 |
| GET | `/search/snapshots/{id}` | 读取保存的 summary/capture，不按 ID 重新水合当前文献 |
| DELETE | `/search/snapshots/{id}?expected_checksum=…` | 校验值匹配才删除；响应为 `{deleted:bool}` |
| POST | `/search/snapshots/{id}/export` | `{format:"ris"\|"bibtex"\|"json",max_output_bytes?:16777216}`；返回文件名、类型、完整内容与数量的 JSON 包 |
| GET / PUT | `/search/snapshots/limits` | 持久 count/per-capture/total 预算；降低额度不自动删除已有记录 |

创建请求示例：

```json
{"key":"client-generated-uuid","name":"Review results","max_items":1000,"deadline_ms":30000,"execution":{"kind":"library","search":{"query":"gene","page_size":50}}}
```

发现捕获的 execution 为 `{"kind":"discovery","session_id":"…","sequence":123,"order":"first_seen"}`，也支持现有候选排序。会话与候选在同一锁内捕获；会话变化返回 409，要求刷新。已提交的同一行动可在会话消失后重试；改变名称、查询、窗口或排序不能复用行动键。客户端提交结果行被拒绝。

本地捕获强制元数据水合、从第一页开始，分页始终校验同一 generation；混合模式记录实际服务模型与各页 retrieval generation。按完整实际页面保存，短末页可容纳时包含它；不足以容纳下一页则停止并明确部分覆盖。创建请求体最多 128KiB；max_items 为 1–10000，deadline_ms 为 100–120000（默认 30000），包含查询与持久提交。可带 x-search-request-id 并通过既有取消接口中止等待；已经提交但响应丢失的动作可以安全重试。

不支持的输入 400，版本/行动冲突 409，预算超限 413，取消 499，执行不可用 503，期限耗尽 504，快照不存在 404，完整性损坏或存储异常 500。JSON 导出保留完整执行信息；RIS/BibTeX 为冻结文献投影。GUI 只预览前 100 篇，导出不静默截断。

## P5 持久订阅（当前开发树）

订阅存储使用同一文献数据库的独立连接。以下路径均位于 `/api/v1/bib` 下，继承宿主 bearer 鉴权，并返回 `Cache-Control: no-store`。JSON 请求体上限 128KiB；API 确认/存储工作有 15 秒期限，后台检查使用规则自己的单轮和每源期限。HTTP 响应丢失不取消已经接受的检查，客户端以原 `run_id` 重试。

| 方法 | 路径 | 用途 |
| --- | --- | --- |
| GET / POST | `/subscriptions` | 返回 `{rules}`；创建接受完整 `SubscriptionSpec`，返回 201 `{rule}`，默认暂停 |
| GET / PUT / DELETE | `/subscriptions/{id}` | 读取 `{rule}`；替换接受 `{version,spec}`；删除要求 `?version=…`，成功 204 |
| POST | `/subscriptions/{id}/enabled` | `{version,enabled}`；版本 CAS，暂停/恢复保留检查点并隔离旧执行 |
| POST | `/subscriptions/{id}/check` | `{run_id:"UUID"}`；手动检查可用于暂停规则，返回 202 `{run}`；保留记录内幂等，跨规则 UUID 冲突 |
| GET | `/subscriptions/status` | `{scheduler:{running,active,last_error}}` |
| GET / PUT | `/subscriptions/limits` | `{limits}`；PUT 接受完整 `SubscriptionLimits`，降低预算不逐出已有记录 |
| GET | `/subscriptions/{id}/runs?before=…&limit=50` | 返回 `{runs,next}` 摘要，limit 为 1–100 |
| GET / DELETE | `/subscriptions/runs/{id}` | 读取 `{run}` 中的规则副本、实际计划、预算计数和来源终态；只允许删除已结束记录 |
| POST | `/subscriptions/runs/{id}/cancel` | 持久取消；重复取消旧终态不影响新执行，返回 `{run}` |
| GET | `/subscriptions/inbox?before=…&limit=50` | 返回 `{items,next}`；持久新增匹配及来源元数据，limit 为 1–100 |
| PUT / DELETE | `/subscriptions/inbox/{id}` | 标记 `{read:true\|false}` 或删除，成功 204；删除不会忘记已见身份，也不删除规范文献 |

规则示例：

```json
{"name":"免疫主题追踪","request":{"query":"immune","sources":["pubmed","openalex"]},"enabled":false,"interval_seconds":3600,"initial_days":30,"overlap_days":3,"window_days":1,"max_pages_per_run":20,"max_records_per_run":1000,"run_timeout_ms":120000}
```

宿主路由在最终来源注入后初始化共享调度器，默认同时执行最多两条规则；已启用规则在重启时恢复，不依赖打开订阅界面。调度句柄全部释放时取消网络工作并结算运行记录。每轮按来源轮换第一页的优先次序，单页预算不足不改变供应商游标的页面大小。

`records` 表示已在页面事务中成功提交的来源记录数；失败请求仍消耗页面尝试预算。只有所有来源完成本轮冻结目标才更新全局成功时间。失败、尚未穷尽和预算停止均不表示“成功无新增”。PubMed 当前日延迟、Crossref 索引窗口与其他来源有界重放的限制见订阅设计；没有普通 OpenAlex key 自动获得更新日期权限的假设。

存储额度失败会保存 `rule.resource_wait`（since_ms、reason、observed_epoch）。相同资源版本下禁止重复领取和手动绕过；修改有效额度、删除历史/通知或释放其他订阅空间后可重试，资源变化不会被“失败后记等待”的竞争窗口吞掉。相同额度重复保存不触发重试；已见身份不会因清理通知而遗忘。

非法输入 400、版本/所有权/并发冲突 409、不存在 404、预算超限 413、API 确认期限耗尽 504，存储异常 500；非法 JSON 和未知字段由框架返回 4xx。当前 API 不自动导入规范文献，也不发送邮件或外部消息。订阅 GUI、从通知进入独立发现与确认导入、跨进程强杀/恢复和实际原生验收仍待完成。

### Crossref 分页兼容性（2026-10-28）

Crossref 官方已在 2026-08-24 发布游标实现变更：发表日期排序与游标的组合会被拒绝，翻页需重发完整查询参数并使用每页返回的新游标。依据见 [官方变更记录](https://community.crossref.org/t/changes-to-cursors-filtering-and-sorting-in-the-rest-api/16246)及 [官方检索建议](https://www.crossref.org/documentation/retrieve-metadata/rest-api/tips-for-using-the-crossref-rest-api/)。当前来源能力的 `pagination` 为 `cursor_or_offset`。

| 请求含义 | 实际排序 | 分页与覆盖 |
| --- | --- | --- |
| 最新/最早发表 | `published desc/asc` | offset，支持首 10000 条范围；达到边界仍有结果时返回 `capacity`，不改排序或宣称遍历完成 |
| 有文本的相关性 | `score desc` | 游标；Crossref 文本仍需要明确的宽松匹配许可或原生查询 |
| 无文本的默认相关性 | `indexed desc` | 游标，并在来源提示中说明没有文本相关性，实际按索引日期排序 |
| 引用数 | `is-referenced-by-count desc` | 游标 |
| 订阅索引时间窗口 | `indexed asc` | 保留保存的发表日期/类型条件，追加闭合索引时间窗口；实际扫描顺序单独披露 |

不透明续页标识绑定实际计划、页面大小和分页模式；旧发表排序的供应商游标不能被解释成 offset。失效续页在持久订阅中可按原冻结窗口重放，保留已见身份并遵守重试时间。供应商总数变化会显示提示；有效续页存在时，不仅凭“已达到报告总数”提前终止。终页的空游标按终止处理。

本轮公开 Crossref 小范围复核使用两个公共 DOI，验证最新/最早顺序及无文本过滤的索引游标遍历，未读取私有文献或使用供应商凭据。OpenAlex 的公共 ID/初始游标请求也取得预期 ID；这不证明付费更新时间权限或有密钥续页能力。完整范围与源代码/日志校验见 [来源兼容性资格](benchmarks/literature-search-p5-source-compat-linux.json)。
