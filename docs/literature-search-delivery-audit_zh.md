# 文献检索 P4/P5 交付核对

2026-10-28 交付状态：P0–P5 已按阶段各提交一次：P0 `01452ed`、P1 `032a685`、P2 `871890f`、P3 `222a37d`、P4 `f2a5a41`，随后为 P5 阶段提交。按用户最后确认的“基本验收、保证可用”收口；下面记录已验证功能和仍需如实保留的性能/平台/来源范围限制。

## 验收依据

以 [路线图第 6–9 节](literature-search-roadmap_zh.md)、[P4 设计](literature-search-p4-design_zh.md)、[P5 设计](literature-search-p5-design_zh.md)及[订阅细化](literature-search-p5-subscriptions-design_zh.md)为依据，保留原始范围。

P4 设计明确允许索引落后时返回显式状态，要求该状态不阻断词法查询和存储写入。60.74 秒持续写入验收已核验词法与写入可用、陈旧代不发布、同步显式冲突及停止写入后收敛；结构性测试不代表生产吞吐。

路线图的 10k/100k 热查询时延使用“争取”措辞。仍须报告实际首次、热查询、并发、维护及资源数据；未达到目标要如实记录，不能用排序微基准代替 API/GUI 测量，或声称已经满足目标。

## P4

| 方案要求 | 已有证据 | 收口状态 |
| --- | --- | --- |
| 模型/分块比较、冻结配置、独立中英评价、组别退化 | [英文 SciFact](benchmarks/literature-search-p4-scifact-linux.json)、[中文 NeuCLIR](benchmarks/literature-search-p4-neuclir-hybrid-linux.json) | 已有独立收益；保留中文 18 条查询退化及材料覆盖限制 |
| 专用启用、模型 digest、远端显式配置、输入范围、超限拒绝、参数身份 | [长输入](benchmarks/literature-search-p4-long-input-linux.json)、[推理配置](benchmarks/literature-search-p4-inference-options-linux.json)、[资源预算](benchmarks/literature-search-p4-resource-limits-linux.json) | 已实现并通过本地配置、资源与输入边界验证；按 P4 阶段边界提交 |
| 专用 worker、短事务、提交批次、原子发布、增量复用与删除失效 | [有界清理和完整多附件运行](benchmarks/literature-search-p4-bounded-cleanup-linux.json) | 100k 篇/200k 附件/700k 块完整功能通过；排序终版通过索引/API专项回归 |
| 硬约束与权限前置、精确 ID 不退化、版本游标、真实证据 | Rust 索引/服务/工作流测试、[混合资格](benchmarks/literature-search-p4-hybrid-linux.json)、[附件资格](benchmarks/literature-search-p4-attachments-linux.json) | 已有端到端功能及回归证据；P4/P5 前后端接线分阶段提交 |
| 故障/重建/更换模型/内容变化/进程恢复/数据库分支恢复 | [恢复](benchmarks/literature-search-p4-recovery-linux.json)、[维护](benchmarks/literature-search-p4-maintenance-linux.json)、[资源](benchmarks/literature-search-p4-resource-limits-linux.json) | 所列恢复与资源场景通过；没有在提交后重跑全套候选矩阵 |
| 10k/100k 规模与端到端性能、内存/持久体积 | 各容量收据及完整 700k 块运行 | 规模/资源已有测量；共享主机宽查询及四客户端目标未达到，精确排序微基准不能替代终版 API 重测 |
| 持续写入、索引落后状态、词法/写入可用、最终收敛 | [持续写入收据](benchmarks/literature-search-p4-sustained-write-linux.json) | 60.74 秒、至少 100 次写入通过；词法读写持续可用，旧向量代不发布，暂停后收敛 |
| GUI 状态、模型范围、恢复、证据定位、实际 Linux 原生 | [原生 GUI](benchmarks/literature-search-p4-isolated-gui-linux.json)及容量/维护记录 | Linux 原生和语义 GUI 已验证；不据此声明其他操作系统或打包应用资格 |
| 一个 P4 提交，排除 P5 与用户其他改动 | 已保存并核对 P4/P5 接线边界 | 已提交 `f2a5a41`；未将工作区其他 GUI/阅读器改动纳入 |

## P5

| 方案要求 | 已有证据 | 收口状态 |
| --- | --- | --- |
| 引用/被引方向、上下文、未解析记录、来源时间、相似推荐单独标识 | [探索](benchmarks/literature-search-p5-exploration-linux.json)、[真实原生](benchmarks/literature-search-p5-native-linux.json) | 本地 SDK/HTTP/GUI 闭环通过，公网完整性不作保证 |
| 精确身份重新获取、预览确认、目标集合、保留用户元数据 | [GUI](benchmarks/literature-search-p5-subscriptions-gui-linux.json)、[原生](benchmarks/literature-search-p5-native-linux.json) | 本地流程和实际 Linux 原生验证通过 |
| 不可变执行捕获、来源/版本/覆盖、幂等、完整性及预算、JSON/RIS/BibTeX | [存储](benchmarks/literature-search-p5-snapshot-storage-linux.json)、[产品](benchmarks/literature-search-p5-snapshots-product-linux.json)、[原生导出](benchmarks/literature-search-p5-native-linux.json) | 已有证据；原始文件不属于元数据快照 |
| 持久订阅、时间语义、源扫描、部分失败不冒充穷尽、水位/重放/额度 | [调度器](benchmarks/literature-search-p5-scheduler-linux.json)、[执行器](benchmarks/literature-search-p5-subscription-execution-linux.json)、[来源兼容](benchmarks/literature-search-p5-source-compat-linux.json) | 本地协议和部分公网契约通过；逐源最终覆盖清单待审查 |
| 幂等新增/更新通知、重启不重复、不自动导入、不发送外部消息 | [持久存储](benchmarks/literature-search-p5-subscriptions-storage-linux.json)、[真实进程恢复](benchmarks/literature-search-p5-subscription-recovery-linux.json) | 已有故障/接管/关闭数据库家族恢复证据 |
| GUI 规则/计划/历史/通知/预算、显式取消、视图关闭不停止后台任务 | [浏览器与桥接](benchmarks/literature-search-p5-subscriptions-gui-linux.json)、[真实原生](benchmarks/literature-search-p5-native-linux.json) | 通过；不冒充打包应用或其他平台发布资格 |
| 一个 P5 提交，排除其他工作区改动 | 已保存独立文件与混合接线边界 | 已单独提交；未将 Agent、阅读器、构建产物等其他工作区改动纳入 |

跨平台打包、供应商全集覆盖、私人真实库与更广科研任务的质量不由当前 Linux 本地验证推出；作为发布范围和限制单独记录，不用这些未取得的证据声称完成。
