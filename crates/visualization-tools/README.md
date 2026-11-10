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
