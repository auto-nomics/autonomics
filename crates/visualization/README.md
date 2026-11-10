# visualization

按项目组织可视化文档的 Rust 存储核心。与 GUI、Agent 和渲染引擎解耦。

完整目标、当前证据和未完成项见 [科研绘图完成标准](ROADMAP.md)。

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

已接入 Agent 工具、画布选择和对象批注、BioArt 检索与替换、结构化科学草图、修改提案预览及版本历史。当前已有网格及水平/垂直分层布局，科学内容规划仍依赖 Agent 与用户澄清，尚不能视为完整 Leo 替代品。

## Gateway HTTP 接入

Gateway 在应用数据库同目录打开 `visualizations.sqlite`，启动失败时报告错误，不回退到内存存储。路由挂载于 `/api/v1/visualization`，均要求 Gateway bearer token 或有效 session：

- `GET/POST /projects`：列出、创建项目（创建参数 `id`, `title`）。
- `GET/POST /projects/{project}/figures`：列出、新建画布。
- `GET/PUT/DELETE /projects/{project}/figures/{id}`：读取当前文档、保存、删除。
- `GET /projects/{project}/figures/{id}/revisions/{revision}`：读取历史版本。
- `POST /projects/{project}/figures/{id}/operations`：提交 `expected_revision` 和 `operations`。

写入版本冲突返回 409；不存在的项目/文档返回 404；请求体上限 32 MiB，单批最多 1000 个操作。读取当前文档返回 `{metadata, scene}`。保存参数为 `{expected_revision, title, document}`，新建参数为 `{title, format, document}`。删除参数为 `{expected_revision}`。GUI 已接入项目选择、新建和服务端保存；本地记录采用手动导出/导入迁移。

## Agent 入口与服务端刷新

Runtime 为 agent 注册 `visualization` 工具，支持 `projects/list/read/read_image/read_preview/create/sketch/propose/apply`。它与 Gateway 使用同一路径的 SQLite 数据库；`apply` 必须携带 `read` 返回的修订号。每次调用在阻塞线程中打开数据库连接，避免在异步执行器上同步读写。

Agent 只能读取已保存场景。GUI 提供“重新加载服务端版本”，存在未保存修改时要求确认；取消保留当前编辑。暂未自动订阅服务端变更；修改提案可在 GUI 预览并采用。真实 Gateway 认证、版本冲突与基本画布读写已通过回归。

## 科学草图计划

`visualization` 工具新增 `sketch` 操作：传入项目 ID、标题及 `document` 中的结构化 `Plan`，即可创建可编辑画布。Plan 包含 `message`、`audience`、`nodes`、`edges` 和 `assumptions`。节点使用唯一 ID 与不重复的 row/column 网格位置；形状支持 rectangle、ellipse；边可标为 inhibitory。标签、形状、箭头分别生成，证据备注保留在 customData 中。程序只编译明确给出的关系，不推断科学机制。

可运行示例：`cargo run -p visualization --example sketch > /tmp/scientific-sketch.json`。浏览器检查：在 `apps/gui` 运行 `CANVAS_SKETCH_SCENE=/tmp/scientific-sketch.json node tests/canvas-editor.mjs`。图意与假设同时附在首个节点 customData，经过 Excalidraw 原生导出仍可保留。支持显式网格及自动水平/垂直布局，见下方自动构图说明。尚未实现医学图标自动匹配。

## BioArt 图像素材

画布工具栏的“BioArt 素材”打开 NIH 素材检索面板。Gateway 代理目录检索与最多 8 MiB 的 PNG 下载，不接受调用方指定 URL。导入对象保存来源 URL、作者、许可、文件标识及 SHA-256；图像数据随画布持久化，独立于目录的后续可用性。当前素材作为整体图片编辑，尚不支持 SVG 内部路径编辑或自动图标匹配；Agent 可使用 replace 提交素材替换提案。

接口：`GET /api/v1/visualization/bioart/search?q=cell&start=0`；`GET /api/v1/visualization/bioart/{id}/image`。目录不可用时返回明确错误，不生成虚构素材或许可证。

Agent 新增 `bioart` 工具：`search` 返回目录结果，`insert` 根据 asset_id、project_id、figure_id 和 expected_revision 下载 PNG 并追加到现有场景。下载后重新检查版本，旧版本调用不写入。插入保留所有其他元素及文件；相同 SHA-256 图片复用文件数据。共享下载适配器验证 PNG 头与尺寸，并限制文件大小。Agent 工具的真实运行仍以完整 runtime 测试结果为准。

## 从画布发起 Agent 需求

项目画布的“Agent 绘图”面板可选择已有 Agent，填写受众与需求。点击“交给 Agent”先保存当前场景，再将项目、画布、观察到的版本和最多 100 个选中对象的摘要追加到该 Agent 草稿中；不覆盖已有草稿，也不自动发送。用户在现有对话中发送后，Agent 可使用 visualization/bioart 工具处理，随后在画布预览和采用修改提案；直接执行的修改则需重新加载服务端版本。

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

## 原生 GUI 集成检查

