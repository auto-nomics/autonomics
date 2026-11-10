# visualization-tools

Agent 工具适配层，独立于 runtime 和 HTTP 服务。Runtime 使用 `registrations(database_path)` 与 `bioart_registrations(database_path)` 注册；二者使用与 Gateway 相同的项目画布数据库。

- visualization：projects、list、read、read_image、read_preview、read_markup、read_plan、save_plan、sketch_plan、create、sketch、propose、apply。
- bioart：search、preview、insert、replace；图片数据和来源信息一同保存。

优先使用 propose 供用户在 GUI 预览和采用；所有修改必须使用已读取的版本。GUI 未保存内容不会自动暴露给工具。

验证：`cargo test -p visualization-tools`。工具输入 schema 与注册依赖通过真实 agentik 宏编译，不依赖整个 runtime 的计算节点依赖。

`read` 保留图片文件元数据而省略 dataURL；`propose` 只返回提案元数据，避免将图片字节塞入文本上下文。`read_image` 按 element_id 读取已保存的图片对象，以 SDK 图像内容块返回 PNG/JPEG/WebP；可携带 expected_revision 检查版本。遵守运行时 6 MiB base64 传输限制，超过时要求先缩小参考图。图像传递需要模型声明视觉能力且运行时未禁用工具图像。`read_preview` 读取 GUI 为当前版本生成的整图 PNG，缺少当前版本截图时明确报错，不回退到旧图。工具本身不启动浏览器渲染。

`bioart replace` 在指定修订上将占位图升级为素材组，保留语义锚点和连线并返回提案元数据，用户须在 GUI 对照预览后采用。不会将候选文档中的图片字节返回给模型。

`bioart preview` 在修改画布前返回素材图像及来源、作者、许可、校验值，支持视觉模型核对素材；不创建画布修订。GUI 素材面板也提供预览，并复用已加载图片进行插入。

在活动画布上规划草图时，应传 figure_id 和 expected_revision；返回的是该记录的待审阅提案，正式图稿与其他记录保持不变。只有明确需要独立新图时才省略 figure_id。

`read_markup` 按 markup_id 返回独立保存的草图审阅图与说明，明确原始 at_revision 和当前 current_revision。标记使用划除、补画和强调三类几何；不作为正式场景对象写入。读取旧标记时必须与当前文档核对，不能把历史图当作当前整图预览。`read` 附最近 50 组标记摘要，省略详细笔迹和图片字节。

sketch.document 支持 layout=grid（默认）、horizontal、vertical。网格模式要求唯一 column/row；自动模式省略坐标，按显式边分层，保留反馈和并行关系。图稿仍作为可审阅提案进入活动画布。自动布局不会补充科学关系；环内节点输入顺序影响前向/反馈通道。

科研规划使用 read_plan / save_plan / sketch_plan。save_plan.document 保存科学图意、受众、组件、关系、假设及问题/答案，可在信息不完整时持久化；生成要求 issues 为空且计划与图稿版本一致。planning_version=0 创建，后续使用 read_plan 返回的 version。更新和生成均须 expected_revision；旧版写入返回冲突。已有计划的画布必须使用 sketch_plan 生成草图，不能用直接 sketch 跳过问题历史。普通 read 只附计划摘要。

多面板计划：`panels:[{id,title,column,row}]`，每个 node 的 `panel` 引用所属面板，组件 ID 全图唯一。各面板独立布局；显式跨面板 edges 生成绑定实际组件的场景级连接线，正交路径避开节点和标题，保留方向、抑制与依据。输出保留原生 frameId，更新 frame 的 x/y 会整体移动内容并更新外部箭头端点（不会在每次移动时自动重新避障），删除 frame 则保留内容并解除归属。GUI 绘图计划与 save_plan/sketch_plan 使用同一结构与版本审阅流程。

`{op:"reroute",id}` 仅整理现有生成式跨面板箭头；可放在 propose.operations 中，跟在移动操作之后执行。以当前绑定和当前标签尺寸重新避障，保留文字、样式、ID、科学依据及无关对象。操作失败时整批回滚，不从旧计划重建图稿。端点解绑、旋转的绑定组件或无效几何需先处理；普通手绘箭头不在此操作范围。GUI 的“整理跨面板连线”使用相同操作并进入提案审阅流程。

`bioicons` 提供 search/preview/insert/replace，用于开放 SVG 素材。preview 返回 PNG 图像块与逐素材作者/许可/转换说明；insert 与 replace 都生成待审阅提案，要求 project_id、figure_id、expected_revision，replace 还需 element_id。保留原文件和规范化文件哈希；SVG 内部路径不是独立画布对象。`visualization read_image` 可将保存的 SVG 安全转换为 PNG 供视觉模型查看，原文件保持 SVG。

`bioicons palette` 需 project_id、figure_id、expected_revision、element_id，可读取图片或语义锚点的实际颜色与 PNG 视觉预览；`recolor` 另需 colors 对象（`#RRGGBB` 映射），创建配色提案。动作不访问目录网络，也不需要 asset_id。仅修改目标 SVG 实例，保留来源许可、文字路径、位置与其他实例，并记录前后哈希及颜色映射。先读 palette，勿猜测源颜色；相同颜色的路径一起替换，不能单独指定同色路径。


