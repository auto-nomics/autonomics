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
cd crates/tui-http/frontend
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

注意：HTTP API 使用 runtime 配置的文献库路径，默认位于 `~/.autonomics/bib.db`，可用 `AUTONOMICS_BIB_DB` 覆盖。`autonomics-tui bib --db` CLI 子命令仍保持自己的显式路径参数。
