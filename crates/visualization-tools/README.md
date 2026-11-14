# visualization-tools

Agent 工具适配层，独立于 runtime 和 HTTP 服务。Runtime 使用 `registrations(database_path)` 与 `bioart_registrations(database_path)` 注册；二者使用与 Gateway 相同的项目画布数据库。

- bioart：search、preview、insert、replace；图片数据和来源信息一同保存。

优先使用 propose 供用户在 GUI 预览和采用；所有修改必须使用已读取的版本。GUI 未保存内容不会自动暴露给工具。

验证：`cargo test -p visualization-tools`。工具输入 schema 与注册依赖通过真实 agentik 宏编译，不依赖整个 runtime 的计算节点依赖。

`read` 保留图片文件元数据而省略 dataURL；`propose` 只返回提案元数据，避免将图片字节塞入文本上下文。`read_image` 按 element_id 读取已保存的图片对象，以 SDK 图像内容块返回 PNG/JPEG/WebP；可携带 expected_revision 检查版本。遵守运行时 6 MiB base64 传输限制，超过时要求先缩小参考图。图像传递需要模型声明视觉能力且运行时未禁用工具图像。`read_preview` 读取 GUI 为当前版本生成的整图 PNG，缺少当前版本截图时明确报错，不回退到旧图。工具本身不启动浏览器渲染。

`bioart replace` 在指定修订上将占位图升级为素材组，保留语义锚点和连线并返回提案元数据，用户须在 GUI 对照预览后采用。不会将候选文档中的图片字节返回给模型。

`bioart preview` 在修改画布前返回素材图像及来源、作者、许可、校验值，支持视觉模型核对素材；不创建画布修订。GUI 素材面板也提供预览，并复用已加载图片进行插入。

在活动画布上规划草图时，应传 figure_id 和 expected_revision；返回的是该记录的待审阅提案，正式图稿与其他记录保持不变。只有明确需要独立新图时才省略 figure_id。

`read_markup` 按 markup_id 返回独立保存的草图审阅图与说明，明确原始 at_revision 和当前 current_revision。标记使用划除、补画和强调三类几何；不作为正式场景对象写入。读取旧标记时必须与当前文档核对，不能把历史图当作当前整图预览。`read` 附最近 50 组标记摘要，省略详细笔迹和图片字节。

sketch.document 支持 layout=grid（默认）、horizontal、vertical。网格模式要求唯一 column/row；自动模式省略坐标，按显式边分层，保留反馈和并行关系。图稿仍作为可审阅提案进入活动画布。自动布局不会补充科学关系；环内节点输入顺序影响前向/反馈通道。



`{op:"reroute",id}` 仅整理现有生成式跨面板箭头；可放在 propose.operations 中，跟在移动操作之后执行。以当前绑定和当前标签尺寸重新避障，保留文字、样式、ID、科学依据及无关对象。操作失败时整批回滚，不从旧计划重建图稿。端点解绑、旋转的绑定组件或无效几何需先处理；普通手绘箭头不在此操作范围。GUI 的“整理跨面板连线”使用相同操作并进入提案审阅流程。

`bioicons` 提供 search/preview/insert/replace，用于开放 SVG 素材。preview 返回 PNG 图像块与逐素材作者/许可/转换说明；insert 与 replace 都生成待审阅提案，要求 project_id、figure_id、expected_revision，replace 还需 element_id。保留原文件和规范化文件哈希；SVG 内部路径不是独立画布对象。`visualization read_image` 可将保存的 SVG 安全转换为 PNG 供视觉模型查看，原文件保持 SVG。

`bioart` 的 preview/insert/replace 默认使用 PNG，可显式传 `format:"svg"` 获取规范化矢量文件与 PNG 预览。没有 SVG 表示或规范化失败时明确返回错误，不自动改用 PNG。保留 NIH 作者、许可、素材与文件身份、原始及规范化哈希。GUI 的 BioArt 面板可选择插入格式；已保存的 BioArt SVG 使用同一套 `bioicons palette/parts/geometry/recolor/edit_parts` 本地编辑动作，无需 BioIcons 目录 ID，也不会重新下载或更换素材来源。路径编辑仍属于一个图片对象内的修改，不会自动赋予路径生物学身份。

GUI 的 `test:canvas:bioart-svg:catalog` 验证真实目录搜索/预览/插入、内部路径平移与导出，不调用模型。`test:canvas:bioart-svg:live` 需要显式 `VISUALIZATION_LIVE_FIT_SCENE`，验证已有双实例 BioArt SVG 的真实 Agent 改色/路径强调；`test:canvas:bioart-svg:online` 从空图开始，先让真实 Agent 搜索/预览/插入两个 SVG，用户采用后在同一会话继续改色/路径强调，不允许传入外部图稿。真实模型测试需设置已配置的 provider/model 与新输出目录。BioArt 修订准备只替换一种重复填充色，保留其他填充和轮廓，避免全色归一把内部结构抹平。

`test:canvas:svg:visible-repair` 使用显式保存的待修图（已有橙色轮廓但无明显蓝色主体），通过真实候选反馈让 Agent 读取渲染贡献并自行选择主体路径；以实际 PNG 的蓝/橙近色像素核对可见效果，同时保护另一实例、几何、源文件和来源许可，再验证 GUI 采用与源/来源清单导出。这是给定视觉反馈后的修订，不验证模型能自主发现所有视觉问题。

`test:canvas:candidate-path-agent:live` 需同样的显式待修图，先准备与正式图配色不同的候选，再通过候选路径选择界面交接真实 Agent；核对其精确使用候选ID/文档hash和所选part_id/文件sha生成子候选，并保持前序修改记录、文件、父候选和正式图。该模式不验证新的科学机制发现或在线素材获取。

`test:canvas:shared-path-context:live` 不点击路径草稿按钮，手动关闭候选路径面板后绑定“跟随当前文档”，直接发送不含任何 ID/hash 的普通请求。它核对实际模型用户消息收到编辑器快照、冷启动绑定迁移、候选/路径精确匹配及完整采用导出；仅代表给定保存图稿的局部修订。

`bioicons palette` 需 project_id、figure_id、expected_revision、element_id，可读取图片或语义锚点的实际颜色与 PNG 视觉预览；`recolor` 另需 colors 对象（`#RRGGBB` 映射），创建配色提案。动作不访问目录网络，也不需要 asset_id。仅修改目标 SVG 实例，保留来源许可、文字路径、位置与其他实例，并记录前后哈希及颜色映射。先读 palette，勿猜测源颜色；相同颜色的路径一起替换，不能单独指定同色路径。