在 `apps/gui` 执行 `npm run test:canvas:native`。脚本编译 `api-server` 的 `visualization_native` 示例，启动临时 SQLite、真实可视化 HTTP 路由及真实注册的 Agent 工具，再由 Playwright 操作 GUI：创建项目和画布、草图提案预览及采用、整图预览交接、局部文字修改、历史恢复及 SVG 导出。

这项检查的模型决策使用确定性样例，其他工作台 API 使用测试数据；可视化 API、数据库、工具实现和画布引擎均使用实际代码。它验证集成协议，不验证真实模型的科学推理或绘图质量。Gateway 鉴权由独立 Gateway 集成测试覆盖。

示例仅绑定回环地址，测试工具入口 `/test/visualization` 不属于生产 Gateway 路由。可通过 `VISUALIZATION_NATIVE_BIN` 指定已编译的示例，再直接运行 `node tests/canvas-native.mjs`。截图输出到 `/tmp/autonomics-canvas-native.png`，失败截图为 `/tmp/autonomics-canvas-native-failure.png`。

## 持久化草图标记

“草图标记”打开独立审阅画布，可补画、划除或圈出强调区域，附上修改说明后保存。笔迹使用场景坐标，浏览与缩放时重新投影；审阅截图使用相同的场景几何合成。正式文档、图稿版本和常规源文件/SVG/PNG 导出不包含这些笔迹。草稿关闭及页面退出具有未保存保护。

每组记录保存 `at_revision`、说明、笔迹及最长边不超过 1280 的 PNG 合成预览。创建仅接受当前图稿版本，版本冲突不写入；记录后续保留原始版本与截图，不跟随新图稿移动。已保存记录可查看并标为完成。列表显示最近 50 组摘要，详细几何和截图按 ID 读取。

REST：`GET/POST .../figures/{id}/markups`；创建参数 `{expected_revision,note,strokes:[{kind:"draw"|"cross"|"emphasis",points:[[x,y],...]}],dataURL}`。`GET .../markups/{markup}` 读取完整记录；`POST .../markups/{markup}/resolve` 标记完成。每组最多 100 笔、总计 10000 个点；补画每笔最多 2000 点，划除与强调使用两个边界点。

Agent `read` 返回标记摘要，不返回截图字节。`read_markup` 要求 markup_id，以图像内容块返回带笔迹的图稿，文本块附说明、几何、at_revision、current_revision 和 historical。旧版标记是显式历史审阅，不作为当前整图预览的回退。Agent 需核对版本和科学意图，再提出正式图稿修改。GUI 交接草稿附带最近记录中的待处理标记；无本地修改时不额外保存图稿，保持标记与交接版本一致。

## 自动构图

科学计划支持 `layout: "grid" | "horizontal" | "vertical"`。默认 grid 保留原有行为，要求每个节点显式给出唯一 column/row；horizontal 和 vertical 要求省略网格坐标，根据已提供的关系生成分层布局。自动模式不会忽略用户坐标，也不推断缺失关系。

自动布局先处理强连通分量，再按显式关系分层；环内以节点输入顺序决定布局上的前向关系，所有反馈关系仍保留为箭头。分支使用前后重心排序减少可避免的交叉，跨层、反馈和并行关系走外侧通道。长标签按词边界换行，长词与中文可逐字换行；自动模式按标签高度增加节点及层间距离。节点、标签和每条连接线保持独立可编辑，来源关系、证据和假设保留在 customData。

示例：`cargo run -p visualization --example sketch -- vertical > /tmp/vertical-sketch.json`。Agent 可在 sketch.document 中指定 layout，无需计算网格坐标；对现有画布仍须传入 figure_id 和 expected_revision，先生成提案供用户审阅。

这些布局不等同于完整的图形排版系统：目前没有分面、解剖背景内嵌图或任意障碍物路由；密集网络仍可能出现连线之间的交叉。网格模式尊重原始坐标，不执行自动避障。需要用户核对图意、顺序和科学关系。

自动布局验证包含分支、反馈环、并行关系、跨层避障及中英文长标签；`cargo test -p visualization --test layout_edits` 检查移动反馈端点后中段标签保持正确位置。`npm run test:canvas:native` 额外通过真实工具生成三类图稿，检查 Excalidraw 载入后的标签边界、连线避障、首次视口适配及 SVG 导出，并在 GUI 审阅和采用反馈端点移动提案。

新图稿的拉丁字符使用与浏览器相同的本地字体度量，减少过早换行；中文及复杂文字整形仍需进一步统一。首次载入适配会预留浮动工具栏空间，后续平移缩放不被覆盖；超出引擎最小缩放范围的大图仍需平移查看。

## 可迭代的科研绘图计划

画布“绘图计划”保存核心图意、受众、构图、组件、关系、依据、假设及待澄清问题/答案。用户可在表单中编辑，也可交给 Agent 使用 `read_plan` / `save_plan` 更新。计划按独立 version 保存，并保留不可变历史；计划自身的保存不递增图稿修订。GUI 遇到未保存图稿时，会先保存图稿，再将计划关联到该修订。

缺少图意、受众、组件、关系说明，存在未回答的问题，或结构无法编译时，状态为 needs_clarification；计划仍可保存，供继续讨论。多个组件没有边时，必须明确填写仅并列展示等关系说明。字段齐备后状态为 ready，但这只表示结构可生成，不表示科学内容正确。问题答案不会被程序自动解释成科学关系；应核对组件和关系是否反映答案。

