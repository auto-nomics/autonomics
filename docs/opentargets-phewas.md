# Open Targets 变异关联查询

`opentargets_phewas` 接收上游文件读取节点产生的 DataFrame，默认读取 `variant_id` 列；配置示例：`{"id_column":"variant_id","max_pages":1000}`。输入必须是已完成映射和参考等位基因核验的 GRCh38 `chr_pos_ref_alt`；节点不会将 GRCh37 rsID 转成 GRCh38。无效格式（包括未映射 rsID）记为 `unresolved_id`。重复 ID 合并查询，空值汇总到审计表。

输出端口 0 是逐 study locus 的关联表，不要求存在 L2G 预测。`input_id` 是查询变异，`lead_variant_id` 是 API 返回的实际 lead；`relationship` 为 `lead`、`non_lead` 或 `unknown`。`pvalue_mantissa` 和 `pvalue_exponent` 是字符串，P 值为 mantissa × 10^exponent；保留两个分量以避免极小值下溢。其他数值列也以字符串提供。样本祖源和 LD 人群组成以 JSON 保留，分别来自 study 的 `discoverySamples`、`replicationSamples` 和 `ldPopulationStructure`。这些是研究级元数据，不等同于单一祖源关联结果。空值表示 API 未提供。

输出端口 1 是逐唯一 ID 的审计表：`reported_count`、`returned_count`、`pages_fetched` 和 `error`，状态包括 `complete`、`no_results`、`unresolved_id`、`failed`、`incomplete`、`inconsistent_count` 和 `truncated`。失败或截断时已取回的行仍在端口 0，分析前应检查端口 1。计数完成只证明当前 API 请求的分页完成，不能证明历史数据集的关联覆盖完整。节点不自动重试，也不自动过滤欧洲祖源。建议保留原始输入表、映射结果、端点、执行时间及 OT 数据版本，并按 input ID 连接审计表。

2026-11-06 对官方 API 的实测：`17_66232877_G_C` 查询总数为 1309，page size 1 的前两页分别返回该变异和另一个变异作为 lead；首条研究的 discovery samples 同时包括欧洲和东亚样本。另以 page size 100 完整请求了 14 页，返回 1309 条、1309 个不同 study locus ID。这验证了该示例的分页，并不验证全部研究或锚文数据库等价性。官方说明 variant API 返回包含该变异的 credible sets，而复杂系统查询可使用 BigQuery 或数据下载：<https://platform-docs.opentargets.org/data-access/graphql-api>、<https://platform-docs.opentargets.org/data-access>。当前结果不能直接宣称复现锚文 PheWAS；是否需要 BigQuery 或快照仍需与锚文数据版本和定义逐项比较。
