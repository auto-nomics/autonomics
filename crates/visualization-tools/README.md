# visualization-tools

Agent 工具适配层，独立于 runtime 和 HTTP 服务。Runtime 使用 `registrations(database_path)` 与 `bioart_registrations(database_path)` 注册；二者使用与 Gateway 相同的项目画布数据库。

- visualization：projects、list、read、read_image、read_preview、create、sketch、propose、apply。
- bioart：search、preview、insert、replace；图片数据和来源信息一同保存。

优先使用 propose 供用户在 GUI 预览和采用；所有修改必须使用已读取的版本。GUI 未保存内容不会自动暴露给工具。

验证：`cargo test -p visualization-tools`。工具输入 schema 与注册依赖通过真实 agentik 宏编译，不依赖整个 runtime 的计算节点依赖。

`read` 保留图片文件元数据而省略 dataURL；`propose` 只返回提案元数据，避免将图片字节塞入文本上下文。`read_image` 按 element_id 读取已保存的图片对象，以 SDK 图像内容块返回 PNG/JPEG/WebP；可携带 expected_revision 检查版本。遵守运行时 6 MiB base64 传输限制，超过时要求先缩小参考图。图像传递需要模型声明视觉能力且运行时未禁用工具图像。`read_preview` 读取 GUI 为当前版本生成的整图 PNG，缺少当前版本截图时明确报错，不回退到旧图。工具本身不启动浏览器渲染。

`bioart replace` 在指定修订上将占位图升级为素材组，保留语义锚点和连线并返回提案元数据，用户须在 GUI 对照预览后采用。不会将候选文档中的图片字节返回给模型。

`bioart preview` 在修改画布前返回素材图像及来源、作者、许可、校验值，支持视觉模型核对素材；不创建画布修订。GUI 素材面板也提供预览，并复用已加载图片进行插入。

在活动画布上规划草图时，应传 figure_id 和 expected_revision；返回的是该记录的待审阅提案，正式图稿与其他记录保持不变。只有明确需要独立新图时才省略 figure_id。
