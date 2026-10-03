# DAG 推广化调查设计(草案 v0)

- 日期:2026-10-02 · 分支:`feat/dag_generalize`(与 main 同点,阶段起点)
- 状态:**草案,待拍板 §7 各张力项后定稿**
- 目标形态(已确认):DAG 成为唯一执行底座;领域计算类工具全部节点化;agent 工具通道只保留查询/检索与控制面
- 盘点范围(已确认):bio_crates 计算类 + stat_crates 统计类 + 数据库/检索类,全量纳入

---

## 1. 术语与背景

| 术语 | 定义 | 证据 |
|---|---|---|
| agent 工具 | `ToolFunction` + `#[tool]` 宏,请求-响应,返回 `ToolResult`(文本/JSON) | `crates/agentik-core/src/tools/function.rs:252` |
| DAG 节点 | `DagNode`/`NodeFactory`/`NodePlugin`,端口化数据流(DataFrame/File/FileSet) | `crates/dag-core/src/node/node_trait.rs:16`、`registry/registry.rs:30` |
| manifest 插件节点 | `manifest.toml` 声明、零 Rust,执行内核是 `container_command` | `crates/container-plugin/src/factory.rs:28` |
| 控制面工具 | data-engine 元工具 21 个 + host 控制 14 个,操作 DAG 本身/宿主,**不属于迁移对象** | `crates/data-engine-tools/src/lib.rs:80-113` |

三次前置迁移已铺平道路:crate 化(2026-08-07)→ 官方容器化(2026-08-26)→ manifest 插件化(2026-09-28)。本阶段是第四步:**收口工具面**。

## 2. 现状基线(v0 数字,待 M1 精确化)

### 2.1 三张面子

| 面 | 规模 | 备注 |
|---|---|---|
| 进程内节点 | 20 个 bundle,**287 kinds**(io 57、ml 54、survey 29、hypothesize 26、grf 23…) | `crates/data-engine/src/default_registry.rs:38-128` |
| 容器插件节点 | 28 家族 / **97 kinds**,27 个 ghcr digest 镜像 + 1 占位 | `/mnt/projects/node-plugins`,启动时 loader 注册 |
| agent 工具(全开 profile) | **~138 个装配**;定义 ~226 | `crates/runtime/src/tools.rs:216-280`、`host.rs:462-561` |

工具面构成:领域查询类 ~103(opentargets 8、gwascatalog 10、chembl 6、kegg 6、protocolio 6、string 5、rcsb 4、opengwas 3…)、控制面 35、本地库(KMS 10 / bib 13 / writing 19 / catalog 6 / vbash 1)。
定义未装配:ensembl(8)、reactome(9)、embase(2)、eutils/arxiv/biorxiv/europepmc(经 `LiteratureGateway` 以 `lit_search`/`lit_fetch` 间接暴露,`crates/bib-base/src/query.rs:838-847`)。

### 2.2 查询类双轨现状

`nodes-io` 已注册 **~44 个 `source_*` 节点**(opentargets/chembl/openalex/kegg/semantic-scholar/crossref/nhanes/alphafold/interpro/pubchem/protocolio/clinicaltrials/rcsb/uniprot/reactome,`nodes-io/src/lib.rs:110-163`)。即:**查询类的"节点形态"大体已存在,缺的是逐工具的覆盖度核对与工具退役决策**,不是从零建。
(卫生问题:opentargets 两个工厂被注册了两次,`nodes-io/src/lib.rs` 双段注册,M1 时顺手清掉。)

### 2.3 底座就绪度

- **fingerprint 增量**:blake3(engine_version‖kind‖spec‖Σ 输入身份),DataFrame 身份=上游指纹链式传播(`dag-core/src/fingerprint.rs:137-177`);`incremental` 默认关闭,`run_dag` 当前全量执行。
- **append-only runs + per-node 证据**:`dag-history.db` 三张表;`NodeRunDetails`(镜像/digest/exit_code/日志 VFS URI)+ `InputBinding`(入边指纹)自动捕获(`dag-core/src/dag/history.rs:33-48`、`runtime.rs:248-286`)。
- **使用痕迹可挖**:`~/.autonomics/agent.db`(会话含 tool_use/tool_result)+ `dag-history.db`(trigger=`agent:<session>`)→ M2 有数据源。

### 2.4 已知缺口与遗留(并入盘点)

