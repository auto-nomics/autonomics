# TUI HTTP API

交互式 TUI 启动时会同步启动 HTTP API 后端。该后端不是独立的文献服务，而是后续 TUI REST API 的统一入口；文献管理是第一个业务模块。

## 架构

- 实现位置：`crates/tui-http`
- 路由命名空间：`/api/v1`
- 默认绑定：`127.0.0.1:8765`，只监听本机回环地址
- 地址覆盖：`AUTONOMICS_HTTP_API_ADDR`
- 鉴权：可选设置 `AUTONOMICS_HTTP_API_TOKEN`；设置后所有 `/api/*` 请求需要 `Authorization: Bearer <token>`，静态前端资源不受影响
- 数据库：与 `RuntimeHost` 中的 `SharedInfra.bib` 完全共享同一个 `BibShared`，因此 API、TUI agent 工具和写作系统共享同一 Turso 连接与文献源 HTTP 客户端
- 原文存储：TUI 使用独立文献 VFS 空间 `vfs:///literature/...`；底层默认是 `state_dir/literature/` 本地目录，并自动挂载到 Agent VFS
- 生命周期：TUI 启动完成 `RuntimeHost::open` 后绑定端口；退出时先 graceful shutdown HTTP API，再关闭 agent 和共享基础设施
- 安全边界：默认绑定所有网卡且当前无鉴权。仅在受信网络或有本机防火墙隔离的环境使用；跨不受信网络暴露前必须增加鉴权

新增业务模块时，在该 crate 中实现独立 `Router`，再挂载到 `tui_http::server::api_router` 的 `/api/v1/<module>` 下。不要在各业务模块内自行启动 `TcpListener`。

## 文献 API

