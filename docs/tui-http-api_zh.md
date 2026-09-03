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
| `GET` | `/api/v1/bib/articles` | 搜索/列出本地库：`query`、`limit`（上限 2000）、`offset`、`sort`（`created_at`\|`updated_at`\|`title`\|`year`）、`order`（`asc`\|`desc`）、`collection_id`、`unfiled=true`；响应含 `total`/`hits`/`articles`/`offset`/`limit` |
| `GET` | `/api/v1/bib/articles/unfiled` | 列出所有未归入任何集合的文献，可选 `limit` |
| `POST` | `/api/v1/bib/articles` | 手动创建文献元数据 |
| `POST` | `/api/v1/bib/articles/upload` | 上传文档文件一步建档：抽取文本后扫描 DOI / arXiv 标识符，命中则经 gateway 拉取真实元数据（离线时落为按标识符索引的存根），未命中创建 `local:{uuid}` 手工条目；重复上传会挂到已有文献上。multipart 字段：`file`（必需）、`category_id`（可选） |
| `POST` | `/api/v1/bib/articles/import` | 按 DOI / PMID / arXiv 等标识符从外部源导入，并可自动获取 OA 全文 |
| `POST` | `/api/v1/bib/articles/import/batch` | 批量导入文献文件：`{"format": "bibtex"\|"ris"\|"csl_json"\|"auto", "content", "category_id"?}`；标识符已存在或批内重复的条目合并进已有 id（`duplicate_details`），缺标题的条目进 `failed`；`auto` 按内容嗅探格式 |
| `GET` / `PUT` / `DELETE` | `/api/v1/bib/articles/{id}` | 查看、更新、删除文献。详情响应为 `{"article", "fulltext", "fulltext_pagination", "annotations"}`——全文元信息在顶层 `fulltext` 字段，不在 Article 内；全文翻页用 `fulltext_pagination` 的 `offset`/`limit`/`next_offset`。所有 `DELETE` 一律 200 + JSON（无 204；删除不存在的文献返回 200 与 `"deleted": false`） |
| `GET` / `POST` / `DELETE` | `/api/v1/bib/articles/{id}/fulltext` | 查看、上传、删除全文；`GET` 支持 `offset` / `limit` 字符分页（默认 100000，最大 500000）；上传为 multipart 字段 `file`，原始文件按 SHA-256 内容寻址保存到文献 VFS，并自动抽取纯文本 |
| `GET` / `HEAD` | `/api/v1/bib/articles/{id}/fulltext/raw` | 流式返回 VFS 中的原始文件，支持单区间 HTTP Range；HTML/PDF 以安全下载语义响应 |
| `GET` / `POST` | `/api/v1/bib/articles/{id}/annotations` | 查看（可选 `?page=N` 过滤）、新增注释；高亮几何放在 `data` JSON 列（`{"rects":[...],"color":...}`） |
| `PUT` / `DELETE` | `/api/v1/bib/annotations/{id}` | 更新（部分更新，显式 `null` 清空字段）、删除注释 |
| `GET` | `/api/v1/bib/articles/{id}/csl-json` | 单篇文献的 CSL JSON（引用引擎直接可用） |
| `GET` / `POST` | `/api/v1/bib/collections` | 列出（扁平）、创建集合（支持 `parent_id` / `sort_order`；分类树由前端自建） |
| `GET` / `PUT` / `DELETE` | `/api/v1/bib/collections/{id}` | 查看、更新（改名/移动/排序共用，`parent_id` 环拒绝 400）、删除集合 |
| `GET` / `POST` | `/api/v1/bib/collections/{id}/articles` | 列出、添加集合成员 |
| `DELETE` | `/api/v1/bib/collections/{id}/articles/{article_id}` | 移除集合成员 |
| `PUT` | `/api/v1/bib/collections/{id}/status` | 更新集合状态 |
| `GET` / `PUT` | `/api/v1/bib/settings` | Web 前端设置，存于 `bib_meta` 的 `web:` 命名空间；PUT 为逐键合并（不覆盖未提及的键） |
| `GET` / `POST` | `/api/v1/bib/chat` | 聊天记录持久化：`?scope={token}` 读取 `{"scope","payload"}`，POST 整体覆盖（`navigator.sendBeacon` 友好；payload ≤ 5 MiB） |
| `GET` | `/api/v1/bib/requests` | 列出待补全文请求 |
| `GET` | `/api/v1/bib/export` | 导出 BibTeX / RIS / Markdown / CSL JSON |
| `GET` | `/api/v1/bib/search/external` | 并发搜索 PubMed、arXiv、bioRxiv、OpenAlex、Crossref、Semantic Scholar |

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

# 上传 PDF 一步建档（自动识别 DOI / arXiv，可指定归类）：
curl -X POST http://127.0.0.1:8765/api/v1/bib/articles/upload \
  -F file=@paper.pdf -F category_id=<collection-id>

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