- **stat 侧节点缺口**:`competing_risk`、`multistate`、`gbtm`、`lca`、`ensemble`、`shap`、`sem` 有算法无节点封装(`docs/sta_epi_nodes_zh.md:319`)。
- **容器侧遗留**(`docs/container-node-migration-backlog.md`,注意该文档未回写插件化代际差):MAGMA gene/set/meta 未包装;sLDSC、GenomicSEM(Wave 1);MRlap、LCV、CPASSOC(Wave 2 阻塞);coloc、HyPrColoc 未钉 digest;TwoSampleMR 原生 Rust fallback 未删;LAVA 生产级 UKB scan 基线;GCTA 队列级功能各自独立迁移。
- **值模型**:仅 DataFrame/File/FileSet(`dag-core/src/value.rs:10-14`)——标量、N×N 矩阵输出无家(OpenGWAS 先例因此把 `gwasinfo_count`/`ld_matrix` 留在工具面)。
- **live-data 语义**:source 节点零输入 → 指纹=kind+spec 恒定;一旦启用 incremental,API 数据更新不会失效缓存。**需在调查中定案**(见 Q3/T4)。

## 3. 调查问题清单

| # | 问题 | 为什么重要 | 验证途径 |
|---|---|---|---|
| Q1 | 每个已装配工具的节点对应物覆盖度:全等价/部分(参数或输出缺失)/无 | 迁移工作量的分母 | M1 矩阵 |
| Q2 | OpenGWAS 判定规则(表格→节点,标量/矩阵/文件→工具)推广后是否完备?例外类别:多轮交互、富文本浏览、有状态会话 | 决定"唯一底座"的边界画在哪 | M3 spike |
| Q3 | 底座缺口清单:标量/矩阵端口、live-data 新鲜度、进度流式(工具的 `ProgressRecord` vs 节点 `NodeReporter`)、超时/重试语义差 | 哪些迁移被底座挡住 | 代码核对 + spike |
| Q4 | 发现性:300+ kinds 下 agent 的 list→get→add 往返 token 成本 vs 工具 schema 常驻 prompt;同名能力双轨时 agent 如何选择 | 体验回归是最大隐性风险 | M2 会话挖掘 + 实测 |
| Q5 | 实际使用分布:哪些工具高频/零调用;哪些 source 节点从未被图引用;双轨重复调用比例 | 退役排序的证据基础 | M2 |
| Q6 | 等价性验证:每迁移项的金标准来源(R 参考/官方容器基线/新旧并行 diff) | 项目既有纪律(xval、Podman e2e 基线)不可降级 | M6 逐项设计 |
| Q7 | 运维与安全:API 密钥如何到节点(进程内 reqwest vs 容器隔离网络——后者禁网!)、速率限制、故障语义 | 查询类节点化的一半难度在运维不在代码 | 配置面调查 |
| Q8 | 退役路径:兼容期长度、prompt/skill 引导改写、文档回写、`enable_*` 旗标下线节奏 | "逐步替换"的执行语法 | D4 计划 |
| Q9 | 容器遗留 backlog 是否归并本阶段批次,还是保持独立轨道 | 范围控制 | 拍板 |

## 4. 盘点维度(矩阵字段)

每个工具一行,字段:

1. **身份**:crate、工具名、装配点(tools.rs/host.rs/未装配)、`enable_*` 旗标
2. **形态**:输入 schema 概要、输出类别(表格/标量/矩阵/文件/富文本/多轮)、状态性、典型耗时、外部依赖(API/DB/容器/本地库)
3. **节点对应物**:kind、参数覆盖(全/缺哪些)、输出覆盖、语义差异(分页?排序?限流?)
4. **使用证据**(M2):90 天调用次数、错误率、最近调用、被哪些 profile 用
5. **初步判定**:`节点化` / `保留工具` / `双轨过渡` / `退役` / `上游阻塞`(Wave 2 类)/ `待定`
6. **验证方案**:金标准来源 + 等价性测试设计
7. **批次建议** + 依赖

## 5. 调查方法

- **M1 静态映射矩阵**(脚本化):扫 `#[tool]` 定义 × `tool_set_from_config`/`tools_from_profile` 装配 × `NodeRegistry` kinds,输出 TSV(沿用 `containers/image-inventory.tsv` 惯例)。参数/输出覆盖度用 schema 对比(schemars 两侧同源,可自动 diff)。
- **M2 使用挖掘**:`agent.db` 会话消息抽 tool_use 频次/错误/参数分布;`dag-history.db` runs 与 trigger 关联,反查节点使用。产出 top-N 工具表 + 零调用清单 + 双轨重复率。
- **M3 判定规则 spike**(3 个代表案例,规则打样):
  - ①已双轨仍高频的查询工具(从 M2 选,预计 chembl/gwascatalog 之一)——退役工具、补节点缺口的全流程演练;
  - ②标量/矩阵输出工具(`gwasinfo_count`/`ld_matrix` 型)——验证"不扩 PortType,标量=单行 DataFrame、矩阵=parquet File"的逃逸通道是否成立;
  - ③多轮/有状态或富文本浏览工具(`lit_search` 查询改写、KMS 浏览型)——验证"保留工具"边界的判定细则。
