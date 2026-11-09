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

GUI 新画布按项目调用 Gateway API；旧 IndexedDB 记录保留独立入口，可导出后导入项目画布。导出渲染由 GUI 引擎承担。BioArt 检索、插入与 Agent 工具注册已接入。

验证：`cargo test -p visualization`，`cargo clippy -p visualization --all-targets -- -D warnings`。

## 对象编辑服务

`scene::Operation` 提供带 `op` 判别字段的 `add` / `update` / `remove` JSON 协议。
`VisualizationService::apply(project_id, id, expected_revision, operations)` 在指定版本上执行整批操作，成功后保存一个新修订；任何校验或版本冲突均不写入部分结果。

更新为字段级浅合并；禁止修改对象 ID。删除会清理 Excalidraw 的容器、frame、boundElements 和端点绑定。其他对象字段、图片文件和画布设置原样保留。协议不替代渲染引擎的元素规范化，也不检查科学内容正确性。API 层应限制批量大小、文档大小并鉴权。

后续完整工作流仍需：Agent 工具注册、画布选择与批注上下文、BioArt 检索插入、科学内容规划与草图、布局及局部修改预览、端到端验证。当前不能视为已实现完整 Leo 替代品。

## Gateway HTTP 接入

Gateway 在应用数据库同目录打开 `visualizations.sqlite`，启动失败时报告错误，不回退到内存存储。路由挂载于 `/api/v1/visualization`，均要求 Gateway bearer token 或有效 session：

- `GET/POST /projects`：列出、创建项目（创建参数 `id`, `title`）。
- `GET/POST /projects/{project}/figures`：列出、新建画布。
- `GET/PUT/DELETE /projects/{project}/figures/{id}`：读取当前文档、保存、删除。
- `GET /projects/{project}/figures/{id}/revisions/{revision}`：读取历史版本。
- `POST /projects/{project}/figures/{id}/operations`：提交 `expected_revision` 和 `operations`。

写入版本冲突返回 409；不存在的项目/文档返回 404；请求体上限 32 MiB，单批最多 1000 个操作。读取当前文档返回 `{metadata, scene}`。保存参数为 `{expected_revision, title, document}`，新建参数为 `{title, format, document}`。删除参数为 `{expected_revision}`。GUI 已接入项目选择、新建和服务端保存；本地记录采用手动导出/导入迁移。

## Agent 入口与服务端刷新

Runtime 为 agent 注册 `visualization` 工具，支持 `projects/list/read/create/apply`。它与 Gateway 使用同一路径的 SQLite 数据库；`apply` 必须携带 `read` 返回的修订号。每次调用在阻塞线程中打开数据库连接，避免在异步执行器上同步读写。

Agent 只能读取已保存场景。GUI 提供“重新加载服务端版本”，存在未保存修改时要求确认；取消保留当前编辑。暂未自动订阅服务端变更；修改提案可在 GUI 预览并采用。真实 Gateway 认证、版本冲突与基本画布读写已通过回归。

## 科学草图计划

`visualization` 工具新增 `sketch` 操作：传入项目 ID、标题及 `document` 中的结构化 `Plan`，即可创建可编辑画布。Plan 包含 `message`、`audience`、`nodes`、`edges` 和 `assumptions`。节点使用唯一 ID 与不重复的 row/column 网格位置；形状支持 rectangle、ellipse；边可标为 inhibitory。标签、形状、箭头分别生成，证据备注保留在 customData 中。程序只编译明确给出的关系，不推断科学机制。

可运行示例：`cargo run -p visualization --example sketch > /tmp/scientific-sketch.json`。浏览器检查：在 `apps/gui` 运行 `CANVAS_SKETCH_SCENE=/tmp/scientific-sketch.json node tests/canvas-editor.mjs`。图意与假设同时附在首个节点 customData，经过 Excalidraw 原生导出仍可保留。当前仅有规则网格布局，尚未实现医学图标自动匹配或复杂布局优化。

## BioArt 图像素材

画布工具栏的“BioArt 素材”打开 NIH 素材检索面板。Gateway 代理目录检索与最多 8 MiB 的 PNG 下载，不接受调用方指定 URL。导入对象保存来源 URL、作者、许可、文件标识及 SHA-256；图像数据随画布持久化，独立于目录的后续可用性。当前素材作为整体图片编辑，尚不支持 SVG 内部路径编辑、自动图标匹配与替换。

接口：`GET /api/v1/visualization/bioart/search?q=cell&start=0`；`GET /api/v1/visualization/bioart/{id}/image`。目录不可用时返回明确错误，不生成虚构素材或许可证。

Agent 新增 `bioart` 工具：`search` 返回目录结果，`insert` 根据 asset_id、project_id、figure_id 和 expected_revision 下载 PNG 并追加到现有场景。下载后重新检查版本，旧版本调用不写入。插入保留所有其他元素及文件；相同 SHA-256 图片复用文件数据。共享下载适配器验证 PNG 头与尺寸，并限制文件大小。Agent 工具的真实运行仍以完整 runtime 测试结果为准。

## 从画布发起 Agent 需求

项目画布的“Agent 绘图”面板可选择已有 Agent，填写受众与需求。点击“交给 Agent”先保存当前场景，再将项目、画布、观察到的版本和最多 100 个选中对象的摘要追加到该 Agent 草稿中；不覆盖已有草稿，也不自动发送。用户在现有对话中发送后，Agent 可使用 visualization/bioart 工具处理，随后手动重新加载服务端版本。

统一 Tab 上下文另暴露 dirty、revision 和 selectedObjectIds，便于聊天窗口固定绑定或跟随画布。草稿交接包含当前版本预览引用，修改方案可在提案窗口预览和采用。