`visualization list_markups` 可按 `status: "open" | "resolved"` 筛选审阅历史，并通过 `limit`（1..100，默认 50）和 `before` 翻页。结果为 `{items,next_before}`；重复使用返回的游标直到 null，避免把最近一页当作完整历史。`read` 中的 `markups_pagination.next_before` 表示仍有较早标记。列表只包含摘要，具体笔迹与图像仍通过 `read_markup` 读取并核对历史版本。


`visualization propose` 的可选 `markup_ids` 关联实际回应的待处理意见（最多 50 个不同 ID）。先使用 `read_markup` 查看并核对历史图稿，再关联对应 ID；不要关联尚未处理的意见。结果包含无图像字节的 `review_markups` 摘要。该操作不关闭意见，GUI 用户可在采用时显式选择完成关联标记；`apply` 不接受非空 `markup_ids`，不能绕过审阅采用来关闭意见。


`bioicons parts` 读取当前 SVG 的路径编号、边界及编号视觉结果；`edit_parts` 使用该结果的 `sha256` 和 `edits:[{part_id,fill?,stroke?,opacity?,translate?}]` 生成局部样式提案。`fill/stroke` 允许 `#RRGGBB` 或 `none`；`opacity` 是相对当前效果叠加的 0..1 系数。先查看编号图，不得把路径编号当成科学组件名称。采用后编号失效，必须重新读取；每次最多 32 条编辑，素材最多 256 条非资源路径。`translate:[dx,dy]` 使用 SVG 根坐标，后台补偿上层缩放、旋转与镜像；视口和共享上层裁剪不移动，移动后的超出部分仍会裁掉。端点与曲线控制点编辑见下方 geometry；图片对象外部的标签、绑定和其他实例保持不变。

`parts.render_contribution` 提供有界的逐路径渲染比较：最多32条路径、累计8 MiB解析输入，长边最多256像素；临时隐藏一条路径，与透明背景的 SVG 根预览比较预乘RGBA（不含画布裁剪、背景、缩放与其他对象），返回每条路径的 `changed_preview_pixels_when_hidden`。超过上限明确 `not_computed`，不截断后推断未检查路径。计数是当前分辨率下的渲染贡献，不是可见面积或遮挡掩码；零可能来自覆盖、同色、裁剪或细小结构。非零也不保证原始颜色会以相同RGB出现在最终图中，抗锯齿、透明和覆盖会混色。GUI 素材路径面板展示该诊断，Agent 必须结合最终PNG判断用户要求的颜色/强调是否可见。


`bioicons geometry` 使用 `project_id/figure_id/expected_revision/element_id` 及 `parts` 返回的 `sha256/part_id` 读取端点与曲线控制点（编号 PNG + 无图像字节的 JSON 摘要）。`edit_parts` 可传 `points:[{index,position:[x,y]}]`，坐标为当前快照的 SVG 根坐标。只改指定点，不自动平滑；同一路径不可同时传 `points` 和 `translate`，采用后重新读取编号。路径最多 512 个点，单次最多改 64 个不同点，退化导致拓扑改变会被拒绝。编号是图形坐标，不是生物学身份。

`image_info` 在明确 `element_id` 和 `expected_revision` 后返回图片实际尺寸、裁剪和来源（不返回嵌入文件）。`inset` 接收同一版本与 `inset: {element_id,region:{x,y,width,height},x,y,max_size,title}`，生成待审阅的局部放大快照提案。region 为当前可见图片镜像后、旋转前的比例区域；max_size 为 32..2048。使用前应 read/read_image 检查源图。复用原始文件，不栅格化 SVG；保留许可和版本来源。快照不会随原图移动或修改自动刷新，区域框为独立注释。

删除整个放大图时只 remove 其 `customData.inset.anchor_id`：会清理归属匹配的区域框、引导线及内容组，保留源图、其他快照及独立注释。不要再显式 remove 已级联删除的 ID。

`inset_status` 使用 `element_id`（原始放大图锚点或图片 ID）和 `expected_revision` 比较源图与历史快照，不返回图片字节；`changed` 不单凭画布版本判断。`refresh_inset` 用相同参数生成待审阅提案，保持原比例区域与稳定 ID，保留边框、标题、样式和其他对象；源图变化可能改变比例区域的科学含义，需先 read_image/read_preview 核对。若 `detail_edited` 为真，只有显式指定 `discard_detail_edits:true` 才会在提案中替换放大图独立修改的文件/裁剪。源图缺失、归属不明和旋转后的放大面板须先解决；刷新仍是固定版本快照，不是后台自动同步。

`arrange` 接受 `arrangement:{element_ids:[...],mode:left|center|right|top|middle|bottom|distribute_horizontal|distribute_vertical}` 和精确 `expected_revision`，返回待审阅提案（不返回嵌入图稿字节）。读取完整明确的目标后操作；原生组合、语义锚点及所属标签/素材一起移动，父子重复选择会合并；分布至少三个独立组件。空间不足时沿用编辑器的中心退化方式，仍需核对重叠。可携带 `markup_ids` 关联实际回应的标记，采用时才按用户选择关闭。排除放大区域/引导线注释，先处理锁定或缺失对象；支持手绘点列与无抖动、无圆角的线段/折线边界；被选组合内部的绑定连接线必须与端点同幅移动，不能为满足外部绑定而拉伸组合。分开移动的曲线/旋转/肘线、粗糙连接线和旋转箭头绑定文字仍需原生几何适配。不推断关系、不自动避障，保留来源和文件；必要时另行 reroute。

`read_proposal_preview` 使用 `proposal_id` 和当前 `expected_revision` 请求候选图 PNG，最多短暂等待约 6 秒。返回 `scope=proposal_candidate`、文档 SHA、基础/当前版本、提案状态、renderer 和独立图像块；候选没有因此采用。检查 `base_is_current`，不要把历史候选当作当前文档。工作台需保持打开并登录（无须打开对应画布）；未就绪或失败明确报错，不回退到正式画布图像。资源修复后可用 `retry_preview:true` 重排失败请求；成功图像不可覆盖。模型看图与选择正确修正动作仍需真实案例验证。

`read_proposal` 读取指定候选并隐藏图片数据；复制其 `document_sha256`，再用 `revise_proposal` 的 `proposal_id/document_sha256/expected_revision` 和 `operations` 或 `arrangement` 生成下一版候选。修订基于父候选文档，保留累计修改，返回独立提案及来源。父候选关联开放审阅时，需显式给出 `markup_ids`（`[]` 表示不继承）。两版候选均不会自动采用。存储文档哈希不能用脱敏 JSON 或 PNG 的哈希替代。