点击“生成草图提案”或调用 `sketch_plan`，会从确切的计划版本与画布版本编译候选场景，立即进入现有提案预览流程；采用前正式图稿保持不变。当前计划的同一版本重复生成会复用其尚未处理的提案。采用、编辑图稿或重载新版本后，旧计划需核对并重新保存到当前图稿版本，才能再次生成。已存在计划的画布不允许通过 Agent 直接 sketch 绕过计划记录；明确的新图创建及普通局部 propose 不受此限制。

Agent 输入：save_plan 要求 project_id、figure_id、expected_revision、planning_version（新计划为 0）及 document；sketch_plan 要求上述版本及 title 作为提案摘要。read_plan 返回完整计划及 issues；普通 read 只附计划摘要。GUI 交接附计划版本、基于的图稿版本及待澄清项，Agent 应读取最新完整计划并保留用户答案。过期写入返回冲突，不能覆盖其他讨论轮次的新答案。

REST：`GET/PUT .../figures/{id}/planning`；保存参数 `{expected_revision,expected_plan_version,document}`。`GET .../planning/revisions/{version}` 读取历史；`POST .../planning/sketch` 参数 `{expected_revision,expected_plan_version,summary}`。document 字段为 message、audience、layout、nodes、edges、assumptions、relationship_note、questions；questions 为 `{id,question,answer}`，组件与问题标识须唯一，计划总 JSON 上限 256 KiB。

验证覆盖空白/未回答计划不能生成、答案和历史保留、双版本冲突、独立连接并发写入、重复生成复用、项目隔离及图稿删除级联清理。原生 GUI 检查通过真实注册工具保存问题、表单填写答案、重载、生成/预览/采用，再交接计划摘要。模型决策仍使用固定样例，不能据此宣称真实模型的澄清质量或科学推理已验收。

## 真实模型评估

在 `apps/gui` 运行 `VISUALIZATION_LIVE_PROVIDER=openai VISUALIZATION_LIVE_MODEL=gpt-6.1-sol npm run test:canvas:live`。需本机已配置相应模型；测试使用其真实服务与额度。配置数据库只读打开，图稿和 Agent 对话使用独立临时环境，不修改日常项目。也可显式指定其他支持视觉与工具调用的 provider/model；成功不保证跨模型兼容。

测试使用真实 AgentCore、注册工具、NIH BioArt 服务、Rust API、SQLite 与浏览器画布。工作区外围 API 使用测试数据，未覆盖完整生产 Gateway 会话、提示词或聊天流渲染。前四轮检查包括：先保存澄清问题不画图；吸收答案并保留问答历史；通过整图预览仅修改指定标签；检索、预览并提议替换真实 BioArt 素材。第五轮通过 GUI 在素材上画划除批注，交接文字不提供组件名称，要求模型读取对应批注图并提出仅移除该组件、相关箭头和关系文字的提案，保留剩余对象及位置，不补新关系。每次提案都由测试用户在 GUI 审阅采用，正式图稿在采用前保持不变。

2026-11-10，GPT gpt-6.1-sol 通过上述四轮流程，证据目录 `/tmp/autonomics-canvas-live-mXCzw5`；MiniMax minimax-m3.1-flash-preview 未通过。新增第五轮的运行证据为 `/tmp/autonomics-canvas-live-zFOIqg`：前四轮通过，GUI 划除笔迹准确覆盖受体，模型已读取同版本整图与批注图；随后服务返回 `usage_limit_reached`，未生成删除提案，因此第五轮尚未通过。四轮成功仅证明这个三组件概念图样例，不能证明任意生物学机制、复杂构图或批注视觉理解。输出目录保留 `result.json`、公开工具轨迹、版本化场景和截图；不记录模型内部推理，轨迹会脱敏凭据与图像字节。临时目录中的证据需另行归档才可跨机器复核。

删除操作遵循场景对象语义：素材化锚点会级联删除其图片及标签，不能再次提交这些子对象 ID；连接箭头仅解除绑定。普通形状或箭头删除后，其文字仍保留为独立对象。若要求移除组件及关系，应明确删除相关箭头和箭头文字，不应自动补画跨越被删除组件的新关系。

## 多面板比较图

计划可声明 `panels: [{id,title,column,row}]`，并在每个节点上指定 `panel`。最多 12 个面板；面板 ID 与位置均须唯一，column/row 范围为 0..10。组件 ID 在全图中唯一，组件必须引用已声明面板，每个面板至少有一个组件。未声明 panels 的旧计划保持原有行为。

各面板按同一 layout 独立生成内部图稿，网格节点的 row/column 在所属面板内解释；面板间距根据各行列的实际内容尺寸计算，标题按可用宽度换行。显式跨面板关系生成场景级连接线，绑定实际组件而非面板边框，不受某个 frame 的裁剪。路径通过带转弯代价的正交可见性网格搜索，避开节点、内部关系文字及面板标题；标签选择未遮挡的位置，必要时使用外侧通道，反馈、抑制及平行关系保留原始方向和依据。不会推断新的跨面板关系。

