# visualization

项目画布的 SQLite 存储、几何操作和版本服务。Agent 与 GUI 读写同一份正式画布，不存在提案、候选分支、采用/拒绝或候选预览队列。

## 数据与操作

- `Store::create/save/revision/revisions` 管理画布和不可变版本历史。
- `VisualizationService::apply` 对当前版本执行一个原子操作批次；版本冲突或任一操作失败都不产生部分更新。
- `scene::Operation` 支持文字、原生节点、插图、分面、连接线、分组、裁剪、绑定与追加草图等局部编辑。
- `sketch::compile` 将显式科学节点/关系编译为可编辑 Excalidraw 场景。
- `arrange`、`geometry_review` 与 `placement` 提供布局与有界几何检查，不宣称科学正确性或像素级无碰撞。
- `preview` 保存明确 revision 的 PNG；`annotation` 与 `markup` 保留用户的独立反馈。

## HTTP 与 Agent

`api-server::visualization::router` 在宿主鉴权后挂到 `/visualization`：

| 接口 | 行为 |
| --- | --- |
| GET/POST `/projects` | 列出/创建项目 |
| GET/POST `/projects/{project}/figures` | 列出/创建画布 |
| GET/PUT/DELETE `/projects/{project}/figures/{id}` | 读取、保存或删除正式画布 |
| GET `…/head` | 轻量读取当前版本元数据 |
| POST `…/operations` | `{expected_revision, operations}` 原子直接编辑 |
| GET `…/revisions`、`…/revisions/{revision}` | 版本历史 |
| GET/PUT `…/revisions/{revision}/preview` | 指定版本截图 |
| `…/vector-*`、`…/insets` | SVG 与放大图直接编辑 |
| `…/annotations`、`…/markups` | 对象批注与草图标记 |

Agent 工具协议见 [visualization-tools](../visualization-tools/README.md)。所有写操作使用 `expected_revision`；过期写入返回冲突。旧 proposal 路由已删除，旧客户端必须迁移。现有数据库的历史候选表不再被读取或创建，启动过程不会破坏旧数据。

## GUI 同步

打开的画布轮询轻量 head 元数据，发现新版本后更新原生场景，保留视口并记录到 Excalidraw 撤销历史。当前版本截图随同步上传。用户本地编辑静止后自动保存；当远端版本与本地未保存内容同时变化时，保留本地编辑并提示冲突，不自动覆盖。

截图仍由打开的画布渲染；无头渲染不在当前实现内。素材来源与历史版本保留，导出支持 PNG、SVG、Excalidraw 源文件及来源清单。

## 验证

`cargo test -p visualization -p visualization-assets -p visualization-tools`

`cargo test -p api-server --lib direct_tests`

`cd apps/gui && npm run test:canvas:direct`

历史候选生命周期回归已经退役。现行回归验证直接更新、局部内容保留、过期写入拒绝、批次原子性、来源保护、GUI 同步、PNG、撤销与自动保存；底层几何测试独立保留。