GUI 保存与 Agent 交接会检查图片文件是否已载入；图片对象缺少 fileId 或对应文件数据时拒绝保存，避免持久化只有占位对象的场景。图片加载失败后可删除该对象或重新导入，再保存。

## 可审阅修改提案

Agent 的 `visualization propose` 使用 title 作为修改摘要，在 expected_revision 上应用 operations，保存候选场景而不改动正式画布。GUI 的“查看 Agent 提案”显示待审阅列表和只读候选画布，支持拒绝或采用。存在本地未保存修改时禁止采用；服务端采用在单个事务内检查基础版本、保存新修订并标记提案为 accepted。重复采用和过期提案返回冲突。

REST：`GET/POST .../figures/{id}/proposals`，创建参数 `{expected_revision,summary,operations}`；`POST .../proposals/{proposal}/accept` 或 `/reject`。提案窗口并排展示基础修订与候选场景，并统计新增、修改、删除对象；尚未提供逐项接受。直接 apply 仍保留，供明确要求直接执行的调用方使用。

绘图工具已独立为 `visualization-tools` crate，runtime 负责注册而不再承载实现。规则草图的箭头和标签使用 Excalidraw 绑定，拖动节点时连接线随动。对象文本更新同步 originalText，并更新引擎元素版本与时间戳。

## 对象批注

“Agent 绘图”面板下的对象批注可将意见关联到 1..100 个已保存对象，并记录观察到的版本。批注独立于 Excalidraw 场景存储，不进入图片或源场景导出；可定位关联对象、标为已处理。被删除的关联对象不会被自动重新创建，定位时会提示对象已不存在。

Agent `read` 返回场景及批注；GUI 交接草稿包含待处理批注。REST `GET/POST .../figures/{id}/annotations`，创建参数 `{expected_revision,element_ids,note}`；`POST .../annotations/{annotation}/resolve` 标记处理完成。添加批注检测版本冲突，不递增画布版本。圈选辅助关联对象，不保存独立的永久手绘笔迹；整图交接见下节。

画布“圈选批注”使用独立 SVG 覆盖层，在当前视口拖动椭圆选择相交对象；支持缩放坐标换算与 Esc 取消。完成后打开批注面板，用户可将意见关联到选中的对象。圈选轨迹是临时交互，不作为正式元素或永久手绘笔迹保存；持久化的是对象关联与批注意见。

真实目录检查：`cargo run -p bioart --example canvas_image -- cell`，会检索并下载首个可导入 PNG，输出尺寸、SHA-256 与上游许可，不写入图像数据或推断许可。

参考图读取：Agent 的 `read` 只提供图片文件元数据，`read_image` 按图片 element_id 返回视觉内容块。支持已保存的 PNG/JPEG/WebP，遵守运行时 6 MiB base64 限额；模型须具备视觉能力。普通场景与提案元数据返回值不携带图片字节，数据库中仍保留完整图片。整张画布预览由 GUI 按修订生成，使用 read_preview 读取。

## 整图视觉交接

GUI 交给 Agent 时，非空画布会先保存，再生成最长边 1280 像素的 PNG（必要时降至 1024），上传到该修订的 preview。截图期间发生编辑或服务端版本变化时拒绝交接；图片不进入正式元素列表。空画布不生成预览。

`PUT/GET .../figures/{id}/revisions/{revision}/preview` 存取预览，上传只接受当前版本及符合尺寸/传输上限的 PNG。旧版预览可按历史版本显式读取；Agent 的 `read_preview` 只读取当前版本，没有预览时明确报错，不回退到旧图。工具通过图像内容块传递整图，实际视觉理解依赖当前模型能力。截图不包含应用工具栏、圈选覆盖层或批注列表，批注以独立结构化上下文传递。


## 草图素材替换

Agent `bioart replace` 接受 asset_id、project_id、figure_id、expected_revision、element_id，下载素材后生成候选文档提案，不直接提交正式画布。目标可以是占位形状、已有素材图片或其语义锚点。素材等比例放入原占位边界，标签独立排列；语义锚点保留原 ID、尺寸和箭头绑定，三者组成可移动组。重复替换复用已有素材对象 ID，保留来源与许可。

当前替换入口为 Agent 工具；GUI 素材面板仍提供直接插入，替换提案通过提案窗口审阅。核心操作 `image::illustrate` 不包含自动图标匹配，也不保证素材与科学概念相符，调用方需核对检索结果。


素材检索结果可先预览再插入，预览不改动正式图稿。Agent 使用 bioart preview 获取视觉内容与来源信息。

Agent 更新节点 x/y 时，关联标签和所属素材同步平移，普通直线箭头端点随动、另一端保持原位。删除带素材的语义锚点会一并删除其所属图片和标签；不会删除同一图中的其他节点。旋转或肘形连接线、复杂尺寸重排仍需显式几何编辑或在 GUI 中调整。

## 历史版本与恢复

项目画布的“历史版本”分页列出版本、标题和时间，选中后按需载入完整场景进行只读预览。恢复使用现有保存接口创建新修订，不改写历史记录；存在本地未保存修改或服务端版本已变化时拒绝恢复。

`GET .../figures/{id}/revisions?limit=20&before=42` 返回不含文档及图片字节的修订元数据，按版本倒序；before 为独占游标，单页最多 100 条。完整历史文档仍通过 `/revisions/{revision}` 读取。

Agent `sketch` 带 figure_id 与 expected_revision 时，在该画布上生成可审阅的整体草图替换提案；省略 figure_id 才创建独立记录。局部修改仍使用 propose，避免用整图草图替换无关内容。