输出使用 Excalidraw 原生 frame，形状、节点文字、关系箭头、关系文字和面板标题均为独立对象，通过 frameId 归属面板。标题在图稿内部，包含在 SVG/PNG 导出中。Agent 更新 frame 的 x/y 会整体平移其内容，并更新外部箭头的相应端点，内部箭头不会重复移动；删除 frame 会保留内容并解除归属。素材替换继承原组件的 frameId，移动面板会同时移动其图片与标签。调整 frame 尺寸不会自动缩放或重新布局内容。

“绘图计划”表单支持添加/移除面板、编辑标题及行列、指定组件面板。移除面板后原组件需重新分配，或移除全部面板后回到单图；不会自动删除组件或推断关系。生成仍走版本化提案与采用流程。

`tests/panels.rs` 覆盖全局标识与绑定、非法结构、素材化面板平移、其他面板保留及计划版本；`test:canvas:native` 覆盖表单分配、重载、提案采用、原生 frame 载入、Agent 整面板移动、原生 frame 标题拖动及保存重载、SVG 标题导出。跨面板连接另外覆盖正向、反馈、平行、标签原生中点与 frame 位移的回归。原生拖动会按编辑器规则沿节点边界重新计算绑定落点；Agent 平移保留端点相对节点的位置。移动之后不自动改写整条路径。可点击“整理跨面板连线”根据当前对象位置重新避障，预览采用后生效；连线之间仍可能交叉。真实模型的多面板构图选择尚未验证。

### 移动后整理跨面板连线

“整理跨面板连线”先保存当前画布，为现有的生成式跨面板连接线批量提交 `{op:"reroute",id}` 提案，自动打开现有前后预览。采用前正式图稿版本保持不变；没有符合条件的连线时明确提示。整理期间本地画布变化时，提案保留在列表，须重新核对版本；采用仍受未保存修改及服务端修订检查保护。

Agent 也可将 reroute 放在 propose.operations 中，跟在移动操作之后；操作按顺序执行，任一失败则整批回滚。整理读取当前绑定对象与当前标签实际尺寸，不从旧科研计划重新生成组件，不修改文字、字体、颜色、箭头样式、方向、依据或其他对象。当前路由会避开形状、素材图片、文字与旋转障碍物的保守包围盒；箭头必须为无旋转的场景级跨面板箭头，绑定组件须为无旋转矩形或椭圆，标签须无旋转且具有有效尺寸。端点已解除绑定、对象几何无效或存在多个关系标签时返回可说明的错误，避免静默丢失关联。端口位置会按新路径重新选择，ID 与绑定对象保持不变。

已有的普通或手绘箭头不会被批量按钮改写；已生成的跨面板箭头即使经过手动改线，仍会在该按钮的提案范围内。用户可逐项使用 Agent reroute 指定连接线，或拒绝整份提案保留原线路径。单元测试覆盖当前障碍物、标签样式、无关对象保留及失败原子性；原生 GUI 回归覆盖添加障碍物、提案前正式版本不变、非法整理不写入、采用及载入后的路径避障。

## 固定字体与字宽测量

新建画布及生成的图稿使用 Excalidraw `fontFamily: 9`（Liberation Sans）。字体以本地 WOFF2 随前端发布，包含 SIL Open Font License 及原始版权说明；不依赖用户操作系统是否安装 Helvetica，也不依赖远程字体服务。旧图稿的字体字段保持原值；整理连接线不会替换已有文字样式。

后端 `font_metrics.rs` 从实际发布的同一字体文件生成，包含 2327 个 Unicode 字符的 advance 与 908 对 kerning，并保存 SHA-256；自动构图的 layoutInfo 记录字体与校验值。换行按整词尝试，过长的单词再拆分；单行宽度加入相邻字符的字距调整。关系文字按可用连接长度换行，保留可辨认的箭身。

更新字体后，在仓库根目录运行 `python3 crates/visualization/scripts/generate-font-metrics.py`，需要开发工具 `fonttools[woff]`；运行时不需要 Python。字体、许可位于 `apps/gui/public/excalidraw/fonts/Liberation/`，GUI 构建将其一起同步到 Gateway。不要仅更新前端字体而保留旧的后端度量。

原生浏览器回归阻断远程 WOFF2 请求，验证字体从本机加载，并比较样例中拉丁/希腊关系文字的后端字宽与 Canvas 实测宽度（误差小于 1 像素），核对字体文件 SHA-256、英文词组不被过早换行，以及独立 SVG 包含内嵌 WOFF2。中英文长标签仍检查载入后的边界。

这不是完整 Unicode 整形引擎：该字体未覆盖的字符继续采用保守宽度估计，并由浏览器系统字体回退；中文、emoji、双向文字及复杂组合字形尚不能保证跨机器逐像素一致。SVG 内嵌字体只解决所覆盖字符的字体依赖，未覆盖字符仍需要查看端的合适回退字体。

## 开放矢量素材

