# Cachexia v3.9 修复账本复审

本报告审核 `fix-ledger.md` 的全部 19 项交付声明：F01–F15，以及 MAGMA GLS、MAGMA meta 连线、GenomicSEM 远尾分位数、表格类型渲染。结论：**不能验收为“全部修复”，也不能认证为“全部与官方定义一致”**。已经存在有效修复，但新增反例确认修复分支仍有结果错误；文档澄清、测试通过、源码检出、运行态更新是不同的验收层次。

复审日期：2026-10-05。环境在审查过程中被其他会话持续更新，部署观察冻结到北京时间约 09:14；之后的部署状态需要重新核验。复审未修改业务代码、原账本、插件 checkout、安装清单或服务。

## 范围与证据等级

- 原目录登记 397 个 node kind。这份修复账本只覆盖其中部分节点及共享基础设施，不是 397 个节点的完整算法验收。本次未重新逐行审核其余全部节点；其余节点不得因本账本而改标“已修复”或“官方一致”。全量目录仍见 `node-catalog.md`、`nodes.tsv`。
- “原缺陷修复通过”只表示指定缺陷和所测输入通过，不表示节点所有参数、所有输入或完整研究计划通过。
- Python/R 测试为本次独立重跑。Rust 针对性测试使用账本共享 target 中已编译的测试二进制；新增两个 Rust 反例链接这些修复产物运行。没有把它们表述为从当前 main 全量重新构建。
- 官方比对区分真实官方包调用、官方格式源码、独立数学参考，以及尚未做官方整条流水线比较。
- 审核账本 SHA256：`fc1f1ac2749335f35bea9508b2086ecb15daeb321ec3c8a5747cb72085e9116a`。

## 全部交付项对账

以下结论针对账本列出的修复源码，而非假定它们已进入运行中的服务。DF 表示 Arrow/DataFrame。

