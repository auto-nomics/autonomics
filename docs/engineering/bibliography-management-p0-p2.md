# 文献管理 P0–P2 与浏览器导入插件

本轮范围：文献可靠性、迁移和整理、引用输出，以及浏览器导入插件。按用户调整，Office 插件、文献库离线同步和群组协作不在本轮范围内。

## GUI 入口

- 列表顶部 **New article**：无需 DOI 即可创建记录。
- 详情栏 **Edit metadata**：编辑常用字段、文献类型和类型相关字段，增删和排序作者。书籍支持出版者、出版地、版次；章节支持书名；学位论文支持学校；报告支持机构和报告类型；专利支持专利号；网页支持站点、URL、访问日期。
- **Import article**：识别 DOI、PMID、PMCID、arXiv、OpenAlex 及相应官方 URL。
- **Article trash**：分页浏览、恢复、永久删除。普通删除保留附件、笔记和集合关系；永久删除单独确认。
- 附件行：下载、设为首选、删除。上传可选择多个文件；补充附件不替换已有首选文件。部分失败显示具体文件。
- 选中文献后 **Organize**：批量增删标签、加入或移出集合。逐条报告结果，失败重试跳过已成功的记录。
- **Import file**：BibTeX、RIS、Zotero JSON 元数据预览，选择条目后导入；不覆盖已有记录。
- **Migrate Zotero library**：选择 Zotero JSON 和用户明确提供的导出目录，迁移可识别的集合、笔记及文件，查看报告并重试。
- **Find duplicates**：审阅标识符或规范化标题相同的候选，选择保留记录和元数据，再确认合并。合并包含标签、标识符、笔记、附件及集合归属；不同标识符冲突需先修正。
- **Export and cite**：操作勾选记录，未勾选时操作当前记录；支持 BibTeX、RIS、CSL-JSON、Markdown，以及本地 CSL 引擎的 APA、NLM/Vancouver、GB/T 7714 格式，提供复制和下载。
- 拼图图标：浏览器导入插件安装说明。插件支持 Chromium 开发者模式加载，详见 [插件说明](../../apps/bib-connector/README.md)。

## 数据与行为边界

元数据详情返回内容指纹 `revision`。GUI 保存提交 `expected_revision`，后端在事务内比较；过期编辑返回 409，草稿保留，可以显式加载最新记录。旧客户端不带版本的 PUT 仍兼容接受，因此这不是所有旧客户端的强制版本协议。类型相关字段存储在 `Article.extra_fields`，空映射保持既有序列化行为。

回收站记录从活跃列表、检索、计数与导出排除；恢复不重新导入文件。永久删除无法撤销。后端数据库与对象存储不共享事务，对象清理失败会记录日志，详见 [可靠性接口](bibliography-reliability.md)。

Zotero 迁移支持可识别的 JSON 结构和用户选中的相对路径文件。不会根据 JSON 中的绝对路径读取本机文件。笔记转换为纯文本，保留来源标记；不迁移 Word 动态引文字段、所有第三方插件字段或任意富文本排版。不同源库使用不同的来源标签，同一源库重试复用标签。文件以内容哈希去重，保留已有首选附件。未提供、路径不安全、超限或不支持的资源逐项报告；完成元数据导入不代表所有附件都已迁移。

浏览器插件提取通用学术元数据、JSON-LD 或普通网页标题描述，需要用户预览确认；不自动下载 PDF 或保存网页快照。插件 token 保存在浏览器会话存储，网页脚本拿不到该 token。使用现有网关授权范围，不是另行签发的仅导入 token。未发布到浏览器扩展商店。

CSL 使用仓库内固定的样式和 locale，在本地格式化；不宣称支持 Zotero 全部样式、类型或插件生态。内部写作引用生成读取当前文献元数据；已有文档中的引用键不会因作者或年份改名而自动重写。

## 可重复验证

```bash
cd apps/gui
npx tsc --noEmit
npm run test:bib-management
npm run test:bib-management-native
node tests/bibliography-search.mjs
node tests/bibliography-workflow.mjs
```

```bash
# 仓库根目录
cargo check -p autonomics-gui --locked
cargo test -p bib-base --test reliability --test library_transfer --locked
cargo test -p api-server --test bib_reliability --locked
node --test apps/bib-connector/tests/*.test.mjs
cd apps/bib-connector
npm run build
```

真实后端集成测试只使用 loopback、内存文献库及临时对象存储。浏览器测试部分使用模拟 HTTP，真实后端测试覆盖迁移接口边界、文件内容及恢复行为。桌面桥接有编译检查和调用参数验证；目标机器的原生文件对话框与浏览器权限弹窗仍需人工验收。

## 本轮验证记录

- GUI：生产构建通过；菜单与元数据/批量/回收站回归、迁移预览与逐字段合并、三种真实 CSL 样式渲染、Zotero 迁移模型和 GUI 流程均通过。
- 真实 API：内存库与临时对象存储的迁移、重复执行、原文件下载、回收站恢复及过期版本拒绝通过。
- 检索回归：V2 搜索、Unicode 高亮、过滤、导出、浏览器与 Tauri 桥接工作流通过。
- Rust：桌面桥编译检查通过；可靠性、导入/合并、解析器/API、内部写作读取最新元数据的针对性测试通过。既有 API 测试 24 项通过，1 项依赖外部提取器的既有测试仍忽略。
- 插件：5 项自动化测试通过；Chromium 加载检查通过；两次 ZIP 打包哈希一致。

以上不等于 Windows/macOS 安装包或浏览器扩展商店发布验收，也不等于私人真实库的全量迁移保证。迁移前的预览和迁移后的逐项报告是实际完成范围的依据。