画布“矢量素材”面板接入 BioIcons 的逐素材目录，可搜索、预览并插入规范化 SVG。BioArt 与矢量素材插入优先使用视口内空白位置，不移动已有对象；视口已满时放到图稿旁的空白处并定位查看。图片对象保留作者、许可、来源、目录哈希、原文件与规范化文件哈希及转换说明。文字标记使用固定字体转为路径，PNG 仅用于预览，保存和 SVG 导出保留矢量源。内部路径目前仍作为同一图片对象，不支持逐路径拖动；现已支持单实例的精确颜色替换。

Agent 新增 `bioicons` 工具：search、preview、insert、replace；insert/replace 均创建版本明确的提案，采用前不改正式图稿，替换沿用现有语义锚点与标签/连线保护。`visualization read_image` 对保存的 SVG 返回 PNG 视觉结果，不把 SVG 字符串作为模型可视图片，也不向文本上下文泄漏矢量/PNG 字节。

实现与支持范围见 [bioicons README](../bioicons/README.md)。官方目录中许可不同，不以站点代码的 MIT 许可代替素材许可。未知许可不自动推断为公共领域；转换记录跟随对象保存。当前可拒绝包含外部资源、位图或无法可靠转换文字的 SVG。

## 矢量素材配色

选中一个 SVG 素材或其语义锚点，点击“素材编辑”，从实际 SVG 读取颜色与原图预览。选择替换颜色后点击“预览配色提案”，核对前后图再采用。配色只改变选中图片实例；同一 SVG 的相同颜色填充、描边和渐变色会一起替换，文字标记的轮廓、透明度及对象位置保持不变。每次支持 1..32 个 `#RRGGBB` 映射；PNG 不支持内部换色。

旧共享文件保持不变，新实例引用新的内容哈希文件，其他实例保留原色。来源、作者、许可与用户注释保留，附加修改前文件 ID、前后哈希、颜色映射及时间记录。未采用的提案不改正式图稿；历史修订保留原色。颜色面板和提案生成均检查未保存修改与版本冲突。

API 与 Agent 共用 [visualization-assets](../visualization-assets/README.md)；`bioicons palette/recolor` 对已保存的 SVG 工作，无需目录下载。工具 palette 返回实际颜色和 PNG 视觉预览，recolor 生成提案。当前按颜色值替换，不支持单独选择同色路径；逐路径编辑仍待实现。

配色要求 SVG 内文字已转为路径；BioIcons 导入时已处理该转换。含原始文字的其他导入 SVG 会明确报错，避免配色动作悄悄替换字体或改变字形。


### 完整审阅历史

草图标记列表支持“全部标记 / 待处理 / 已处理”筛选及“加载更早标记”，每页 50 条。筛选在分页前执行，较新的已处理记录不会挤出较早的待处理意见。游标按 `(created_at, id)` 稳定倒序，限定在当前项目和图稿内；同一时间戳与标记状态变化均不会导致已读取记录重复返回。

`GET .../markups?page=true&status=open&limit=50&before=<id>` 返回 `{items,next_before}`，`next_before=null` 表示没有后续页；省略 `status` 返回所有状态。`limit` 允许 1..100。为兼容已有调用，不传 `page=true` 仍返回数组。列表不包含 PNG 或详细笔迹；单组读取保留历史版本及合成预览。

Agent 的 `visualization list_markups` 使用相同 `before/status/limit` 与分页结构。`read` 保留最近标记摘要，并用 `markups_pagination.next_before` 明确标出后续页。GUI 交接优先传待处理标记与其游标；Agent 可以继续调用工具访问较早的审阅意见。历史标记仍须与当前图稿版本核对，翻页本身不会改变正式图稿。

验证：存储测试覆盖 55 条同时间戳记录、无重复遍历、状态变化、跨图稿游标和非法筛选；HTTP 与注册工具测试覆盖分页结构及无图像字节泄漏；`test:canvas:native` 在真实 SQLite/HTTP/GUI 中加载第二页并切换两种状态。


### 提案回应审阅意见

局部修改提案可显式关联最多 50 个不同的待处理标记。Agent 应先用 `read_markup` 查看笔迹与说明、核对 `at_revision`，再在 `propose` 中传入真正回应的 `markup_ids`。历史意见可以在与当前图稿核对后关联。其他图稿、不存在、重复或已处理的标记会被拒绝，创建失败不会留下提案或关联记录。

提案的 `review_markups` 返回关联意见摘要，不含 PNG 和详细笔迹。GUI 的前后对照旁展示说明、原始版本及状态；“采用时同时完成关联标记”默认不选中。普通采用仅更新图稿；用户勾选后，采用与关联意见完成在同一 SQLite 事务内提交。拒绝、版本冲突或意见更新失败均不关闭意见，也不会留下部分修订；无关标记保持原状态。采用关联不证明科学内容已获验证，仍须人工核对。

HTTP 创建提案的 `markup_ids` 为可选数组；采用接口通过 `?resolve_markups=true` 显式请求完成关联意见，省略时保持原有行为。关联保留在独立表中，随图稿删除级联清理，不进入正式画布元素和出版导出。已有数据库在初始化时自动创建关联表。

验证覆盖拒绝与默认采用保留待处理状态、显式采用只完成关联意见、历史标记、错误标识、事务失败回滚、过期提案、删除清理，以及注册工具与真实 HTTP/GUI 的“读取标记 → 关联提案 → 核对并勾选 → 采用”闭环。模型决策在 native 测试中为固定决策，不代表真实模型科学判断已通过。