`read` 附带 `proposal_summary`，声明正式当前版本、待审阅数量、其中当前/旧基础版本数量和最新摘要，不返回候选文档。`list_proposals` 使用 `project_id/figure_id`、可选 `expected_revision` 和 `before/limit`（1–100，默认 20），返回待审候选摘要分页、`current_revision/status/next_before`。复制游标加载更早候选；`status` 不支持自行指定，避免把历史已采用项混为待审阅结果。候选详细文档仍通过 `read_proposal` 精确读取，读取后再判断科学内容与修订对象。

注册工具测试验证 2＋1 分页、当前摘要数量、跨项目隔离、版本冲突、非法页大小/状态拒绝，且列表不含文档或图片字节。

候选预览由已登录工作台统一渲染，不再要求目标画布编辑器已打开。队列仍只接受显式 `read_proposal_preview` 请求；首次字体初始化在正式渲染前完成。没有打开工作台时仍可能排队或失败，应保持工作台登录后重试，不会替换成正式画布图像。完全无浏览器的渲染尚未实现。


现有连线的局部修改可用 `operations:[{op:"set_relation_direction",id:"edge-0",direction:"bidirectional",inhibitory:false}]`，两个状态字段都必须明确给出。支持提案与候选连续修订，保留端点几何、绑定、文字、依据和历史计划。不能用来改变放大图引导线、批注或已锁定/删除对象；不能将抑制与双向/无向组合。改变图示方向前须核对科学意图及证据，不要据新端标记声称已经验证机制。

长标签优先使用 `{op:"set_wrapped_text",id:"label-0",text:"..."}`，在默认科学字体下同步换行和尺寸而不是只更新 `text`。可用于普通提案或候选连续修订；自由文本可提供 `max_width`，绑定文字使用原生容器宽度。形状尺寸保持不变，放不下时先明确调整容器/字号，再核对候选；不支持的字体、旋转或复杂连线会失败，不猜测度量。原文保存为 `originalText`，原有绑定、依据与样式保留。

`duplicate` 局部操作支持在待审提案中复用现有完整组件：`{"op":"duplicate","element_ids":["node-0"],"id_prefix":"trialB","dx":300,"dy":0}`。读取候选核对自动展开的成员与新 ID，再请求候选预览，确认内部连接和外部断开符合本轮需求；复制保留原引用与历史计划，不自动验证新实验语境。

`propose` / `revise_proposal` 的 `sync_inset_region` 操作用于源图变换后重新定位固定快照的区域框和引导线，不刷新放大内容。指定完整放大图的锚点或图片 ID；以记录的原始裁剪像素及源文件 SHA-256 校验定位，保留快照、引用和图像字节。源文件变化、区域被裁掉、成员不完整/锁定、旋转面板或复杂引导线会拒绝，应核对并选择显式刷新或原生编辑。该操作生成待审候选，不启用自动跟随。




FigureTool 的输入反序列化错误会报告具体字段（例如 `input.markup_ids`），操作解析报告操作下标。`markup_ids` 必须是 JSON 数组；不自动把 `{}`、item 包装或字符串转换成 `[]`。省略字段不代表解除父候选已有的审阅关联，显式 `[]` 才可解除，且不关闭原审阅。18 项工具回归覆盖错误输入无子提案、显式保留/解除关联、父候选和正式版本不变。

BioArt 和 BioIcons 的 `replace` 支持可选 `proposal_id` 与 `document_sha256`，须成对提供，均来自 `visualization read_proposal`。此时在待审候选上替换素材并创建子候选，不要求先采用草图。传原正式基础版本作为 `expected_revision`；子候选响应包含父 ID、父哈希及自身哈希。若父候选有关联审阅，必须明确传 `markup_ids`，保留 IDs 或用 `[]` 解除。完成后读取子候选预览并留待用户审阅。

这些候选参数和 `markup_ids` 仅用于 `replace`，其他动作拒绝它们，避免将候选定位信息静默忽略后操作正式图。未传候选参数时沿用原有入口：BioArt replace 和 BioIcons insert/replace 生成根提案；BioArt insert 保留既有直接保存语义。通用素材不构成特异分子结构或新科学机制的证据。

`visualization` operations 新增 `fit_illustration_anchor {id,file_id,normalized_bounds:[x,y,width,height]}`。先读取当前图片/候选和文件身份，检查图片或 SVG parts，再明确提供相对整张图片的范围；不把路径边界当成分子域。该操作调整语义锚点与局部正交连接、保留图片字节和科学关系，沿用候选审阅和哈希约束。新素材替换后旧贴合失效，按新素材重新贴合再重路由。图片的有效裁剪、镜像和中心旋转现可映射为轴对齐锚点，选框须位于可见源裁剪内；复杂连线、旋转锚点或固定端口仍须使用原生编辑，检查候选后再采用。

`bioart insert` 与 `bioicons insert` 均返回待审提案，不直接保存正式图。BioArt 的旧插入结果 `{figure,...}` 已改为 `{proposal_id,element_id,base_revision,status,...}`，调用方须审阅并采用。两者都可用 `proposal_id`、`document_sha256` 和正式 `expected_revision`，在指定待审文档上追加独立图片；新图使用场景 `x/y`，不传 `element_id`，已有节点的外观更新应使用 `replace`。连续插入保留所有前序对象和文件，作者/许可与原始来源哈希随素材保存；开放审阅关联须显式保留或解除，不自动完成。

`figure_evidence` 是独立只读来源工具，由 `evidence_registrations()` 注册并接入 Runtime 的绘图能力，不改变原有 `registrations(path)` 的单个画布工具接口。当前提供 Reactome `search` / `record` / `events`：搜索候选记录，再读取所需通路、反应或相关实体 ID，返回官方来源 URL、检索时间、完整检索记录 JSON 序列化的 SHA-256、物种/推断标志、机制说明、输入/输出、催化/调节实体、相关事件和文献字段（存在时）。`events` 用已发现的通路 ID 列出其中候选事件（默认 100、最多 200），显式报告截断；只返回导航元数据，不算逐事件已读证据，仍需 `record` 核对转变和催化复合物。数字引用 ID 及重复项保留，不推断或去重化学计量。投影深度、列表和文本上限、字段省略与截断明确标出；不是完整原始响应，不验证图中每条关系、版权或实验支持，不保存计划或图稿。

