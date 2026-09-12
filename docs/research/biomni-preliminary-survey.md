# Autonomics 与 BioMNI 初步调查

日期：2026-09-12

## 结论摘要

Autonomics 不应按“另一个通用生物医学 Agent”来叙述。BioMNI 的主线是扩大生物医学动作空间并证明跨域泛化；Autonomics 更清晰的可发表贡献是把 LLM Agent 放在一个类型化、可验证、可审计、可重放的科研执行面上。二者的关系可以概括为：

> BioMNI 扩大 Agent 能做的生物医学动作；Autonomics 约束并记录 Agent 实际执行的研究动作。

这个定位有工程事实支撑：本仓库当前根工作空间有 94 个 crate、287 个注册 node factory、22 个 OCI 工具目录、约 47 万行 Rust 代码和约 3,945 个 Rust 测试标注。它的差异化不在“功能更多”，而在 typed DAG、JSON Schema、端口类型校验、DAG snapshot/ref、immutable data catalog、VFS、OCI 隔离执行、artifact 指纹和文献-写作闭环。

当前距正式论文最大的缺口不是代码量，而是缺少一个能把优势转化为证据的公开评测：需要证明 constrained harness 在任务成功率、数值正确性、可重放率、审计完整度、故障恢复或安全边界上优于自由代码 Agent 或普通工具调用 Agent。

## BioMNI 核事实

BioMNI 已从 2025 年 bioRxiv 预印本发展为 2026 年 Science 论文：

- 论文：Huang 等， “Autonomous biomedical research with an artificial intelligence agent”， Science， 2026-07-09， DOI: `10.1126/science.adz4351`。
- 预印本： “Biomni: A General-Purpose Biomedical AI Agent”， DOI: `10.1101/2025.05.30.656746`， PMC `PMC12157518`。
- 代码： <https://github.com/snap-stanford/Biomni>， Apache-2.0；截至 2026-09-12 约 3,871 stars。
- 网站：<https://biomni.stanford.edu>。

### 系统构成

BioMNI 由两部分构成：

1. **BioMNI-E1 环境**：论文报告从 25 个 bioRxiv 领域、每个领域 100 篇 2024 年论文中抽取任务、软件和数据库，经人工筛选后形成 150 个专用生物医学工具、105 个软件包、59 个数据库。数据库分为 web API 动态查询和本地 data lake 两类。
2. **BioMNI-A1 架构**：基于 CodeAct 思路。任务先由检索器选择相关工具、数据和软件；随后 LLM 形成编号计划，生成 Python/R/Bash 代码执行；观察结果再反馈到后续推理，直到给出答案。

### 评测与案例

论文使用的证据结构值得借鉴：

- LAB-Bench：315 个 held-out 问题，DbQA 准确率 74.4%，接近人类专家 74.7%；SeqQA 81.9%，高于报告的人类水平 78.8%。
- HLE 生物医学子集：52 题，BioMNI 17.3%，高于 base LLM 6.0%、coding agent 12.8%、literature agent 12.2%。
- 8 个真实任务基准：变异优先级、GWAS 因果基因、CRISPR screen、罕见病诊断、药物重定位、scRNA-seq 注释、 microbiome disease-taxa 分析、患者基因优先级。相对 base LLM、ReAct+Code 和 BioMNI-ReAct 平均提升分别报告为 402.3%、43.0%、20.4%。
- 3 个案例研究：458 个可穿戴设备文件、约 33.6 万细胞核的 snRNA/snATAC 多组学分析、湿实验克隆协议；其中克隆任务有实验验证。

BioMNI 的论文贡献顺序是：定义问题、构建动作空间、提出 Agent 架构、系统评测、真实案例。这是 Autonomics 论文应模仿的论证结构，而不是简单罗列模块。

## Autonomics 当前状态

以下数字来自当前工作树静态统计，可作为论文系统描述的初稿，正式投稿前应脚本化生成：

| 维度 | 当前状态 |
| --- | ---: |
| 根 Cargo workspace crate | 94 |
| 默认注册 node factory | 287 |
| OCI 工具目录 / Dockerfile | 22 |
| Rust 代码规模 | 约 471,212 行 |
| Rust 测试标注 | 约 3,945 个 |
| `impl ToolFunction` 定义 | 224 处，实际暴露随配置变化 |
| 支持的生物格式 | VCF/BCF、FASTA/FASTQ、BED、GTF/GFF、SAM/BAM/CRAM、BigWig/BigBed 等 |

### 已有技术贡献