### 局部路径样式

“素材编辑”中可读取 SVG 内部路径，按编号/原始 SVG 标识选择单条路径，预览其边界并单独调整填充、描边、透明度系数，或通过位移输入/橙框中心手柄平移路径。平移使用源 SVG 根坐标，后台处理上层旋转、缩放与镜像；上层共享裁剪和素材视口保持原位，超出部分会被裁掉。相同颜色的其他路径不跟随修改。透明度系数作用于当前效果，1 保持、0 隐藏；隐藏可通过历史恢复。修改先生成前后对照提案，采用后保留图片尺寸、坐标、裁剪、绑定、作者许可和原始文件哈希，只新增文件与路径修改记录。来源导出包括这类修改的路径 ID 和样式。

HTTP `GET .../vector-parts?element_id=...&expected_revision=...` 返回文件快照哈希、路径摘要、预览与编号图；`POST .../vector-parts` 接收同版本、元素、`sha256` 及 `edits`。Agent 使用同一实现。共享裁剪/遮罩/图案资源不直接暴露，路径编号与当前 SVG 哈希绑定，过期编号被拒绝。上限为 256 条非资源路径、单次 32 条样式编辑。已支持 SVG 内部路径平移；已支持端点与 Bezier 控制点编辑，增删点、曲线自动平滑和拆为原生画布对象仍未实现。


“素材编辑 → 读取内部路径 → 读取端点与控制点”可选择编号，用 SVG 根坐标输入或拖动控制点。橙色轮廓表示本地几何草稿，正式效果在提案前后对照中核对；未修改点保持原位，不自动平滑相邻曲线。控制点编辑与整条路径平移须分开采用。`GET .../vector-geometry` 使用当前版本、`element_id/sha256/part_id` 返回端点、控制点、命令结构和编号视觉；修改沿用 `POST .../vector-parts`，通过 `edits[].points` 提交。来源清单保留这些稀疏坐标修改，原文件与图稿历史仍可恢复。


### 固定中文字体与独立导出

自动布局及默认文本使用字体 ID 9，现对应 Autonomics Scientific Sans：保留原 Liberation Sans 的 Latin／希腊字母轮廓、度量、kerning 和 shaping 表，补入 Noto Sans SC Regular（400）的 28,642 个 CJK 字形及相关字符。CSS 别名和资源路径保持与编辑器兼容。完整字符映射共 30,969 项，不等于完整 Unicode 覆盖；SC 字形、复杂整形、未覆盖字形和其他字体仍需分别核对。BioIcons 的独立 SVG 文字轮廓渲染仍使用原 Liberation TTF，未在本轮扩展其字符范围。

布局度量从实际 WOFF2 生成，图稿 `layoutInfo` 保留字体名称和 SHA256。断行使用 Unicode 15.0 UAX #14 的机会，避免普通宽度下拆开开闭中文标点，并保留可容纳的不换行空格词组；超长单词仍可按字符拆行。Gateway 通过显式资源路由内嵌字体、来源 JSON 与两份许可，不依赖运行机器的字体或 CDN。

复构建说明及源文件校验见 GUI 字体目录的 README。字体构建检查全部原 Latin 字形和布局表；浏览器验证含中文、Greek 的实际连线标签宽度与后端差异小于 1px、节点标签边界、无远程字体请求及 SVG 导出。`scripts/check-exported-font.py` 对实际导出 SVG 解码字体子集，按文本声明的字体族检查字形覆盖，检测实际系统字体依赖，而不只检查是否存在字体 data URL。


### 生产 Gateway 与 GUI 的连续会话

除了直接工具测试，`gateway/tests/visualization_conversation.rs` 通过实际鉴权 HTTP、RuntimeHost、研究者 Agent 工具注册、SDK 工具循环、SSE 和会话历史验证四轮流程：保存澄清问题、不提前绘图；保留用户答案并生成草图提案；读取同版本整图后仅改指定标签；读取审阅标记后删除指定组件及关系，由用户采用并完成关联意见。整个流程包含 15 次模型请求、11 次真实工具调用，API 与 Agent 共用图稿数据库。该 Rust 测试的 PNG 是传输夹具，不用于证明视觉选择。

`apps/gui/tests/canvas-gateway.mjs` 进一步启动编译后的 GUI 与完整 Gateway，不使用 Vite，也不替换登录、工作区、聊天、历史、事件或可视化 HTTP 接口。测试用 HttpOnly 登录 Cookie 创建项目和图稿，在实际聊天框中交接并发送四轮请求，通过真实 SSE 显示回复，操作提案前后预览及采用。整图 PNG 和 B 组件上的划除标记由 GUI 实际生成，注册工具读取后送至模型；测试核对版本、PNG 大小及模型收到的数据长度、无关对象、原修订、关联意见完成及源文件导出，同时检查生产字体路由与无外部服务请求。