按当前科学意图明确指定搜索物种，不默认人类。搜索命中及通路总览不是逐关系证据；应读取相关反应、必要时跟随引用 ID，保留来源中的不确定性，再将适用 URL 放入计划 evidence，并把范围和未知项写入 assumptions/questions。只读工具不会自动把计划置为科学正确或自动生成图。

显式联网复现：`cargo run -p visualization-tools --example figure_evidence -- '{"action":"search","query":"interferon gamma","species":"Homo sapiens","limit":3}'`；`record` 改传 `id`，如 `{"action":"record","id":"R-HSA-877300"}`。网络错误明确返回，不构造占位来源。记录指纹针对 SDK 取得的 JSON 值的序列化字节，不是 HTTP 原始字节。

### 精确范围的只读后续参数


这些是只读输入，既不自动执行，也不采用、修改或授权候选。结果过期后调用仍会冲突，需显式重新读取并核对；检查历史候选时保留当前图稿版本与候选基础版本的区别。不补齐错误请求的 ID，不猜测项目或图稿。字段是工具输出的附加上下文，不写入正式场景、候选文档或计划历史；当前只适用于 `visualization` 工具，目录素材工具的结果格式不在此变更范围。


后续真实单例 `/tmp/autonomics-canvas-live-dNDUtr` 已完成三轮来源工作流，第三轮六次绘图调用均成功（创建、读取、子候选修订和 PNG 检查），GUI 采用及来源导出通过。此前 44 次/29 错误的失败轨迹仍保留；两次运行不足以证明所有模型或场景的稳定性。图下说明的科学语言仍有两处需按明确审阅意见收窄，独立记录于 `manual-source-review.json`；结构/调用成功不等于每一句话获科学验证。

明确审阅意见后的独立修订 `/tmp/autonomics-canvas-live-Xu6RXl` 通过：只替换两处说明文字，保存科学图结构、引用与历史计划，预览/采用/源文件核对通过。该次首次 read_proposal 曾漏 project_id 并恢复；GUI 候选反馈现在也从最初交接提供完整的同名只读输入，三类焦点对象的原生浏览器回归通过，不自动发送或修改图稿。此 UI 补充不改变工具的 ID/版本验证，也不意味着已普遍消除模型误填。

### 素材候选与保留身份的平移

BioArt 插入/替换以及 BioIcons 插入/替换/改色/路径修改的成功结果现在统一包含候选哈希、完整图稿范围、`candidate_context` 与同样的只读 `follow_up_inputs`；既有 proposal_id/element_id 字段保留。不包含场景或图像字节。两个素材提供者的实际注册工具回归均验证可原样调用返回的 read_proposal 参数，读取到同一候选及哈希，原图未改动。

在 propose/revise_proposal 中整体移动现有组件或数条说明，使用 `{op:translate,element_ids:[ids],dx:0,dy:79}`，dx/dy 为 JSON 数值。完整组、帧和互相确认的素材/标签归属只移动一次，保持 ID、文件与科学元数据，按既有整理规则处理支持的连线及标签。锁定成员、保护性放大图注释、无法安全处理的部分曲线连接或越界会使整批失败。不要 duplicate/remove 来代替移动。原始 update 的 x/y 字符串仍拒绝；工具错误会精确指到 operations[i].changes.y 并提示 translate，不静默转换。

素材输入中的 `markup_ids` 现在在反序列化阶段提供字段级错误：`{}`、字符串和非字符串项均拒绝，明确说明 `[]` 是有效数组，且不会把非法输入改成空数组。省略/null 保留现有可选字段语义；候选开放审阅的校验不变。真实 IFNG 轨迹曾将 `{}` 错误归因为空数组错误，本修改针对该实际误解提供反馈。核心输入回归、两个实际注册工具入口的失败/不创建数据库回归，以及工具库 23 项测试、严格 all-targets Clippy 通过。仍需真实模型验证反馈能否减少重复错误。

后续输入反馈增加 `operations[n].normalized_bounds` 的严格四数值数组诊断，避免把贴合错误解释为计划 questions 数组；通用 operations 类型错误现在给出原生数值、布尔和 translate 示例。完整工具库 25 项回归通过（含带引号布尔拒绝），请求编码回归证明 Anthropic/OpenAI 的流式和非流式编码均保留完整注册 schema。这不证明远端模型执行约束，也不能据此声称真实调用错误率已下降。

`visualization_fit` 提供已有 `fit_illustration_anchor` 的标量入口：必填项目/画布/候选 ID、精确文档哈希、当前版本、摘要、锚点/文件 ID，以及归一化 SVG 根坐标 x/y/width/height 四个 JSON 数值。它复用 FigureTool 的候选修订流程，返回相同的候选上下文和只读跟进输入，不采用、合并或推断最新分支。原 operations 接口继续支持批量编辑。注册入口回归验证真实带素材图稿在说明已平移候选上继续贴合，保留说明/科学元数据/文件、父候选/正式版本；错误哈希不增加候选，字符串数值与缺失候选 ID 在执行前拒绝。GUI 交接及 IFNG 实际评估已识别该入口；双入口单轮与连续两轮真实验证结果见下文，统计性降错仍待多案例验证。

新增入口后的兼容性回归已全部通过：25 项库测试、asset_input、fit_entry（2 项）、follow_up、translation 和 wire_schema。旧计划错误用例原先以 pop 取工具，新增注册后误取 fit；已改为按 visualization 名称选择并通过重跑。GUI 构建、TypeScript 与严格 Clippy 通过。实时评估可设置 `VISUALIZATION_LIVE_REQUIRE_SCALAR_FIT=1`，要求实际成功调用 visualization_fit；不能用未调用新入口的结果证明其模型效果。

专用入口真实评估 `target/visualization-evaluations/ifng-art-scalar-fit-v4` 已通过：明确要求并实际成功调用 visualization_fit，单一替换→贴合→说明平移候选链，最终 PNG、GUI 采用和源文件核对均通过。148466 ms、27 次调用、8 次错误（缺项目 1 次、translate.dx 字符串 6 次、禁用 memory_search 1 次）；贴合入口无失败。人工查看 PNG 确认说明与受体标签分离。这一单例不证明统计性降错或交互延迟通过，也不证明模型自主科学审查；平移数值类型错误仍存在。