| 项目 | 节点或基础设施及作用 | 输入 → 输出 | 修复判断与官方一致性边界 |
|---|---|---|---|
| F01 | `h5ad_ucell_score`：逐细胞签名评分 | H5AD、gene_sets、截断秩等参数 → 加分数列的 H5AD、JSON 报告 | **部分修复**。正向签名公式通过 UCell 2.16.0 金标；官方 `+/-` 签名仍不支持，不能认证完整 UCell 接口等价。不能替代 singscore。 |
| F02 | 参数渲染器；影响 `mvmr` 等插件 | manifest、spec → argv/env/script；MVMR 为 GWAS TSV → RDS、log | **部分修复且有兼容回归**。MVMR `false`→R FALSE 已通过；既有 MTAG/LDSC presence 消费者仍声明 bool，显式 false 会启用开关。新增 flag 类型未完成相应插件迁移。MVMR gencov 仍不在交付范围。 |
| F03 | `hypothesize.boolean_test`：联合检验 | 成分 P 的 DF → joint test row、成分信息 | **原 NA 缺陷修复通过**：intersection 保留缺失，非缺失时取 max。仍需 analysis_id、资格、失败原因的计划适配；本结论不认证 union/min 或 complement/1−p 为任意联合零假设的有效检验。 |
| F04 | `hypothesize.adjust_pvalues`：多重校正 | 全家族 DF、method、n_total → 原行及 p_adj/reject | **原跨 batch/保行缺陷修复通过**。全家族校正、null 原位回填通过。研究 DAG 必须显式指定 BH/BY、冻结完整家族，并构造缺失假设的校正占位列；不能把 null 的 p_adj 自动当成计划要求的占位 1。 |
| F05 | `hypothesize.t_test`：单样本、配对、双样本检验 | 数值列/分组 DF、paired/mu/conf_level → estimate、SE、CI、t、df、P | **部分修复，仍有结果错误**。配对 Arrow-null 对齐和 CI/SE 已修；非配对宽表被错误按整对删缺失；配对/双样本 mu 被忽略，与 R 不一致。 |
| F06 | `mrlap`：样本重叠校正 MR | 两组 GWAS DF、VFS LD-score 面板 → observed/corrected 效应、SE/P、LDSC 等摘要 DF | **部分修复**。beta/se 派生 z 和 LDSC 等位基因翻转的包装缺陷通过。固定 EUR、M 取面板行数、jackknife 分块等仍未官方端到端验收，eaf 回文规则未实现。 |
| F07 | `pseudobulk_counts`：供者×细胞类型计数汇总 | H5AD、计数层/分组/条件参数 → counts TSV、samples TSV、log | **未完整修复**。元组分组与计数检查有改善，但碰撞后的输出 ID 仍重复，破坏 DESeq2 输入身份契约。 |
| F08 | `xenium_ingest`：Xenium 矩阵与细胞信息摄取 | 官方 H5 矩阵、cells CSV → counts parquet、cells TSV、gene stats、log | **原 CSC/条码对齐缺陷修复通过**，格式逻辑符合 10x 官方 CSC 定义。不是整个 Xenium 质量控制、分割或面板资格的验收。 |
| F09 | `spatial_nhood`：空间标签邻域富集 | 坐标/标签 TSV、可选 block → 邻域统计 TSV、log | **部分修复**。稀疏图消除原 n² 结构；块内置换仍不保持空间自相关。合并多切片输入时建图未按 block 隔离。不能认作计划的空间保留零模型。 |
| F10 | `lme4_fit`：LMM/GLMM | 长 TSV、公式、family/ddf 等 → 固定效应、随机参数、模型状态 | **原 KR 参数缺陷修复通过**。真实 lmerTest KR df/P 对拍通过，奇异/不收敛状态显式记录。状态在模型文本中，仍需下游科学门控；23 checks 不认证任意模型、GLMM 或 KR 区间的全部行为。 |
| F11 | `string_ppi`：STRING 网络获取/映射 | species/version/network_type/threshold 参数 → mapped/raw edges、gene info、log | **原类型选择缺陷修复通过**：支持 physical，分数层级已修正。默认仍 functional，计划必须显式设 physical 并冻结版本/阈值/映射。离线测试不等于真实全库下载验收。 |
| F12 | `network_proximity`：网络距离及随机背景统计 | 边 TSV、set_id/role/gene TSV → proximity TSV、log | **部分修复，仍有错误**。10000 默认置换、单元素 separation NA、表契约已改善；auto 表头会删真边，separation 仍非对称官方定义；表达/可检测性匹配、失败保行、P 的 MC 精度仍欠缺。 |
| F13 | `lr_communication_score`、`permutation_test`：自定义 LR 分数/标签置换 | LR/cluster 表或统计 DF → 分数/置换结果 DF | **仅文档澄清**。明确自定义算法和可交换零假设有价值，但没有实现官方 CellPhoneDB、CellChat、NicheNet，也没有新增空间/供者限制零模型。 |
| F14 | `cmap_connectivity`：查询与参考扰动谱比较 | 查询 gene/value TSV、GCT/GCTx → 排名/方向 TSV、log | **格式与方向缺陷修复通过；官方算法不等价**。GCTx 路径/轴方向、相关指标逆转排序通过；`wcs` 只是诚实标记的探索性 rank-reversal，仍不是 WTCS/NCS/tau。 |
| F15 | DAG 指纹与调度策略 | kind/spec、输入身份、引擎版本/修订 → fingerprint、复用决策/告警 | **部分修复**。引擎 source_revision 入指纹，但插件脚本/manifest/镜像等实现身份未自动纳入；Metadata 默认与禁用内存 guard 未改变，E01 配置尚未验收。 |
| 其他 1 | `magma_set`：竞争性基因集 GLS | 真正 `.genes.raw` 或基因结果 DF、集合定义 → beta/SE/P 等 DF | **GLS 数学修复通过，节点仅条件合格**。非单位 R 的 RHS/残差度量已修；DF 路径仍伪造 R=I、MAC=100、坐标=0。独立 R 矩阵金标不等于完整官方 MAGMA 二进制对拍。 |
| 其他 2 | `magma_meta`：合并 cohort 基因统计 | 多端口 cohort DF 或 cohort_files → meta DF | **原连线输入缺陷修复通过**。有线 cohort 输入和原 E2E 测试通过；完整官方 MAGMA meta 等价仍未新增认证。 |
| 其他 3 | GenomicSEM `qchisq_sf`：munge 的 P→Z 数值转换 | p、df → χ² survival quantile；munge 继续派生 Z | **指定远尾数值缺陷修复通过**。40 个 R 分位数金标及相关测试通过，不代表整个 GenomicSEM SEM/LDSC 流程与官方包一致。 |
| 其他 4 | `table_from_df`：报告表格渲染 | DF、格式参数 → 带表格内容的 DF | **指定类型渲染缺陷修复通过**。无符号整数/日期/时间保值，不能支持的类型报错而非静默空白。属于 Arrow 展示契约，不是生物统计算法认证。 |

## 新增反例与仍需阻断的路径

### R01 Pseudobulk 输出 ID 仍碰撞

位置：`/home/zj-normal/Work/node-plugins/pseudobulk/scripts/pseudobulk_counts.py:160`。

