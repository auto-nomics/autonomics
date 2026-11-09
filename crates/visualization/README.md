# visualization

按项目组织可视化文档的 Rust 存储核心。与 GUI、Agent 和渲染引擎解耦。

- `Project.id` 由宿主系统提供，可复用现有项目 ID；标题仅作展示。
- `Visualization` 是文档元数据，每条记录属于一个项目；所有文档读写均带项目 ID。
- `format` 为版本化格式名（如 `excalidraw/v2`），`Revision.document` 为完整 JSON，包括引擎原生场景及其内嵌图片。
- 新建记录从 revision 1 开始；每次保存原子更新元数据并新增不可变修订。
- 保存和删除必须提供预期 revision；过期写入返回 `Error::Conflict`。
- `list` 只返回元数据，`revision` 按需读取场景。恢复历史可读取旧 revision，再以当前 revision 调用 `save`，保留恢复轨迹。
- 删除记录会级联删除其修订；不提供隐式清理历史操作。

```rust,no_run
use visualization::Store;
use serde_json::json;

let mut store = Store::open("visualizations.sqlite")?;
store.create_project("research-project-id", "细胞信号研究")?;
let figure = store.create(
    "research-project-id", "信号通路", "excalidraw/v2",
    &json!({"type":"excalidraw", "version":2, "elements":[], "files":{}}),
)?;
let scene = store.revision(&figure.project_id, &figure.id, figure.revision)?;
store.save(&figure.project_id, &figure.id, figure.revision, "信号通路修订", &scene.document)?;
# Ok::<(), visualization::Error>(())
```

`Store` 使用同步 rusqlite；异步服务应在数据库工作线程或 `spawn_blocking` 中使用，避免阻塞执行器。数据库父目录由调用方创建。项目隔离是数据模型约束，服务层仍需验证调用方的项目访问权限。

当前交付存储 crate；GUI 的 IndexedDB 尚未迁移，也未新增 HTTP API。后续服务适配应将画布创建/保存/列表接入本 crate，并显式导入现有浏览器记录。暂不包含导出渲染、BioArt 素材索引或 Agent 修改协议。

验证：`cargo test -p visualization`，`cargo clippy -p visualization --all-targets -- -D warnings`。