`visualization_translate` 为同一刚性平移操作提供顶层必填 dx/dy 数值和 element_ids 数组，要求完整候选作用域、版本和哈希。复用 FigureTool 修订逻辑，不替换 ID、不自动采用或切换分支。编译检查与注册工具回归通过：说明身份/来源、父候选和正式版本保留，返回子候选的完整回读输入；带引号数值在执行前拒绝，错误哈希不生成候选。GUI 交接及实时 IFNG 评估已识别新入口；`VISUALIZATION_LIVE_REQUIRE_SCALAR_TRANSLATE=1` 要求模型实际成功调用并记录使用状态，最终内容/连续链/导出约束不变。真实 MiniMax 双入口单例已通过（`target/visualization-evaluations/ifng-art-scalar-edits-v5`）：120455 ms、20 次调用、2 次错误（均为不可用的 memory 工具）；visualization_fit 和 visualization_translate 各成功调用一次，素材替换、累计候选、说明避让、PNG、GUI 采用及导出核对通过。已人工查看候选 PNG，说明位于受体标签下方；这是通用受体素材，不验证 IFNGR 特异结构，单例也不证明统计性降错或交互延迟达标。 新增入口后的组合回归 `cargo test -p visualization-tools --lib --test translate_entry --test fit_entry -j 1` 已通过（25 项库测试、2 项贴合入口测试、1 项平移入口测试）；该结果验证注册兼容性与候选保护，不代表真实模型双入口验收。


连续会话评估可在上述双入口案例上增加 `VISUALIZATION_LIVE_IFNG_CONTINUATION=1`，通过实际候选反馈 UI 在同一 Agent 会话发起第二轮：将三条现有说明再下移 24 个单位。测试逐对象核对第一轮插图/文件字节、标签、连线、科学依据及其他位置不变，核对父候选不可变、正式图未提前改写，并对最终候选执行 GUI 采用和源文件导出。MiniMax 实测通过（`target/visualization-evaluations/ifng-art-continuation-v6`）：第一轮 173466 ms、22 次调用、4 次错误（越界贴合范围、缺少 figure_id、两个不可用 memory 工具），第二轮 38561 ms、5 次调用、零错误；两轮均成功读取候选 PNG，已人工查看最终图。该结果来自预先提供的已审阅 IFNG 图稿，不代表从科学意图开始的完整流程或多主题稳定性。


`visualization_add_text` 新增待审候选上的文字添加入口：必填作用域/候选/哈希/版本、唯一 element_id、text，以及顶层数值 x/y/max_width/font_size；可选 frame_id，图下说明应省略该字段。复用 add_wrapped_text 的科学字体度量、换行、保护与原子候选修订，不改现有元素，不自动采用。来源到草图的真实连续案例暴露了嵌套文字数字被写成字符串的问题；新入口的注册测试和严格 Clippy 已通过；新增后的 29 项组合回归通过（25 库测试、文字入口 1、贴合入口 2、平移入口 1），字体更新后的严格 Clippy 也已通过，真实模型效果尚未验证。连续评估可用 `VISUALIZATION_LIVE_REQUIRE_SCALAR_TEXT=1` 强制要求草图生成轮成功调用该入口，并保持确认后的计划不变、单一候选链和原始来源/导出检查。

绑定科学节点的插图位置/尺寸使用 `visualization revise_proposal` 的 `set_illustration_geometry` 操作：`id`（图片或锚点）、`file_id`、数值 `x/y/width/height` 和显式 `normalized_bounds` 必填。与独立图片的 `set_image_geometry` 区分：本操作保留素材、裁剪、镜像、旋转、标签和科学元数据，重新计算语义锚点及支持的原生连线；标签不随尺寸自动移动，应单独核对。完整可见图片及锚点须留在所属面板内；不支持的连接、过期素材、锁定对象或无效几何导致整个候选操作失败。

`visualization_scale_illustration` 为已贴合插图提供保持中心的等比例缩放入口，必填作用域、候选 ID/哈希、正式基础版本、element_id、file_id 和数值 factor；1.1 表示宽高各增加 10%，不是百分数，也不是原生镜像 scale。复用核心 `scale_illustration` 操作，从同一候选的互相归属锚点读取有效 visualAnchorFit.normalizedBounds，检查文件和当前贴合几何；缺失或失效时拒绝，不推测或放宽到完整图片。后续由原 set_illustration_geometry 路径验证锁定、画框、连接和其他保护，生成待审子候选；开放审阅仍须明确保留或解除，未自动采用。标签保持原几何，调用后须看 PNG 并独立审阅位置。这针对 EGFR 连续实测 v25 中多次错误填入范围对象、遗漏范围的实际问题；输入与候选链回归不等于真实模型已完成验收。

`figure_evidence record` 返回 `source_context`，突出物种、催化复合物名称和事件到顶层通路的多条祖先路径。路径来自 Reactome `/data/event/{id}/ancestors` 的实际成员关系；最多返回 8 条、每条 8 个节点并标明截断、总路径数和仅覆盖类型化成员关系的哈希范围。它不是对祖先通路记录的完整阅读，不增加“已读来源”白名单，也不判定本图的机制正确。催化物种/复合物与路径须一起核对；截断、失败或没有某个词都不证明机制不存在。

`visualization read`、`read_proposal` 及候选修改结果返回只读 `scene_geometry_review`：包含后续图注、插图显示外框及所有未删除对象的整体边界、宽高比和已知最小文字字号。它不同于仅编译计划的 `layout_review`。直线点列支持负坐标，旋转对象按几何外框计算；曲线、无效几何和超限对象会明确计入跳过/遗漏并返回 partial/unavailable，不把局部计算当成全图覆盖。上限为 5000 个对象，每条路径 1000 个点；描边、实际字形墨迹、frame 名称与像素碰撞不在此检查内。新增图注或素材后应重新核对整图尺寸并查看 PNG；读取诊断不产生子候选，也不修改图稿或哈希。

`visualization_translate` 保持组件整体平移语义：选择分组插图的标签也会带动其锚点、图片和支持的连接线。只定位一份独立标签时，使用 `visualization revise_proposal` 的 `update` 和该文字对象的数字 x/y，不使用组件平移。成功编辑后的纠错应继续使用返回的子候选；仅在用户明确要备选版本时从旧父候选创建兄弟分支。

只对已知事件类型请求祖先路径，追加请求最多等待 8 秒。上下文失败或超时会明确返回 unavailable，保留本次已读取的原反应记录及原记录哈希，不伪造空的“正确上下文”。非事件类型明确标为未请求。物种、催化名称、路径条数/深度及名称长度均有界并报告截断。Rust 客户端 `event_ancestors` 的类型已按实际多路径响应修正为 Vec<Vec<EventAncestor>>，对应 HTTP 协议回归通过。