1. **类型化动作空间**：Agent 通过 `list_node_factories`、`get_node_spec`、`get_node_ports`、`get_node_doc` 发现能力；node spec 由 JSON Schema 校验，DAG 边由 DataFrame/File/FileSet 等端口类型和列 schema 双层校验。
2. **可审计执行协议**：每次运行保存完整 node/edge/spec/run report snapshot；支持 ref、history log、snapshot diff、checkout 和 branch。这直接对应科研工作流的可追溯性。
3. **受控外部分析执行**：外部生物信息工具通过 Podman 一次性容器运行，默认隔离网络、只读 rootfs、禁用 privilege escalation，输入输出显式声明，输出上传 VFS 并计算 SHA-256 指纹。
4. **不可变数据和参考面板**：data catalog 使用 manifest、版本目录和 canonical digest；panel cache 校验每个文件的大小和 SHA-256 后才原子暴露；DAG 可引用 `/datasets/<id>@sha256-<digest>`。
5. **统一数据面**：VFS 将 local、S3、OSS 和 catalog 挂载到统一命名空间，避免 Agent 直接处理部署路径和凭据。
6. **科研闭环**：生物医学 API、文献库、全文、引文、LaTeX/写作工具、图表/表格节点和 KMS 位于同一运行时，支持从数据到文献支撑稿件的材料流。
7. **可重放出口**：部分 DAG 可反向编译为 R/Python，能作为审稿人可检查的独立执行产物。

## 与 BioMNI 的对照

| 维度 | BioMNI | Autonomics |
| --- | --- | --- |
| 主张 | 通用生物医学 Agent 与跨域泛化 | 生物医学科研 harness：类型化、可审计、可复现执行面 |
| 动作接口 | 检索工具/数据/软件后生成 Python/R/Bash 代码 | ToolFunction 调用 typed DAG node，spec 和端口先验证 |
| 泛化机制 | CodeAct + 资源检索 + 自适应计划 | 注册 node bundle、DataFusion/Arrow、SQL bridge、OCI wrapper |
| 外部工具 | 大型预装环境，代码可调用多种软件 | 每个外部分析声明 OCI image、输入输出、timeout、资源和网络策略 |
| 数据管理 | web API + 本地 data lake | VFS + immutable catalog + digest + panel cache 校验 |
| 审计 | 代码/日志/输出文件夹，论文强调轨迹可检查 | DAG snapshot/ref、run report、artifact 指纹、数据版本和 manifest |
| 证据强项 | 8 个跨域基准、3 个真实案例、湿实验验证 | 单元/数值交叉验证、系统架构和 provenance 机制，尚无统一 Agent 基准 |
| 覆盖广度 | 25 个生物医学子域、湿实验协议和多组学 | 更集中在流行病学、统计遗传学、临床/调查分析、ML/DL、文献写作和可复现数据工程 |

最可辩护的论文命题不是“Autonomics 全面优于 BioMNI”，而是：

> 在需要长期保留、同行检查和复现的生物医学分析中，typed and auditable action surface 能减少无效组合和不可追溯执行，并提高工作流完整性、数值可靠性和重放成功率。

## 相关工作与压力

1. **BixBench**: FutureHouse/ScienceMachine, arXiv:2503.00096。205 个问题来自 60 个真实发表 Jupyter notebook，要求 Agent 探索数据、执行 Python/R/Bash 并解释结果；代码提供 Docker 环境和多 replica 评估。它直接说明“真实 bioinformatics workflow benchmark”已是活跃方向。
2. **BioML-bench**: DOI `10.1101/2025.09.01.673319`。覆盖蛋白质工程、单细胞组学、成像和药物发现四类端到端 ML 任务，比较 STELLA、BioMNI、AIDE、MLAgentBench 与人类基线，发现现有 Agent 仍普遍低于人类且架构 scaffolding 很关键。
3. **PromptBio**: DOI `10.1101/2025.07.05.663295`， 2026 年更新为 “PromptBio: An Agentic Platform for End-to-End Computational Biomedical Research”。多 Agent 平台方向与 Autonomics 相邻。
4. **FlowBench**: DOI `10.64898/2026.06.12.731844`。将 planning、fault recovery、biological interpretation 和 end-to-end output fidelity 分开评估，明确指出有效 plan 不等于数据恢复和解释可靠。这与 Autonomics 的 typed validation/provenance 叙述高度相关。
5. **BioMNI 生态更新**：仓库和模型持续发展，并已有 Biomni-E2 社区动作空间建设。因此，Autonomics 若只强调工具数量，很快会被覆盖；必须强调执行治理和科研可复现性。

## 发表缺口

### 1. 缺少统一 benchmark

现有单元测试、golden tests 和容器集成测试能支持软件正确性，但不能回答 Agent 研究问题。至少需要一个 20-50 任务的公开任务集，每个任务包含原始数据、目标、期望输出、评分器和允许工具。

建议先选 Autonomics 已有优势领域：