输入两个不同分组 `("donor__state","type")` 和 `("donor","state__type")`，每组 12 细胞。修复脚本成功退出，计数确实分成两组，但原始 counts 表头和 metadata 的 sample_id 都是重复的 `donor__state__type`。

原因：`urllib.parse.quote(..., safe="")` 仍不转义 `_`；注释声称 `_`→`%5F` 并不成立，见 [Python 官方 quote 定义](https://docs.python.org/3/library/urllib.parse.html#urllib.parse.quote)。现有测试用 pandas 读回后第二列自动变成 `.1`，所以 `len(set(counts.columns)) == 2` 产生假通过。

验收要求：采用真正单射的组 ID，检查原始表头和 metadata ID 唯一且严格一致，而不是检查 pandas 重命名后的列名。

### R02 非配对 Welch 被错误按整对删缺失

位置：`/home/zj-normal/Work/wt-fix-hypo/crates/node-bundles/nodes-hypothesize/src/parametric.rs:149`。

输入 x=`[1,NA,5,10]`、y=`[0,3,NA,2]`、paired=false。修复产物输出 estimate=**4.5**、t≈0.976187、P≈0.495722；实际 R `t.test(x,y)` 独立删除每列缺失，均值差应为 **11/3=3.6666667**、t≈1.3339、P≈0.2925。

原 paired=true 的 4.5 修复正确，但被错误扩展到 unpaired。源码 doc 与 `wide_welch_complete_case_matches_r` 测试把这项非官方行为写成金标，应一起纠正。[R 官方 t.test](https://stat.ethz.ch/R-manual/R-devel/library/stats/html/t.test.html)只在 paired=true 时按整对删缺失。

### R03 配对与双样本分支忽略 mu

位置：同文件 `:134` 的 paired 后端调用以及 `:147`、`:155` 的 two-sample 调用；只有单样本分支使用 `self.spec.mu`。

上述数据 paired=true 时，mu=0 与 mu=1 在修复产物输出完全相同：t=1.2857142857、P=0.4208331517。实际 R paired=true, mu=1 应为 **t=1、P=0.5**。节点允许提交 mu 却不使用，属于静默结果错误；若只打算支持单样本 mu，也必须在其他分支拒绝非零值，而非静默忽略。R 官方 mu 定义包含双样本均值差。

### R04 布尔修复对旧 presence 插件产生反向回归

位置：PR #99 的 `crates/container-plugin/src/compile/render.rs`；既有 `/home/zj-normal/.cache/audit-plugins/mtag/manifest.toml:47` 与 `scripts/h2.sh:7`；LDSC 同类位置为 `manifest.toml:87`、`scripts/munge.sh:48`。

用实际 MTAG manifest 经修复版 `compile_container_spec` 编译 `{force:false}`，得到 MTAG_FORCE=`"false"`。执行真实脚本的安全前缀（不启动 MTAG）实际生成 **`--force`**，而非禁用开关。LDSC 的 daner/daner_n/a1_is_a2 同样使用 bool + 非空判断。

新 Flag 类型解决的是设计手段，不代表既有 manifest 已迁移。账本“27 处迁移 pin 翻转”所对应差异是 migration 测试断言更新，不能证明这些插件实现已升级。当前安装根没有 MTAG/LDSC 家族，因此这是既有插件兼容缺陷，不能描述为本机已运行的 MTAG 结果错误。

额外复验：设置 NODE_PLUGINS_ROOT 为账本所用部署目录后，MTAG migration 的 **4 个测试全部打印 `skipping: plugin directory not present`，但汇总仍为 4 passed、0 ignored**。因此账本 336/336 不能等价解释为全部迁移节点实际验收。

### R05 Network proximity 自动表头判断会丢真边

位置：`/home/zj-normal/Work/node-plugins/netprox/scripts/network_proximity.py:77`、`:97`。

真实无表头输入仅一条 `A\tB`，默认 auto 输出空边集；两条互不相连的符号边也会删第一条。因为首行符号在后面不再出现，并不说明它是表头。单边输入随后最大连通分量计算会失败。

显式 has_header=no 是临时规避，不是默认自动判断正确。需明确识别已知表头或要求输入声明；不能承诺任意基因名都可自动无歧义嗅探。

### R06 Separation 跨集合距离仍单向

位置：同脚本 `:128`。

在路径图 A–B–C–D–E–F–G–H，S={A,B}、T={B,H}，当前 separation(S,T)=**−3.0**，交换集合后=**−0.5**。该实现只使用 S→T 的 nearest distance，再减集合内距离；它不是对称的网络 separation。

作者参考代码的 jorg-closest separation 合并两个方向的 nearest distances 再减两组内部均值，见 [作者 toolbox get_separation](https://github.com/emreg00/toolbox/blob/master/network_utilities.py)。注意：Guney closest 本来可有方向，不能把 closest 的方向性本身当成错误；这里质疑的是命名为 separation 的分支。

此外，空集合被 `continue` 删除而不是保留不可估计行；只按度数分箱，没有计划所需的表达/可检测性匹配；mc_se=`sd(null distance)/sqrt(B)` 是随机距离均值的 MC SE，不是经验 P 的 MC SE/精度门控。官方随机背景要求保持集合大小与度数，见 [Guney 等原论文](https://www.nature.com/articles/ncomms10331)。箱不足时缩小随机集合也不能自动当作同一冻结零模型。

### R07 UCell 只实现官方接口的子集

位置：账本 ba985f63 的 `single-cell/workflow.py:831`；新增写盘修复后的本地 25ac232 中对应 `:838`。

H5AD 基因为 G1–G4，表达 [10,5,1,0]，签名 [G1+,G4-]。插件按字面匹配带后缀基因，报 `has no genes present in the H5AD`；官方签名语义是去除后缀，计算正向分数减去加权负向分数并截断负值，见 [UCell 官方实现](https://github.com/carmonalab/UCell/blob/master/R/HelperFunctions.R)。本次真实调用 UCell 2.16.0、显式使用 BiocParallel SerialParam，在同输入上得到分数 **1**；全缺失签名 [MISSING] 官方实际得到 **0**。

因此正向签名的 21 个测试通过，可以认证受测公式，却不能认证全部 `ScoreSignatures_UCell` 语义。当前 manifest 也没有 w_neg 参数。官方全缺失签名的处理与当前直接拒绝也不同；更严格的输入契约可以选择保留，但必须明确为限制而非完整官方一致。

### R08 Spatial block 只限制置换，不限制建图

位置：`/home/zj-normal/Work/node-plugins/spatial/scripts/spatial_nhood.py:46`、`:120`。

两切片各 3 细胞，坐标分别为 [(0,0),(10,0),(20,0)] 与 [(0.1,0),(10.1,0),(20.1,0)]，k=1。实际建出的 3 条边全为跨切片边 (0,3)、(1,4)、(2,5)。block 标签只传给 permute_labels，没有传给 build_edges。

若每次严格单切片调用，这一反例不适用；但允许合并切片并设置 block_column 并不会得到切片内图。还必须明确：块内标签交换只保持标签数量，仍不保持块内空间自相关；当前新日志已经承认这一点。内存修复和诚实文档不能代替计划零模型实现。

### R09 F15 尚未形成完整实现身份及资源门控

位置：`/home/zj-normal/Work/wt-fix-render/crates/dag-core/src/fingerprint.rs:143`、`src/dag/graph.rs:668`、`src/dag/runtime.rs:178`。

新指纹输入包括引擎修订，但调用点仍只传 kind、原始 node spec、引擎版本和输入身份；没有自动传插件 manifest/script/image 的身份。外部插件改变而引擎修订、spec、输入不变时，仍缺实现变更的失效依据。这是静态设计缺口，本次没有运行缓存污染的完整 DAG 反例。

资源默认依旧为 max_concurrency=CPU 数、input_hashing=Metadata、memory_guard=None。新增 WARN 有帮助，但没有实现/验收 E01 的单大任务、线程 2、8Gi 预算与内容哈希策略。不能把 source_revision 修复当成 F15 全部完成。

## 部署与账本同步问题

- main 仍为 `825e9e0ccc38d5ad4b9566c8cb8b5ca0792a8808`，不含 3 个修复 PR。原 worktree tip 与账本一致：genes=`62aa40e6`、hypo=`fabc0ad0`、render=`65f69645`。
- 复审期间新出现 `/home/zj-normal/Work/wt-local-deploy`、`local/engine-fixes-20261005@e94e82f1`，其历史显示合并三项修复。它不是 main；本次未完成该整合树的全量构建、运行二进制归属及服务加载验收。不能简单说“没有任何整合分支”，也不能据其存在推断服务已更新。
- 六个本地插件 tip 与账本一致，其脚本 SHA256 前 16 位全部匹配账本。符号链接指向修复源码，只能证明磁盘源码，不证明已启动的工厂更新：`crates/container-plugin/src/loader.rs:160` 在加载时把 script_file 读入内存。需要 reload/restart 与实际输出证明。
- single-cell 在本次开始时为旧 detached `1d681ea`，期间被其他会话更新到 `709c964`，最后观察为 `25ac2327dbd64d3cbe12c950ba4f7fb1973d1583`。新提交包含 nullable-string 写盘修复和本地镜像 manifest digest 固定；`/tmp/ucell-image-build-02.log`、`/tmp/ucell-smoke-script-01.log` 有构建/冒烟成功记录。这些记录是读取到的外部会话证据，不是本次独立重跑容器。
- **约 09:14 的 `~/.autonomics/plugins.toml:46` 仍 pin 旧 `1d681ea...`**。`crates/container-plugin/src/sync.rs:218` 检测 HEAD 不同后，`:238` checkout detached 并 reset 到 pin。故仍有下一次同步回退的明确路径；是否就是此前回退的实际原因没有事件日志证明。需同步更新安装来源/锁定身份，而非只恢复 checkout。
- single-cell 部署记录已比账本更新，不应继续机械沿用“镜像尚未重建”的旧状态；但运行中服务是否使用新 digest，本次仍未独立核实。

## 本次验证结果

| 验证 | 本次结果 | 不应扩大解释为 |
|---|---|---|
| single-cell ba985f63 快照 pytest | 21 passed，≤1e−14 官方金标断言通过 | 全部 UCell 参数/签名语义或当前容器验收 |
| 官方 UCell 2.16.0 R 实例 | signed 输入实际得 1，全缺失输入实际得 0，与插件拒绝行为不同 | 插件已实现这两类输入 |
| pseudobulk pytest | 14 passed；新增原始表头反例仍重复 ID | 输出身份契约已正确 |
| spatial pytest | 20 passed；新增多切片反例产生跨切片边 | 计划空间零模型/全规模资源预算通过 |
| netprox pytest | 12 passed；新增表头/separation 反例失败于正确性要求 | 官方网络 separation 和冻结随机背景一致 |
| refdata pytest | 正确 repo cwd 下 21 passed | 真实全库/版本下载验收 |
| cmap pytest | 11 passed | 已实现官方 WTCS/NCS/tau |
| lme4 R 测试 | 23 checks 全通过，真实官方同会话 df/P 对拍 | 任意 LMM/GLMM 与下游科学资格通过 |
| Rust 修复测试产物 | boolean NA 10、adjust 10、t_test 9、MRlap 包装 6、bool R parse 2、gsem qchisq 3、GLS 1、MAGMA meta 2、table 7，通过 | 从当前 main 重编全部工程或全部算法官方认证 |
| 新 Rust 反例 | unpaired 错误估计；非零 mu 被忽略；MTAG false 发出 --force，均成功复现 | 这些正确性问题已被既有通过率排除 |
| 账本 MRlap 本地面板测试 | 读取到的 verify-trackC 日志明确为 1 ignored、0 passed | 完整面板 E2E 已通过 |
| 部署根 MTAG migration | 4 次 early-return skip，测试框架仍记 4 passed | 已真实检查对应插件 |

最初把 refdata 测试与其他 repo 从引擎 cwd 一起调用，因测试使用 cwd 相对路径导致找不到 scripts/string_ppi.py；改为 refdata repo cwd 后 21/21 通过。这一首次失败属于测试调用环境，不计产品缺陷。

新增反例和临时构建脚本保留于 `/tmp/autonomics-fix-review-20261005/`，Rust 反例二进制位于当前仓库忽略的 `target/fix-review/`。本次未重跑账本的全部 753 个 Rust 测试，也未重跑 20k 空间性能基准，不将原日志数字当作本次测量。

## 验收建议

1. 将“全部修复”改为“指定子缺陷已修复；节点级及部署级尚未验收”，对 F01/F02/F05/F07/F09/F12/F15 和两个自定义算法保留限制状态。
2. 优先补回归：原始 TSV ID 唯一性、paired=false 的独立缺失处理、非零 mu、旧 manifest 显式 false、headerless 单边、separation 交换集合、多切片建图、UCell signed signature。
3. MAGMA 主分析强制真实 genes.raw/LD 来源；MRlap 完成同输入同面板官方流水线对拍后，才考虑放行主 corrected P。
4. 分开验收合并代码、冻结插件来源/镜像/脚本身份、加载中的运行服务、冻结数据上的 DAG smoke，以及研究科学资格；保留不可估计/失败的全部假设行。
5. plan_*、MVMR 协方差、singscore、回文频率规则和全计划统计/资源门控仍需独立交付。该账本不足以支持“完整研究计划已能执行”的结论。