两条真实记录已通过同一注册工具读取：R-HSA-2424477 显示 LAT 催化复合物、FCERI 与 DAP12/免疫系统路径；R-HSA-177945 显示 EGFR–SHC1 催化复合物与 EGFR/受体酪氨酸激酶通路路径（`/tmp/source-context-lat-reaction.json`、`/tmp/source-context-egfr-reaction.json`）。前者曾被 v28/v29 模型错误地当作 EGFR 直接证据，原失败结果保持。数据能区分上下文不代表模型已正确使用新字段，完整模型效果仍待验证。


`visualization_move_text` 提供独立文字的顶层数值 `dx/dy` 位移入口，必须传入项目、画布、父候选 ID、文档哈希和基础版本。它只修改指定的未绑定、未锁定、未旋转文字，不扩展原生分组；插图标签移动时保留图片、语义锚点、连接线、文字内容、换行宽度、字体、归属和素材文件。绑定容器的文字不支持此入口；已有 frame 归属保留，移动结果必须留在 frame 内。返回子候选而不采用，下一步从子候选 ID/哈希继续并检查 PNG。字符串位移及过期作用域在写入前拒绝。


原生 arrow/line/freedraw 的 width/height 来自点列外框，不是文字容器宽度。generic update 修改 points 时同步计算规范尺寸；若明确传入 width/height，必须与点列外框一致，不能只增大竖直箭头的 width 来排长标签。错误的几何修改原子拒绝，不生成子候选；应修改真实路径或分别调整文字排版。此校验只针对本轮明确更新路径几何的对象，不自动重写导入文档或无关旧对象。


保留参考图并追加流程时，使用 visualization 的 propose/revise_proposal + operations:[{op:"insert_sketch",plan:{message,audience,layout,nodes,edges,assumptions},id_prefix,x,y}]。Plan 仍需用户给出或明确确认科学内容；操作只负责追加原生形状、绑定文字和有向连接。x/y 定位新图整体外框左上角，前缀必须唯一；不替换已有图片/对象、文件、背景或历史计划。直接 sketch 生成整份新候选，不适用于保留现有内容的追加需求。

`figure_evidence` 支持 `record_batch`，通过原生 `ids` 数组并发读取 1–4 个不同的已发现记录 ID。每条结果保留原始查询 ID、规范来源 URL、记录哈希、投影与来源上下文，并单独标记 `status:ok/error`；输出顺序与输入一致。失败不会取消其他条目，也不表示机制不存在。成功记录的投影负载预算为 512 KiB（不含批次元数据与错误描述）；超预算条目明确标记 `retry_individually:true`，可用 `record` 单独读取。`all_requested_records_retrieved` 仅表示批次交付完整，不代表投影完整或科学主张已验证；图稿与计划不会因读取而修改。

局部候选编辑可用 `read_proposal` 的 `element_ids:["对象ID"]`（1–20 个不同的存活对象）。返回 `selected_elements` 原始完整对象、去除 dataURL 的 `selected_files`、最多 200 个且约 64 KiB 的其他对象原生几何上下文；点列最多 64 点，并明确报告省略。`proposal.document` 不出现在局部响应中，不能将投影当完整场景覆盖保存；候选哈希和 `scene_geometry_review` 仍对应完整不可变候选。选中对象和文件元数据合计超过 256 KiB 时明确报错，不截断。省略 element_ids 保持完整读取；仅 read/read_proposal 支持此参数。

真实局部读取验证：GUI 目录运行 `VISUALIZATION_LIVE_SOURCE_REVIEW_SCENE=/绝对路径/已保存的IFNG图稿.json VISUALIZATION_LIVE_PROVIDER=已配置供应商 VISUALIZATION_LIVE_MODEL=已配置视觉模型 npm run test:canvas:scoped-source-review:live`，使用隔离测试项目，不修改输入文件。`node tests/canvas-scoped-read-evidence.mjs /回归目录 [/旧回归目录]` 审计实际工具结果；提供旧回归时要求完整图稿哈希一致，不以视觉相同代替文档一致。v73 已通过实际 Agent/PNG/GUI 采用/源导出，字节数仅代表文本载荷，不代表 token 或延迟。

候选预览的对象反馈会自动为唯一存活的聚焦对象提供 element_ids；只在对照版本存在或已删除的对象不会生成不可读取的局部 ID，整图反馈继续完整读取。GUI 目录的 `npm run test:canvas:live-timing` 检查时间线的无正文元数据、批次聚合、按轮筛选与遗漏标记；真实 Gateway 回归保存每轮 stream-progress 和 timing。`node tests/canvas-live-timing-audit.mjs /回归目录 [轮次]` 从已保存时间线生成独立审计文件，缺失/截断/传输异常时明确失败，不补造阶段时长。统计为浏览器事件到达窗口，工具批次不是逐工具执行计时。

`test:canvas:gui-scoped-source-review:live` 在同样的供应商/模型/输入图稿配置下，实际选中 GUI 候选对象，测试需求不手写 element_ids，核对自动生成的读取参数是否被真实 Agent 使用；v75 已通过。`canvas-scoped-read-evidence.mjs` 对这种模式额外核对原始需求、GUI 草稿上下文和实际工具返回，防止以手工提示替代 GUI 集成证明。

正式画布现在同样支持 `read.element_ids`，必须带当前 `expected_revision`；过期或缺失版本明确拒绝。默认完整读取仍可省略版本，显式传入过期版本会拒绝。局部响应保留 metadata、scene 修订元数据、审阅意见与分页、规划/候选摘要和全图几何诊断，仅省略 scene.document，另给出 selected_elements/selected_files/geometry_context；scope.revision_scope 明确指向完整已保存修订，不伪造候选哈希。read 与 read_proposal 共享同一对象投影实现。GUI 正式画布 Agent 草稿的 follow_up_inputs.read 在完整且不超过20个对象的选区时自动带对象ID，空选区、超大或截断的选区使用完整读取，未保存变化仍先保存与核对。

正式图稿端到端回归可在 GUI 目录运行 `test:canvas:formal-scoped-source-review:live`（配置同上，输入为保留原范围短语的已保存IFNG图稿）。从正式图开始，不预创建候选，验证实际局部读取返回、propose、候选PNG、采用前正式图不变、GUI采用与可编辑导出。v76已通过，模型另有一次被禁用记忆工具拒绝的非绘图调用；不将单例通过解释为跨主题科学正确性。


## 查询新增对象的位置

`visualization_find_space` 是独立只读工具，参数为 `project_id`、`figure_id`、`expected_revision`、`relative_to`（现有对象ID）、`width`、`height`、`gap`；尺寸为1–4096，间距为4–256，均使用原生JSON数值。针对待审候选还需同时传 `proposal_id` 和 `document_sha256`，要求哈希、当前基础版本和 pending 状态一致；正式画布省略这两个字段。

