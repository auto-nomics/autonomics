# ADR: 本地全文索引引擎

- 状态：P0 原型结论，待 P1 的兼容性、迁移与性能门槛验收后进入生产。
- 日期：2026-10-27。
- 适用对象：本机/服务端文献库的本地词法检索索引。

## 决策

P1 首选验证 **Turso 原生 FTS**，暂不添加第二个直接 Tantivy 依赖。项目使用 `turso = 0.7.2`，Cargo 默认特性包含 FTS；但 `CREATE INDEX ... USING fts` 属于实验性索引方法，必须在 `Builder` 上设置 `.experimental_index_method(true)`。生产数据库在完成针对现有文件、迁移与目标平台的测试前不得启用。

P0 临时原型已在单独内存数据库中验证以下契约：创建字段索引、`fts_match` 查询、`fts_score` 得分、短语匹配和 `fts_highlight` 标记。对应测试为 `turso_fts_supports_scoring_phrase_queries_and_highlights`。该测试证明当前锁定 crate 可用时的能力，不证明现有 `BibCore` 已启用、不证明磁盘库升级安全，也不构成性能数据。

## 理由

- Turso 自身 FTS 已以 Tantivy 为引擎，提供词项位置、BM25、布尔/短语匹配和高亮；直接再加 Tantivy 将增加索引目录管理、双重恢复、额外同步和跨平台写入测试成本。
- FTS 使用现有数据库、对象 key 和事务边界，适合沿用当前 Zotero 对象模型与 Turso actor。
- 当前已有 `searchTerms` 可保留为兼容回退和逐阶段对照；待检索结果 parity 和故障恢复验收后再停用。

## 生产启用前的验证项

1. 检查 `turso` 锁定版本和平台构建实际包含 FTS；在 Linux、macOS、Windows 的正式桌面/服务端打包配置运行契约测试。
2. 建立含标题、摘要、作者、期刊、年份、标识符、关键词、附件段落、批注的独立文档索引。评分时保留 title/abstract/body 权重，元数据过滤和全文正文不能造成错误跨对象合并。
3. 验证修改、删除、批量导入、附件替换、增量更新和进程异常退出后的索引一致性，以及 `OPTIMIZE INDEX` 是否需要和如何调度。
4. 针对当前真实迁移方式验证实验性索引持久化、文件升级/降级兼容和备份恢复。建立 generation 切换或显式回退路径。
5. 以 10k 与 100k 文献及实际全文规模比较当前倒排、Turso FTS 和必要时独立 Tantivy 的索引时间、磁盘、内存、P50/P95 查询延迟。
6. 确认 Turso FTS 的查询语法由 Autonomics AST 安全生成，不把未转义用户输入直接拼进 FTS DSL；检查高亮返回的标记内容按纯文本解析和安全渲染。

## 后续选项

如实验性 API 的版本兼容、平台支持、全文同步语义或基准未达 P1 门槛，使用独立 Tantivy 目录作为派生索引备选，由 outbox/revision 驱动并可从数据库与 VFS 重建。不要同时维护两种生产索引，除非迁移期明确设定终止条件。

## 参考

- [Turso 0.7.2 Cargo features](https://docs.rs/crate/turso/0.7.2/features)：默认启用 `fts`。
- [Turso 0.7.2 Builder API](https://docs.rs/turso/0.7.2/turso/struct.Builder.html)：实验性索引方法开关。
- [Turso FTS 手册](https://github.com/tursodatabase/turso/blob/main/docs/manual.md)：FTS 查询、评分和高亮语义。当前线上手册可能随版本变化，生产验收以 0.7.2 契约测试为准。
- [Tantivy 位置索引文档](https://docs.rs/tantivy/latest/tantivy/positions/index.html)：词项位置支持短语查询。