| 方法 | 路径 | 功能 |
|---|---|---|
| `GET` | `/api/v1/bib/health` | 文献模块状态、文章数、启用的外部源 |
| `GET` | `/api/v1/bib/articles` | 搜索/列出本地库：`query`、`limit`（上限 2000）、`offset`、`sort`（`created_at`\|`updated_at`\|`title`\|`year`）、`order`（`asc`\|`desc`）、`collection_id`、`unfiled=true`；响应含 `total`/`hits`/`articles`/`offset`/`limit`，`articles` 每行附带 `has_fulltext` 布尔标记（该文献是否已存全文） |
| `GET` | `/api/v1/bib/articles/unfiled` | 列出所有未归入任何集合的文献，可选 `limit` |
| `POST` | `/api/v1/bib/articles` | 手动创建文献元数据 |
| `POST` | `/api/v1/bib/articles/upload` | 上传文档文件一步建档：本地快速抽取只用于扫描 DOI / arXiv 标识符（不做 OCR），命中则经 gateway 拉取真实元数据（离线时落为按标识符索引的存根），未命中创建 `local:{uuid}` 手工条目；重复上传会挂到已有文献上。**PDF 正文走 MinerU 云 API 异步解析**（响应即回，`fulltext.parse_status="pending"`，进度订阅 `parse/stream`，未配 token 直接 400——见下节）；txt / html 仍为同步本地抽取（返回即 `done`/`builtin`）。multipart 字段：`file`（必需）、`category_id`（可选） |
| `POST` | `/api/v1/bib/articles/import` | 按 DOI / PMID / arXiv 等标识符从外部源导入，并可自动获取 OA 全文 |
| `POST` | `/api/v1/bib/articles/import/batch` | 批量导入文献文件：`{"format": "bibtex"\|"ris"\|"csl_json"\|"auto", "content", "category_id"?}`；标识符已存在或批内重复的条目合并进已有 id（`duplicate_details`），缺标题的条目进 `failed`；`auto` 按内容嗅探格式 |
| `GET` / `PUT` / `DELETE` | `/api/v1/bib/articles/{id}` | 查看、更新、删除文献。详情响应为 `{"article", "fulltext", "fulltext_pagination", "annotations"}`——全文元信息在顶层 `fulltext` 字段，不在 Article 内；全文翻页用 `fulltext_pagination` 的 `offset`/`limit`/`next_offset`。所有 `DELETE` 一律 200 + JSON（无 204；删除不存在的文献返回 200 与 `"deleted": false`） |
| `GET` / `POST` / `DELETE` | `/api/v1/bib/articles/{id}/fulltext` | 查看、上传、删除全文；`GET` 支持 `offset` / `limit` 字符分页（默认 100000，最大 500000）；上传为 multipart 字段 `file`，原始文件按 SHA-256 内容寻址保存到文献 VFS。PDF 上传与 `/articles/upload` 同走 MinerU 异步管线（返回 pending），txt / html 同步抽取纯文本（返回即 done） |
| `GET` / `HEAD` | `/api/v1/bib/articles/{id}/fulltext/raw` | 流式返回 VFS 中的原始文件，支持单区间 HTTP Range；HTML/PDF 以安全下载语义响应 |
| `POST` | `/api/v1/bib/articles/{id}/reparse` | 重新解析已有全文（右键「重解析」入口）：fulltexts 行拨回 `pending` 并重新拉起 MinerU 后台任务，进度走 `parse/stream`。无全文 404；源文件不在 VFS（如 open-access 内联文本）400；已有解析在跑 409 |
| `GET` | `/api/v1/bib/articles/{id}/parse/stream` | 解析进度 SSE 流（事件表与懒恢复语义见下方「全文解析」节） |
| `GET` | `/api/v1/bib/fulltext-statuses` | 全量解析状态：`{"statuses":[{article_id, parse_status, parse_engine, parse_error}]}`——列表页并行拉取后在前端按 article_id 倒排 |
| `GET` / `POST` | `/api/v1/bib/articles/{id}/annotations` | 查看（可选 `?page=N` 过滤）、新增注释；高亮几何放在 `data` JSON 列（`{"rects":[...],"color":...}`） |
| `PUT` / `DELETE` | `/api/v1/bib/annotations/{id}` | 更新（部分更新，显式 `null` 清空字段）、删除注释 |
| `GET` | `/api/v1/bib/articles/{id}/csl-json` | 单篇文献的 CSL JSON（引用引擎直接可用） |
| `POST` | `/api/v1/bib/articles/{id}/fetch-metrics` | 手动获取该文献的期刊指标（IF / JCR / 中科院分区）：按期刊名查 EasyScholar 并写入期刊级缓存；阻塞式，响应 `{"journal", "metrics"}`（未命中 / 无 key 时 `metrics` 为 `null`，不报错）。无期刊名 400，文献不存在 404。导入 / 更新路径也会在后台自动富化 |
| `GET` | `/api/v1/bib/journals/metrics` | 列出全部已缓存的期刊指标（`journal_metrics` 表按 `lower(trim(journal))` 键控；前端拉全量后按期刊名匹配注入列表/详情） |
| `GET` | `/api/v1/bib/journals/validate-easyscholar` | 校验 EasyScholar API key：`?key=` 显式校验（设置页保存前预检），缺省校验当前生效的 key（env 或 `web:easyscholar_key` 设置项热替换）；响应 `{"valid", "message"}` |
| `GET` / `POST` | `/api/v1/bib/collections` | 列出（扁平）、创建集合（支持 `parent_id` / `sort_order`；分类树由前端自建） |
| `GET` / `PUT` / `DELETE` | `/api/v1/bib/collections/{id}` | 查看、更新（改名/移动/排序共用，`parent_id` 环拒绝 400）、删除集合 |
| `GET` / `POST` | `/api/v1/bib/collections/{id}/articles` | 列出、添加集合成员 |
| `DELETE` | `/api/v1/bib/collections/{id}/articles/{article_id}` | 移除集合成员 |
| `PUT` | `/api/v1/bib/collections/{id}/status` | 更新集合状态 |
| `GET` / `PUT` | `/api/v1/bib/settings` | Web 前端设置，存于 `bib_meta` 的 `web:` 命名空间；PUT 为逐键合并（不覆盖未提及的键）。含服务端热换键 `easyscholar_key`、`mineru_key`、`mineru_url`（见下节） |
| `GET` / `POST` | `/api/v1/bib/chat` | 聊天记录持久化：`?scope={token}` 读取 `{"scope","payload"}`，POST 整体覆盖（`navigator.sendBeacon` 友好；payload ≤ 5 MiB） |
| `GET` | `/api/v1/bib/requests` | 列出待补全文请求 |
| `GET` | `/api/v1/bib/export` | 导出 BibTeX / RIS / Markdown / CSL JSON |
| `GET` | `/api/v1/bib/search/external` | 并发搜索 PubMed、arXiv、bioRxiv、OpenAlex、Crossref、Semantic Scholar |