`visualization list_markups` 可按 `status: "open" | "resolved"` 筛选审阅历史，并通过 `limit`（1..100，默认 50）和 `before` 翻页。结果为 `{items,next_before}`；重复使用返回的游标直到 null，避免把最近一页当作完整历史。`read` 中的 `markups_pagination.next_before` 表示仍有较早标记。列表只包含摘要，具体笔迹与图像仍通过 `read_markup` 读取并核对历史版本。


`visualization propose` 的可选 `markup_ids` 关联实际回应的待处理意见（最多 50 个不同 ID）。先使用 `read_markup` 查看并核对历史图稿，再关联对应 ID；不要关联尚未处理的意见。结果包含无图像字节的 `review_markups` 摘要。该操作不关闭意见，GUI 用户可在采用时显式选择完成关联标记；`apply` 不接受非空 `markup_ids`，不能绕过审阅采用来关闭意见。


`bioicons parts` 读取当前 SVG 的路径编号、边界及编号视觉结果；`edit_parts` 使用该结果的 `sha256` 和 `edits:[{part_id,fill?,stroke?,opacity?,translate?}]` 生成局部样式提案。`fill/stroke` 允许 `#RRGGBB` 或 `none`；`opacity` 是相对当前效果叠加的 0..1 系数。先查看编号图，不得把路径编号当成科学组件名称。采用后编号失效，必须重新读取；每次最多 32 条编辑，素材最多 256 条非资源路径。`translate:[dx,dy]` 使用 SVG 根坐标，后台补偿上层缩放、旋转与镜像；视口和共享上层裁剪不移动，移动后的超出部分仍会裁掉。路径控制点与形变暂不支持，图片对象外部的标签、绑定和其他实例保持不变。


`bioicons geometry` 使用 `project_id/figure_id/expected_revision/element_id` 及 `parts` 返回的 `sha256/part_id` 读取端点与曲线控制点（编号 PNG + 无图像字节的 JSON 摘要）。`edit_parts` 可传 `points:[{index,position:[x,y]}]`，坐标为当前快照的 SVG 根坐标。只改指定点，不自动平滑；同一路径不可同时传 `points` 和 `translate`，采用后重新读取编号。路径最多 512 个点，单次最多改 64 个不同点，退化导致拓扑改变会被拒绝。编号是图形坐标，不是生物学身份。

`image_info` 在明确 `element_id` 和 `expected_revision` 后返回图片实际尺寸、裁剪和来源（不返回嵌入文件）。`inset` 接收同一版本与 `inset: {element_id,region:{x,y,width,height},x,y,max_size,title}`，生成待审阅的局部放大快照提案。region 为当前可见图片镜像后、旋转前的比例区域；max_size 为 32..2048。使用前应 read/read_image 检查源图。复用原始文件，不栅格化 SVG；保留许可和版本来源。快照不会随原图移动或修改自动刷新，区域框为独立注释。

删除整个放大图时只 remove 其 `customData.inset.anchor_id`：会清理归属匹配的区域框、引导线及内容组，保留源图、其他快照及独立注释。不要再显式 remove 已级联删除的 ID。

`inset_status` 使用 `element_id`（原始放大图锚点或图片 ID）和 `expected_revision` 比较源图与历史快照，不返回图片字节；`changed` 不单凭画布版本判断。`refresh_inset` 用相同参数生成待审阅提案，保持原比例区域与稳定 ID，保留边框、标题、样式和其他对象；源图变化可能改变比例区域的科学含义，需先 read_image/read_preview 核对。若 `detail_edited` 为真，只有显式指定 `discard_detail_edits:true` 才会在提案中替换放大图独立修改的文件/裁剪。源图缺失、归属不明和旋转后的放大面板须先解决；刷新仍是固定版本快照，不是后台自动同步。

`arrange` 接受 `arrangement:{element_ids:[...],mode:left|center|right|top|middle|bottom|distribute_horizontal|distribute_vertical}` 和精确 `expected_revision`，返回待审阅提案（不返回嵌入图稿字节）。读取完整明确的目标后操作；原生组合、语义锚点及所属标签/素材一起移动，父子重复选择会合并；分布至少三个独立组件。空间不足时沿用编辑器的中心退化方式，仍需核对重叠。可携带 `markup_ids` 关联实际回应的标记，采用时才按用户选择关闭。排除放大区域/引导线注释，先处理锁定或缺失对象；支持手绘点列与无抖动、无圆角的线段/折线边界；被选组合内部的绑定连接线必须与端点同幅移动，不能为满足外部绑定而拉伸组合。分开移动的曲线/旋转/肘线、粗糙连接线和旋转箭头绑定文字仍需原生几何适配。不推断关系、不自动避障，保留来源和文件；必要时另行 reroute。

`read_proposal_preview` 使用 `proposal_id` 和当前 `expected_revision` 请求候选图 PNG，最多短暂等待约 6 秒。返回 `scope=proposal_candidate`、文档 SHA、基础/当前版本、提案状态、renderer 和独立图像块；候选没有因此采用。检查 `base_is_current`，不要把历史候选当作当前文档。相关画布需保持打开（内部切到 Agent 标签可继续后台渲染）；未就绪或失败明确报错，不回退到正式画布图像。资源修复后可用 `retry_preview:true` 重排失败请求；成功图像不可覆盖。模型看图与选择正确修正动作仍需真实案例验证。