- **M4 双轨成本对照**:grf 进程内 23 kinds vs 容器插件 23 kinds 的参数/输出/数值差异 diff,量化双轨维护税,反哺判定。
- **M5 文档代际差回写**:backlog 各条目按插件化现状重核(部分 gate 已被 digest-pinned manifest 消化),与 Q9 一并处理。
- **M6 验证设计**(与 M3 同步起步):每候选迁移项登记金标准来源;无金标准的不开工。

## 6. 交付物

| # | 交付物 | 形态 |
|---|---|---|
| D1 | 工具×节点全量映射矩阵(含判定列) | TSV + 报告 |
| D2 | 底座缺口清单(dag-core/data-engine 需动点,分必须/可选/明确不做) | 文档 |
| D3 | 判定规则 v2(OpenGWAS 规则推广 + 例外细则) | 文档 |
| D4 | 分批迁移计划(批次/依赖/验收标准/回滚) | 文档 |
| D5 | 调查总报告 | `docs/design/`(本文件演化或另立) |

## 7. 需要拍板的张力(附建议)

- **T1 查询/检索的边界**:同是"查 ChEMBL",浏览式探索(人读)与数据获取(进管道)流向不同。建议判据:**输出会进分析管道 → 节点;纯人读浏览/标量问数 → 工具**。同一能力的两半可并存但各有归属。
- **T2 标量/矩阵**:建议**不扩 `PortType`**,标量=单行 DataFrame、矩阵=parquet File;除非 M3-② 证明人机工效灾难再议。
- **T3 双轨期引导**:工具与节点并存期间,agent 需明确指引(优先节点、工具标注 deprecated)。退役前置条件建议:连续 N 周零调用 + 节点等价验收通过。`deprecated` 字段两侧都有,用它承载。
- **T4 live-data 新鲜度**:incremental 当前默认关,不阻塞本阶段;但 D2 必须登记"source 节点 TTL/强制失效钩子"为启用增量的前置项。
- **T5 本地库工具归属**(KMS/bib/writing):倾向视为控制面/知识面,**不纳入**本阶段迁移;仅确认共识。
- **T6(范围)容器遗留 backlog**:建议归并为独立批次挂在本计划下(共享验收框架),不混入查询类主线。

## 8. 风险登记册

| 风险 | 缓解 |
|---|---|
| 发现性回归:300+ kinds 两跳发现比工具直呼贵 | Q4 实测;必要时做节点目录的分层/领域前缀索引 |
| 双轨语义漂移:同能力两套参数分叉 | M1 的 schema 自动 diff 进 CI;deprecated 标注 |
| 隐性组合习惯:会话里工具间的搭配链条被打断 | M2 挖工具共现序列,迁移批次尊重链条 |
| 数据陈旧:incremental + source 恒定指纹 | T4:登记为启用前置,不默认开 |
| 范围蔓延:~103 工具全量精查 | M1 自动化 + 分层:高频精查、低频粗查、零调用直接退役候选 |
| 密钥进容器:查询节点若走容器路径则禁网冲突 | Q7 先行;查询类默认进程内路径(reqwest),不强制容器化 |

## 9. 执行顺序(建议)

1. **M1 矩阵脚本 + 跑全量** → D1 初版(判定列先空)
2. **M2 挖掘脚本 + 90 天窗口** → 填使用证据列
3. 拍板 §7 → D3 判定规则 v2 定稿
4. **M3 三个 spike**(并行)→ 检验规则、产出底座缺口实证
5. **D2 缺口清单 + D4 分批计划** 定稿
6. M4/M5 择机穿插;Q9/T6 决定 backlog 归并

---

## 附录:关键证据索引

- 节点注册装配:`crates/data-engine/src/default_registry.rs:38-128`;bundle feature:`crates/data-engine/Cargo.toml:51-72`
- source 节点注册表:`crates/node-bundles/nodes-io/src/lib.rs:110-163`(含 opentargets 重复注册)
- 工具装配:`crates/runtime/src/tools.rs:216-280`(单 agent)、`crates/runtime/src/host.rs:462-561`(多 agent profile)
- 值模型:`crates/dag-core/src/value.rs:10-14`(NodeValue)、`:204`(PortType)
- fingerprint:`crates/dag-core/src/fingerprint.rs:137-177`;增量门:`dag/graph.rs:638-683`;默认关:`dag/runtime.rs:178`
- run 证据:`crates/dag-core/src/dag/history.rs:33-48`;`NodeRunDetails`/`InputBinding`:`dag/runtime.rs:248-286`
- container_command 对 agent 禁用:`crates/data-engine/src/data_engine.rs:68-79`
- OpenGWAS 先例规则:commit `6fcf90d`(2026-08-07)
- stat 节点缺口:`docs/sta_epi_nodes_zh.md:319`
- 容器遗留:`docs/container-node-migration-backlog.md:13-88`