### 全文解析（MinerU 云 API，异步）

PDF 上传不在服务端同步抽取正文，而是走 MinerU（`model_version=vlm`）异步流水线，**失败不回退本地解析**：

1. 上传（`/articles/upload` 或 `/articles/{id}/fulltext`）即返回，`fulltexts.parse_status = "pending"`；后台任务读 VFS 源文件 → 上传 MinerU → 轮询页进度 → markdown 落 `text_content`（`done` / `parse_engine="mineru"`）
2. 客户端订阅 `GET /articles/{id}/parse/stream` 收进度；打开流会**懒恢复**重启后丢失的任务（行状态 pending/processing 且无活跃任务时重新拉起，重新上传 MinerU）
3. 进程重启后刷新页面即可续上（同上懒恢复）；失败行 `failed` + `parse_error`，可 POST reparse 重试

状态机：`pending → processing → done | failed`（旧库行缺省 `done`；txt/html 同步路径落 `done`/`builtin`）。

`parse/stream` 事件（与 agent 聊天同一 SSE 序列化，data 为 snake_case、不含文章 id）：

| 事件 | data | 说明 |
|---|---|---|
| `progress` | `{"percent","stage"}` | 上传 5%；页进度 10–90% 带「解析中 x/y 页」 |
| `done` | `{"parse_engine","markdown_length"}` | 终态，流关闭 |
| `error` | `{"message"}` | 业务失败终态（客户端不应重连），流关闭 |
| `ping` | `{}` | 每 10 秒心跳 |

流打开时解析已结束的，会从 DB 快照合成一个 done/error 终态事件再关流——晚到的订阅者不会只收到心跳。

**Token 配置**（无 token 上传 PDF 直接 400）：

```bash
# 运行时热换（存 bib_meta `web:` KV，PUT /settings 逐键合并，优先生效）：
curl -X PUT http://127.0.0.1:8765/api/v1/bib/settings \
  -H 'Content-Type: application/json' \
  -d '{"settings":{"mineru_key":"<token>","mineru_url":"https://mineru.net"}}'

# 或进程环境变量（启动种子，settings 未覆盖时兜底）：
export MINERU_API_TOKEN=<token>
export ENDPOINT_MINERU_URL=https://mineru.net   # 缺省即此值
```

限制：文件 ≤ 50MiB（沿用 `MAX_UPLOAD_BYTES`）；MinerU 侧申请 5min / 上传 10min / 轮询 30s（5s 间隔）/ 下载 5min / 单任务总时限 10min。

### Agent 聊天（SSE）

`POST /api/v1/agent/chat` — agentik 引擎的 HTTP 入口。模型与工具集由服务端持有（TUI 配置的活动模型 + 文献工具 `bib_all_registrations`），请求体中的 `model_config` / `tools` 被忽略，**API key 永不出现在浏览器**。请求体：`{"message", "agent_type"?, "messages"?（历史), "system_prompt"?}`。未配置模型时返回 503。

响应为 `text/event-stream`，事件与 jayread 前端协议一致：

| 事件 | data | 说明 |
|---|---|---|
| `text_delta` | `{"text"}` | 增量回答文本 |
| `tool_call_start` | `{"tool_use_id","name","input"}` | 工具调用开始（后台任务 id 为 `bg-{seq}`） |
| `tool_call_result` | `{"tool_use_id","is_error","preview"}` | 工具结果预览（截断至 500 字符） |
| `done` | `{}` | 回合结束，流关闭 |
| `error` | `{"message","error_code"}` | 出错并关闭流 |
| `ping` | `{}` | 每 10 秒一次的命名事件心跳（客户端靠解析事件重置空闲计时器） |

客户端中断（关闭流）会自动取消 agent 回合。`agent_type` 取 `homepage`（文献库助手，默认）/ `paperReader`（围绕当前文献）/ `screening`（筛查向），决定系统提示身份；`system_prompt` 字段追加纸面上下文。

全局端点：

```text
GET /api/health
GET /api/v1        # 模块列表 ["bib","agent"]（未配置模型时 agent 不挂载）
```

## 前端

Rust 后端在根路径直接托管文献管理前端（autonomics-web，React 18 + Vite + antd），无需另起前端服务：