`placement_review.candidates` 给出参考对象周边、画布内部障碍边缘及整图外侧的有限矩形建议，最多8项，按新增外框面积、距参考对象的远近排序，`bounds` 为实际坐标；重复位置去重。内部搜索每轴最多32个坐标，不保证找到所有可用空隙。每项额外返回 `resulting_bounds` 与 `resulting_aspect_ratio`，描述完整场景加上拟放入矩形的原生外框；面积最小可能使图过高或过宽，应结合本轮构图要求选择，不自动采用首项。这不是实际插图像素或PNG尺寸。不创建或预留位置，不移动对象，不返回场景/图片字节。frame整体外框也作为障碍，当前用于对象周边追加，不做容器内部打包或穷尽式布局搜索。任一对象外框无法可靠计算或超过5000对象覆盖上限时，返回 unavailable 与空建议；原生外框不等于字形/描边/自动frame名称的像素碰撞，仍须查看PNG。后续编辑沿当前修订或叶候选进行，位置查询不能绕过版本冲突。

素材仅用于区室或背景时，可独立 insert 并写可见范围说明；不要将细胞器图标替换到分子节点，或伪设 semanticAnchorId。GUI目录 `npm run test:canvas:context-art:contract` 仅验证合成保护契约；`test:canvas:context-art:live` 需要实际视觉模型和 `VISUALIZATION_LIVE_SOURCE_REVIEW_SCENE` 指向已保存的凋亡图，验证实际位置查询、检索/预览、独立插入、单图局部移动及来源导出。其完成状态以本次 result.json 为准，不把已保存图的素材化称为从空项目科学生成。

v83 已通过实际模型区域查询→BioArt预览/插入→单图移动→GUI采用/源与来源清单导出（约161秒、20调用1次恢复），原图逐字段保持。输入为显式已保存的v79图稿，不冒充同会话从空项目生成。独立BioArt插入的PNG哈希字段为customData.sha256；带图预览的SDK文本可附[image omitted]标记，评估只剥离这一已知尾标记，仍严格解析其JSON并核对实际文件哈希。





可用只读命令 `cargo run -p visualization --example find_space -- SCENE_JSON REFERENCE_ID WIDTH HEIGHT GAP` 检查显式保存的场景、图稿记录或原生导出（输入上限64MiB）。输出几何诊断与候选位置，不打开数据库、不写源文件、不下载素材。


`test:canvas:context-art:compact:live` 需 `VISUALIZATION_LIVE_SOURCE_REVIEW_SCENE` 指向显式保存的凋亡图；要求模型在位置查询结果中选择完整外框比例0.5–2.5内、增长面积最小的位置，而非固定首项。后续通过实际图片聚焦准备单对象局部修改草稿，验证读取/仅图片移动/采用/源与来源导出。已保存图的布局验证不代表从空项目生成或通用美观性保证。


`visualization image_info` 可读取待审候选图片：同时提供 `proposal_id`、精确 `document_sha256`、当前 `expected_revision` 和图片 `element_id`。候选必须 pending 且基础版本等于当前正式图；返回候选范围、自然尺寸、裁剪、像素哈希及来源元数据，不返回图片字节。省略两项候选引用仍只查正式图，不会猜测候选；`inset` 仍只作用正式版本，并明确拒绝候选参数。


素材修改成功结果在明确命中待审图片时会附带 `follow_up_inputs.image_info`，包含实际项目、画布、候选、文档哈希、版本和图片引用，可原样作为 visualization 输入。候选反馈聚焦存活图片时也会生成同样模板；文字或无法唯一匹配的焦点不会伪造图片查询。模板不包含修改专用 markup_ids，也不会自动执行。


`test:canvas:context-art:image-info:live` 在显式保存图上验证素材结果和图片聚焦草稿中的image_info模板被真实模型执行，核对候选范围、文件哈希、自然尺寸与无字节响应，再执行仅图片移动、采用和导出；需 `VISUALIZATION_LIVE_SOURCE_REVIEW_SCENE`。此模式要求实际元数据调用，不以仅出现模板作为通过依据。




计划 `layout_review.long_labels[].context` 提供声明的节点/面板归属；关系的 `cross_panel` 为 false/true/null，分别表示同面板/跨面板/未声明可比较归属。它是计划拓扑而非像素布局原因、来源验证或生物因果推断；不能从标签长短猜测跨面板关系。旧诊断可缺少 context。






远端版本变化且本地有草稿时，可点“合并未冲突修改”。按共同已保存版本三方比较，成功仅载入草稿并采用最新保存基线；同字段或并发结构变化有冲突时保留现有草稿并列出位置。它不是科学/图关系验证，检查后需另行保存；冲突详情中可逐项取舍。`test:canvas:plan-merge` 检查合并逻辑，`test:canvas:plan-updates` 包含实际外部写入与合并后版本保存验证。

合并失败后，计划面板展示检查时共同基线、本地草稿和远端版本的冲突值（有界、长值截断、数据字节隐藏）。快照不会随之后的输入变动，需调整草稿后重新检查；刷新或成功合并会清除快照。可逐项保留本地或采用远端；所有冲突作出明确选择后载入合并草稿，仍需另行保存。

冲突取舍应用前会重新核对本地草稿和远端版本，任一变化需重新合并检查；不会用旧快照覆盖新编辑。列表取舍采用完整列表，超过40个展示上限时须先减少冲突。`test:canvas:plan-resolve` 验证混合选择、本地/远端快照失效保护及显式保存后的历史/来源保持。

计划有未保存修改时可展开“保存前查看草稿修改”，相对当前保存基线列出字段前后值；合并后的远端已保存内容不重复列入。内容完全相同、仅对象字段顺序不同不会标成dirty；全部采用远端时可回到干净状态而无需多存一版。`test:canvas:plan-updates` 和 `test:canvas:plan-resolve` 覆盖草稿比较及干净状态回归。

长流程 `visualization_gateway_live` 宿主将隔离数据库保留在评估输出目录的 `runtime/` 中，`ready.json` 含该路径（文件含登录token，禁止公开）。已有评估目录仍拒绝复用；这便于中断后诊断，不提供自动续跑。长任务终端日志应定向到工作区中的独立日志文件，避免仅存 `/tmp`。