只有模型决策使用固定脚本，宿主位于 `gateway` 的 `test-util` 功能内，不进入默认发布构建；没有测试专用 HTTP 操作捷径。数据、数据库和一次性登录凭据均隔离于临时目录，宿主只监听 loopback。模型轨迹仅记录步骤、动作和图像长度，不写入图像内容或凭据。它证明生产集成与数据一致性，不证明真实模型的科学判断、视觉定位或外部模型提供方 HTTP 行为。

```sh
cargo test -p gateway --test visualization_conversation
cargo build -p gateway --features test-util --example visualization_workflow
npm --prefix apps/gui run test:canvas:gateway
```

GUI 测试宿主要求一个空的临时目录，初始化后写入只供本地测试读取的 `ready.json`；创建 `stop` 文件会触发正常关闭。测试脚本负责完整生命周期与清理，不读取用户模型配置或调用付费模型。真实模型在生产 Gateway 中的完整科学绘图仍须单独验证。

### 局部放大快照

选中一张图片（或其语义锚点），点击「局部放大」，拖动框选区域或输入区域百分比，填写标题后预览并采用提案。预览框选面板保留原图旋转、镜像与已有裁剪，拖动通过图片局部坐标转换为比例区域；候选放大图保留相同方向。区域框、无箭头虚线、放大图边框、图片与标题是独立可编辑对象，放大图内容作为一组移动。SVG 继续复用矢量文件，不生成扁平位图。源对象与文件保持不变，放大图记录来源对象、版本、文件哈希和裁剪区域，来源导出保留这些修改记录及原素材许可。

这是固定版本快照：后续移动或修改源图，不会自动刷新快照或区域框。使用「删除放大图」预览删除提案，或让 Agent remove 放大图 anchor_id，会同时清理图组及其匹配的源区域框和虚线；原图、其他放大图及归属已修改的独立注释保留。画布原生 Delete 仍遵循编辑器对象删除规则，不额外处理注释归属。源图自动联动仍需实现。

`canvas-native.mjs` 在 `VISUALIZATION_BIOICONS_LIVE=1` 时验证真实 SVG 的框选拖动、比例输入、提案审阅、采用前不变、文件共享、重载及裁剪 SVG/来源导出；该固定测试不证明真实模型能选择科学上正确的放大区域。


点击「检查并刷新放大图」可将现有快照与其来源版本比较。检测区分图片文件、位置/变换/裁剪及来源信息；画布版本增加本身不会被误判为源图变化。刷新复用原比例区域，保留对象 ID、放大图边框、标题、样式和本地说明，更新图片文件、裁剪、旋转/镜像及区域框/虚线，尺寸适配现有边框。该操作先生成提案，采用时检查版本；源图替换后须核对所选比例区域是否仍代表预期科学部位。局部修改过的放大图文件或裁剪需要显式选择替换，源图缺失、成员归属变化或旋转后的放大面板不能直接刷新。历史快照与文件均保留。

HTTP 提供 `GET inset-status?element_id=...&expected_revision=...` 与 `POST insets/refresh {element_id,expected_revision,discard_detail_edits?}`；Agent 对应 `inset_status` 与 `refresh_inset`。当前图片裁剪为原生 SVG/PNG/JPEG/WebP 支持范围，复杂手工变形后的刷新和自动联动仍待补齐。


框选的浏览器校准测试使用带任意角度旋转、双向镜像和已有裁剪的四分区 SVG。它分别核对预览内外参照点颜色、旋转后的拖动坐标、精确原生 crop、区域框位置、源图/文件不变、采用重载，以及独立 SVG 渲染后的放大区像素颜色；该测试独立于在线素材目录，常规 `canvas-native.mjs` 也会执行。它证明坐标和渲染一致性，不证明区域的科学选择正确。


### Agent 的选区上下文

画布通过统一工作台的 `agentContext` 通道发布有界选区摘要；发送消息时读取当前状态，保留固定目标/跟随文档的既有行为。「交给 Agent」复用相同的 `canvasSelectionContext`，提供位置、尺寸、旋转、镜像、裁剪、图片文件 ID、绑定端点及放大图角色。放大图边框、图片、标题、区域框和引导线按已有对象身份/归属记录核对；复制或不一致的引用标为 `unverified_inset_reference`，不能直接据此执行级联删除。区域框与引导线是注释，不能据此推断新的生物组件或科学关系。

统一通道的 `selectedObjects` 为 JSON 字符串，`selectionReady` 表示当前编辑器场景已发布；加载/重载会清空旧摘要。`dirty=true` 时包含尚未保存的几何，Agent 应先让用户保存，再读取后端版本后编辑。连续修改同一选区，即使 dirty 和选中 ID 不变，也会更新摘要；false 选中项和已删除对象不会混入。摘要最多 100 对象、16000 字符，保留完整对象和不截断的 ID，附总数及截断标志。截断时不能执行“整个选区”的操作，需用户缩小选区或明确全部目标；读取保存的文档不能恢复省略的临时选中状态。

摘要不传图片字节或任意自定义备注；标签文本有独立上限。`test:canvas:selection` 覆盖边界、角色核对、复制/归属异常、隐私和截断；`canvas-native.mjs` 验证实际工作台发布、连续未保存移动及 Agent 交接的一致性，`workbench-context.mjs` 覆盖既有上下文目标与消息附加流程。


### 已有组件的对齐与分布