```text
http://<本机IP>:8765/
```

前端源码在 `apps/web`（pnpm workspace，含 `vendor/citation-engine` 与 `vendor/pdfium-viewer` 两个本地包）。构建：

```bash
scripts/build-web.sh          # = pnpm --dir apps/web install && pnpm build
```

构建产物 `apps/web/dist` 入 git（与旧惯例一致），由 `crates/tui-http/src/frontend.rs` 通过 rust-embed 托管：release 构建把 dist 嵌入二进制，debug 构建直接从磁盘读取（改前端只需重跑构建脚本，不必重编 Rust）。哈希化的 `/assets/*`、`/wasm/*` 返回 `Cache-Control: immutable`；未命中的非 API GET 回落 `index.html`（SPA 路由），未命中的 `/api/*` 保持 JSON 404。

开发时保持 Rust TUI/HTTP 后端运行，然后：

```bash
pnpm --dir apps/web dev
```

Vite 在 `http://127.0.0.1:5173` 启动开发服务并把 `/api` 代理到 Rust 后端（SSE 不缓冲）；后端端口可用 `API_PORT` 覆盖（默认 8765）。

## 桌面版

`apps/desktop` 是同一套前端的 Tauri v2 壳（仅 Linux 打包）：壳进程内嵌 tui-http（`BibShared` + 模型槽 → `ApiRouterBuilder` → 随机端口），窗口直接导航到内嵌服务地址——前端与浏览器模式完全同一份代码（同源 fetch，SSE 无跨域）。模型槽启动时从共享 `config.db` 读一次（模型仍由 TUI 配置；改模型后桌面版需重启）。

数据与 TUI 完全共享（`~/.autonomics`：bib.db / config.db / vfs.toml），尊重所有 `AUTONOMICS_*` 环境变量；唯一差异是 `AUTONOMICS_DATA_DIR` 未设置时默认 `state_dir/data`（TUI 默认是开发机路径）。与 TUI 同时运行是安全的（bib.db 多进程 WAL + busy_timeout），但建议避免长时间双开狂写。

```bash
# 开发（窗口弹出，Rust 改动需重启；前端改动只需重跑 build-web.sh 后 Ctrl+R）
cargo build -p autonomics-desktop && ./target/debug/autonomics-desktop

# 打包（前端 dist 新鲜化 → release 编译 → deb/appimage 产物在 target/release/bundle/）
scripts/build-desktop.sh
```

注意：`tauri.conf.json` 故意不配 beforeBuildCommand，dist 新鲜化由 `build-desktop.sh` 保证；裸跑 `cargo tauri build` 会嵌入磁盘上现有的 dist（可能过期）。脚本会设置 `NO_STRIP=true`（Arch 上 linuxdeploy 自带的旧 strip 不认识新 binutils 的 `.relr.dyn` 段会致命失败）；appimage 打包最后一步需联网从 GitHub 下载 AppImage type2 runtime（已有缓存 `~/.cache/tauri/` 时跳过）。

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

# 上传 PDF 一步建档（自动识别 DOI / arXiv，可指定归类；正文走 MinerU 异步）：
curl -X POST http://127.0.0.1:8765/api/v1/bib/articles/upload \
  -F file=@paper.pdf -F category_id=<collection-id>

# 重解析已有全文 + 订阅进度（SSE）：
curl -X POST http://127.0.0.1:8765/api/v1/bib/articles/<article-id>/reparse
curl -N http://127.0.0.1:8765/api/v1/bib/articles/<article-id>/parse/stream

# 批量导入 .bib（format 亦可 ris / csl_json / auto）：
curl -X POST http://127.0.0.1:8765/api/v1/bib/articles/import/batch \
  -H 'Content-Type: application/json' \
  -d '{"format":"bibtex","content":"@article{a, title={A}, doi={10.1/x}}"}'

# agentik 聊天（SSE）：
curl -N -X POST http://127.0.0.1:8765/api/v1/agent/chat \
  -H 'Content-Type: application/json' \
  -d '{"message":"帮我检索 CRISPR 筛选综述"}'
```

注意：HTTP API 使用 runtime 配置的文献库路径，默认位于 `~/.autonomics/bib.db`，可用 `AUTONOMICS_BIB_DB` 覆盖。`autonomics-tui bib --db` CLI 子命令仍保持自己的显式路径参数。
