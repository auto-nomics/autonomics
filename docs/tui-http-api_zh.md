# TUI HTTP API

交互式 TUI 启动时会同步启动一个仅监听本机的 HTTP API 后端。该后端不是独立的文献服务，而是后续 TUI REST API 的统一入口；文献管理是第一个业务模块。

## 架构

- 实现位置：`crates/tui-http`
- 路由命名空间：`/api/v1`
- 默认地址：`http://127.0.0.1:8765`
- 地址覆盖：`AUTONOMICS_HTTP_API_ADDR`
- 数据库：与 `RuntimeHost` 中的 `SharedInfra.bib` 完全共享同一个 `BibShared`，因此 API、TUI agent 工具和写作系统共享同一 Turso 连接与文献源 HTTP 客户端
- 生命周期：TUI 启动完成 `RuntimeHost::open` 后绑定端口；退出时先 graceful shutdown HTTP API，再关闭 agent 和共享基础设施
- 安全边界：默认仅监听 loopback。当前无鉴权；显式绑定 `0.0.0.0` 或局域网地址前需要自行增加网络层隔离或鉴权

新增业务模块时，在该 crate 中实现独立 `Router`，再挂载到 `tui_http::server::api_router` 的 `/api/v1/<module>` 下。不要在各业务模块内自行启动 `TcpListener`。

## 文献 API

| 方法 | 路径 | 功能 |
|---|---|---|
| `GET` | `/api/v1/bib/health` | 文献模块状态、文章数、启用的外部源 |
| `GET` | `/api/v1/bib/articles` | 搜索本地库，支持 `query`、`limit` |
| `POST` | `/api/v1/bib/articles` | 手动创建文献元数据 |
| `POST` | `/api/v1/bib/articles/import` | 按 DOI / PMID / arXiv 等标识符从外部源导入，并可自动获取 OA 全文 |
| `GET` / `PUT` / `DELETE` | `/api/v1/bib/articles/{id}` | 查看、更新、删除文献 |
| `GET` / `POST` / `DELETE` | `/api/v1/bib/articles/{id}/fulltext` | 查看、上传、删除全文；上传为 multipart 字段 `file`，自动抽取纯文本 |
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

## 示例

```bash
curl http://127.0.0.1:8765/api/v1/bib/articles?query=gwas&limit=20

curl -X POST http://127.0.0.1:8765/api/v1/bib/articles/import \
  -H 'Content-Type: application/json' \
  -d '{"id_type":"doi","id":"10.1038/s41586-023-06236-2"}'

curl -X POST http://127.0.0.1:8765/api/v1/bib/articles/<article-id>/fulltext \
  -F file=@paper.pdf
```

注意：HTTP API 使用 runtime 配置的文献库路径，默认位于 `~/.autonomics/bib.db`，可用 `AUTONOMICS_BIB_DB` 覆盖。`autonomics-tui bib --db` CLI 子命令仍保持自己的显式路径参数。