- GWAS summary statistics -> LDSC / MR / coloc / LAVA / MAGMA。
- 流行病学或 survey 数据 -> 加权、回归、中介、敏感性分析。
- 临床或影像数据 -> 表格处理、PyRadiomics、模型评估。
- 文献问题 -> PubMed/OpenAlex 检索、筛选、引用完整性和稿件段落生成。
- 故障注入 -> schema 错误、坏输入、面板缺失、容器超时、artifact 被篡改。

### 2. 缺少对照系统

最小 baseline 应包含：

1. base LLM，无工具。
2. 自由 CodeAct：同一模型 + Python/R/Bash + 依赖清单。
3. 普通 ToolFunction：只暴露 API，不暴露 typed DAG。
4. Autonomics no-history 或 no-container-policy 消融。
5. 完整 Autonomics。
6. 外部系统：优先接 BioMNI 或一个 BixBench agent 适配器；若成本或许可受限，可先复用公开结果并做任务子集复测。

### 3. 指标还没有论文化

建议采用分层指标：

- **任务结果**：答案准确率、数值误差、Top-k、AUROC/Spearman、开放题人工盲评。
- **动作质量**：schema rejection 率、无效边率、端口类型错误率、重试次数、故障恢复率。
- **科研复现**：同一 snapshot 重跑成功率、checksum 稳定性、DAG 编译后独立运行成功率、环境重建时间。
- **审计**：节点/数据/镜像/参考面板版本覆盖率、run report 完整率、引文可解析率。
- **成本**：token、工具调用、CPU/GPU 时间、墙钟时间。
- **安全**：越权路径访问、网络访问、敏感凭据暴露、容器资源超限行为。

### 4. 安全与威胁模型需正式化

容器执行面已经优于自由代码执行，但论文需要明确范围：VFS 操作不等于操作系统 shell；外部工具建议 digest pin；API 凭据只在 runtime 层；多 Agent 会话有独立 DataEngine。还应报告已知边界，例如 catalog 多写并发尚无 optimistic concurrency、部分分析仍是过渡 native 实现、远程 artifact 内容 hash 和外部输入 invalidation 还需要完善。

### 5. 需要真实科研案例

BioMNI 的说服力来自“研究者真实任务”和湿实验验证。Autonomics 至少需要 3 个端到端案例：

1. 多工具统计遗传学研究：公开 GWAS 数据到 LDSC/MR/coloc/MAGMA，并生成可重放 DAG 和 manuscript assets。
2. 临床/流行病学可重复分析：survey 或公开临床数据，含加权、缺失值、敏感性分析和图表。
3. 文献到稿件：系统检索、全文管理、引用图、自动生成图表和 LaTeX，人工检查引用真实性。

若条件允许，加入专家用户研究：让外部研究者完成同一任务，比较时间、错误、可复现性和审计满意度。

## 建议论文框架

题目方向：

1. **Autonomics: a typed, auditable execution harness for biomedical AI agents**
2. **Constrained action spaces improve reproducibility in biomedical AI agents**
3. **From LLM agents to reproducible biomedical research workflows**

论文结构：

1. Introduction: 生物医学 Agent 的能力扩张快于执行治理；自由代码轨迹难审计、难复现。
2. Related work: BioMNI、BixBench、BioML-bench、PromptBio、FlowBench、workflow/pipeline systems。
3. Harness contract: model cockpit、capability registry、evidence plane、execution plane、research protocol。
4. Autonomics architecture: typed DAG、node registry、VFS/catalog、OCI runtime、history、writing/KMS。
5. Benchmark: Autonomics-AuditBench 或 ReproBioBench，包含任务、baseline、指标和消融。
6. Results: 任务性能、重放率、audit completeness、故障恢复、成本和专家研究。
7. Case studies: 三个真实科研工作流。
8. Discussion: 安全、限制、适用范围、社区生态。

## 30 天优先事项

1. 固化一个评测名称和最小任务集，先做 20 个任务、5 类领域。
2. 写 benchmark runner：统一任务输入、模型配置、工具开关、trajectory capture、DAG snapshot 和评分。
3. 增加 BioMNI/BixBench 式基线；至少完成 base LLM、CodeAct、ToolFunction、Autonomics。
4. 定义 audit completeness schema：每个结果能否追溯到 prompt、tool call、DAG、image digest、data digest、artifact hash。
5. 选定一个公开数据案例做“一键重放”演示，作为论文和仓库核心图。
6. 脚本化统计 94 crates、287 nodes、22 containers、测试数量和 schema 覆盖率，避免论文数字手工漂移。
7. 建立论文级威胁模型和安全评估表。

## 总体判断

短期最现实的目标是生物医学信息学、AI for Science 系统或健康 AI 会议的系统/基准论文；若后续能提供强真实科研发现、专家用户研究和开源一键复现，才有条件冲击更高通量综合期刊。当前最重要的是把“工程能力”转化为可测命题：typed and auditable harness 是否真的让 Agent 产生的科研流程更正确、更安全、更容易复现。