Agent 可调用 `visualization` 的 `arrange` 动作：`{project_id,figure_id,expected_revision,arrangement:{element_ids:[...],mode:"top"}}`。六种对齐为 `left/center/right`（水平位置）和 `top/middle/bottom`（垂直位置）；`distribute_horizontal/distribute_vertical` 整理间距。请求使用读取到的版本及明确的完整目标 ID，默认生成待审阅提案，不直接改写图稿。可附 `markup_ids` 关联已实际回应的审阅意见，仍由用户在采用时决定是否完成。

原生组合、语义锚点、所属素材与标签作为一个组件移动；框架保留并移动其内容，重复选中的父子成员合并为一个单元。矩形/文字/图片使用旋转后的矩形边界，椭圆和菱形按编辑器的各自边界公式计算。分布需要至少三个独立单元；空间充足时边距相等，空间不足时沿用编辑器固定外边界组件、整理其他中心位置的退化方式，不保证消除重叠。直线箭头的绑定端点和文字跟随实际组件位移，保留关系、证据、样式与素材文件。无关对象和旧修订不变。

锁定成员、放大区域/引导线注释和缺失对象会在生成提案前拒绝。曲线、旋转箭头或肘线只能随两端整体平移；若两端分别移动则需先提供明确的原生几何，不能静默破坏连接。无抖动、无圆角且非肘线的直线/折线，以及手绘点列，已支持原生边界。组合内的绑定连接线必须与其端点同幅移动，防止整理使组合变形；组合外的独立直线可调整端点跟随组件。旋转箭头的绑定文字、粗糙线条、曲线和肘线边界仍需原生适配。整理不自动避障，需在提案中检查重叠和交叉；已有跨面板箭头可另行 `reroute`。移动固定快照的源图后，应检查放大图状态并按需刷新。

`arrange` 单元测试覆盖全部对齐轴、双向分布、拥挤退化、父子合并、旋转边界、框架、锁定及连接线条件。已注册工具测试覆盖版本和审阅意见的采用事务；原生浏览器回归以编辑器自身 `getCommonBounds` 校验组合素材、旋转椭圆和菱形，验证提案采用、连线/标签、文件不变与重载。固定几何案例不证明真实模型能正确选择科学对象或布局。


组合回归进一步覆盖内部折线及绑定文字（按编辑器计算的视觉位置，而非旧导入文件中可能失效的文字 x/y）、旋转手绘与笔压。浏览器直接用编辑器 `getCommonBounds` 比较整理结果，验证采用、重载后点列及笔压不变。手绘使用编辑器的点列边界，与笔压生成的可见笔刷轮廓不完全相同；不声称笔刷像素外缘精确对齐。Agent 选区摘要同时提供最多 32 层 `group_ids` 和截断标志，整理前仍须读取保存的完整组合结构。


### Agent 检查候选图

`read_proposal_preview` 接收 `project_id/figure_id/proposal_id/expected_revision`，为不可变候选文档创建按需渲染请求，等待保持打开的项目画布在后台导出 PNG，返回独立视觉块；不采用提案，也不覆盖正式版本。画布切到内部 Agent 标签后仍可服务请求。后台副本使用普通编辑器的绑定修复，保留箭头绑定文字，PNG 不嵌入源场景或任意自定义备注。科学字体资源按实际字节验证指纹；缺失图片或字体加载失败会记录失败，不发布空缺图像。

缓存按提案 ID 与保存的候选 JSON 字节 SHA-256 关联，元数据包含 `scope=proposal_candidate`、基础版本、读取时的当前版本、提案状态、渲染器/科学字体指纹。`base_is_current=false` 时应与当前文档核对，不能把候选预览视为当前正式图。第一次成功图像保持不变，重复相同交付幂等，不同图像或失败回报不能覆盖它；采用或拒绝后仍可读取该候选历史。每图最多 16 个未完成请求，每次向浏览器提供最多两个任务；不导出用户尚未保存的本地画布。

无运行中的相关画布时保持排队，工具最多等待约 6 秒再明确返回未就绪，不回退到正式画布的 PNG。失败只在资源修复后通过 `retry_preview:true` 显式重排；成功缓存不重新生成。预览请求和完成接口沿用项目归属、哈希/基础版本校验及 PNG 大小上限；删除图稿时一起清理。当前仍依赖 GUI 渲染；无界面渲染、候选基础上的直接增量修订和真实模型自主视觉纠错仍待实现或验收。

HTTP 为 `POST .../proposals/{proposal}/preview-request`（当前 `expected_revision`、可选 `retry_failed`）、`GET .../proposal-preview-requests`、`GET .../proposals/{proposal}/preview?expected_revision=N`、`POST .../proposals/{proposal}/preview`（基础版本、文档哈希、renderer 与 PNG/error 二选一）。图片由受信任的浏览器渲染器提供；服务端校验协议及图像头，不从像素反证文档一致性，也不证明科学正确性。存储/HTTP/已注册工具测试覆盖归属、冲突、不可覆盖、采用历史、失败重试及无渲染器；浏览器回归验证审阅打开前的真实候选视觉块、箭头标签像素、字体/内容指纹、缺失素材失败，以及禁止用一像素正式预览冒充候选图。