Reactome Content Service 的 JSON/文本/二进制 GET 遇429最多额外重试2次，尊重秒数或HTTP-date形式的 Retry-After，每次最多等待10秒；超过预算直接保留原错误，不截短服务端等待后重试。无头时等待1秒、2秒。404和Analysis提交不重试，限流持续时仍报告失败而非空来源。离线验证：`cargo test -p reactome --test rate_limit`。

单个待审候选的文字修改可使用 `visualization_reflow_text`：提供 `project_id`、`figure_id`、`proposal_id`、精确 `document_sha256`、当前 `expected_revision`、`title`、文字 `element_id` 和完整 `text`（以 `originalText` 为准）。可选 `max_width` 与 `font_size` 为 JSON 数值，省略时保留既有宽度预算与字号。该入口复用 `set_wrapped_text` 和候选修订校验，不自动采用；箭头文字沿原路径中心重排，形状内文字仍受容器限制。连续修改使用返回的子候选 ID 与哈希，最后检查 PNG 的重叠和裁切。批量修改仍使用 `revise_proposal.operations`。

真实模型回归 v200：在已有 EGFR 图稿的 GUI 候选反馈中，明确请求 `visualization_reflow_text` 将一条箭头标签改为宽度 180。实际调用 5 次、0 错误、耗时约 20.8 秒；先读取原候选与 PNG，再生成一个子候选并读取、预览，人工采用及可移植源文件导出通过。测试逐对象比较原文、字号、箭头中心与绑定、其他对象、文件及历史计划。此结果验证显式工具请求的调用链，不证明模型自主选工具、整图排版或科学正确性。回归入口为 `tests/helpers/canvas-live-caption-width.mjs`，设置 `VISUALIZATION_LIVE_TYPED_REFLOW=1`；默认仍允许原有批量操作入口。

`visualization_move_text` 支持两种明确的来源：同时省略 `proposal_id/document_sha256` 时，按当前 `expected_revision` 从正式图稿生成一份待审候选；两者都提供时，沿精确待审候选生成子候选。只提供一项或提供空字符串会拒绝。`dx/dy` 必须为顶层数值，移动单个未绑定文字而不展开分组，保留其他对象、字体、原文、来源和历史记录；不会读取任意未采用候选来代替正式版，也不会自动采用。

`visualization_reflow_text` 也支持直接从正式图稿生成一份待审候选：同时省略 `proposal_id/document_sha256`，提供当前 `expected_revision`、文字ID、完整 `text` 和需要调整的数值 `max_width/font_size`。已有候选修订仍须成对提供精确ID/哈希；半对或空字符串拒绝。来源不会自动选取未采用候选，沿用原生换行、容器与文字几何校验，正式图稿、其他对象和历史计划保持不变，人工预览后决定是否采用。


整体平移分面时，支持对已生成、无旋转、无粗糙和无圆角的原生跨分面箭头按当前组件/说明边界重布线，不再仅限分面交换或对齐操作。保留绑定端点、箭头方向、线型、依据和说明全文；锁定箭头/说明或无法满足路由约束时拒绝整次修改。路径和说明位置可能明显变化，必须查看子候选 PNG；此能力不推断科学顺序，也不保证消除折返或达到出版质量。

### 多概念 BioIcons 批量检索

`bioicons {action:"search_batch",queries:["receptor","protein","nucleus"]}` 一次执行1..8个独立词法查询，省略单数 `query`。每个查询最多256个UTF-8字节，不允许空白或忽略首尾空白/大小写后的重复项。各项返回独立的成功/错误、`total`、前4个结果、词法匹配分层统计和 `search_scope`；每个素材仍保留作者、许可、来源及目录哈希。共享目录缓存，不下载SVG，也不读写图稿。查询错误不表示零命中，零命中也不表示目录不存在对应科学组件。需要更多候选时用原 `search` 细化，选用前仍须 `preview` 并核对科学含义。此能力只减少可合并查询的Agent往返，不证明真实模型延迟下降。

批量检索的共享目录若加载失败，仅记录当前查询为 `error`，本批剩余查询标为 `not_attempted`、原因 `shared_catalogue_unavailable`，不会把同一目录故障逐查询重试。`failed` 仅计实际失败项，`not_attempted` 单独计数；两者均不能解读为零命中。`all_queries_succeeded` 只表示每个请求的检索成功，不表示已返回全部匹配或完成目录语义普查。后续新调用仍可重新尝试加载目录。

### 显式选择 SVG 预览采样贴合

`visualization_fit_svg_preview` 接收当前 `project_id/figure_id/proposal_id/document_sha256/expected_revision`、`title`、图片或锚点 `element_id` 及当前 `file_id`，在后端读取该精确候选的 SVG 预览非零 alpha 包围盒（一个预览像素边距），沿标准修订路径生成子候选，无需手工路径求并集或输入归一化坐标。回复 `svg_preview_fit_basis` 保留实际使用的数值范围、文件哈希和采样限制，不返回图像字节。

只支持未裁剪、未旋转且预览非空的 SVG；不支持的图像、过期版本/哈希/文件身份和非叶候选会拒绝，不回退到其他图稿或猜测范围。已有明确审阅矩形或栅格图时继续使用原 `visualization_fit`，其必填数值字段不变。新工具禁止多余手工边界参数，避免静默忽略。原始候选、源文件和科学信息保留，是否能在现有分面内合法贴合仍由原操作检查。背景与装饰也会进入采样范围；这不是精确矢量轮廓、科学组件识别或分子结构验证，仍须核对生成子候选 PNG，不自动采用。

已有独立状态图形可在 `propose/revise_proposal.operations` 中显式关联：`{op:"attach_illustration_badge",id:"badge-image",anchor_id:"existing-anchor",meaning:"用户明确指定的状态标记含义"}`。主插图保持不变，建立双向附属引用并加入原生组件组，保留素材来源；双方必须已有同分面归属，标记完整处于分面内且未被其他组件占用。后续组件平移/复制会携带标记；需要单独解除时用 `{op:"detach_illustration_badge",id,anchor_id}`，不删除或移动图形，不抹去含义及来源。请从当前候选实际对象读取身份，不猜测哪个组件处于该状态；归属记录不是科学适用性验证，不可用单个状态符号冒充完整复合物。


在 `propose/revise_proposal.operations` 中用 `{op:"position_arrow_caption",id:"bound-label-id",position:0.5}` 将说明移到从起点沿路径长度的一半处；保持实际路径与科学内容，通过共线控制点重新分配实现原生定位。比例须严格介于0和1；只支持单说明、未旋转未锁定的直角锐角箭头，拒绝曲线、斜线、肘形与分面外结果。使用精确候选身份生成待审子候选，核对PNG及全图遮挡后再人工采用。
