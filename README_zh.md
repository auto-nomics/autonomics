# autonomics / agentik

[English](README.md) | [中文](README_zh.md)

Autonomics 是一个以 LLM 智能体为核心、可自我扩展的完整科研系统。两种智能体角色分工协作：**Researcher** 负责科学问题本身——文献、证据、类型化 DataFusion DAG、执行与解释、稿件写作；**Developer** 负责能力建设——当某个分析节点尚不存在时，Researcher 记录能力缺口，派生一个 Developer 子智能体，由它起草新的容器插件（manifest、脚本，必要时还有新的 OCI 镜像），通过确定性校验门禁，并把不可变快照安装进实时节点注册表。操作经验也以同样的方式沉淀：真实运行中的观察会被蒸馏成可审查的 skill。流行病学、统计遗传学、临床与调查分析、机器学习、科研数据库、可复现执行，以及受监督的自我改进，被当作同一个科研系统的组成部分，而不是彼此独立的应用。

根 Cargo 工作空间在本版 README 更新时包含 101 个 crate。这是一个活跃研究代码库，API、节点契约和配置路径仍可能调整。`dendrite/` 是独立的嵌套 Rust 工作空间，用于知识管理系统。

## 系统契约

系统横跨五个面与一个协议：

- **模型驾驶舱**：常驻 gateway 守护进程、流式对话、持久记忆，以及由 `researcher` / `developer` 两种智能体类别构成的受约束多智能体拓扑。
- **能力注册表**：经 JSON Schema 校验的工具与类型化 DAG 节点，以 `plugin/node` 寻址（内置节点位于 `core/` 命名空间），覆盖分析、I/O、source、sink 与写作操作；manifest 插件可以在系统运行期间安装。
- **自我改进基座**：插件 RSI（递归自我改进）、skill 进化与运行环境（镜像）开发。智能体起草、确定性门禁校验、人类审批——三者齐备，变更才会到达运行中的系统。
- **证据面**：生物医学科研 API、统一文献网关、以类型化数据形式在 DAG 边上流动的引文、全文存储、文献库记录与知识树访问共享同一个运行时。
- **执行面**：DataFusion、Arrow、可插拔的任务执行器契约（当前为 local 与 remote，未来可对接 SLURM/Kubernetes）、VFS、不可变数据包、Podman 和参考面板缓存。
- **科研协议**：DAG 历史、快照、diff、分支、溯源导出（RO-Crate / W3C PROV-JSON）与数值交叉验证，让智能体的工作可检查、可重放、可发表。

Autonomics 不是通用聊天应用，不是围绕自由脚本的 notebook 替代品，也不是单体 pipeline；它同样不是一个自主变更器：智能体永远拿不到原始 git、镜像仓库、主机文件系统或实时注册表的访问权。一切改进都是通过门禁与审查的提案——模型可以起草和测试，人类与可信基础设施负责晋升。

## 演示

![PubMed 文献检索](docs/pubmed.gif)

## 系统架构

![系统架构：瘦前端、gateway 守护进程、researcher 与 developer 智能体、能力注册表、自我改进基座、执行面、共享数据面、科研协议](docs/diagrams/architecture.png)

瘦前端与唯一的常驻守护进程通信。`autonomics serve` 以单写者身份持有 `RuntimeHost`：启动时恢复持久化的智能体布局，在 `127.0.0.1:8765` 上提供 REST + SSE（Bearer token 保护，支持 `Last-Event-ID` 重放），并在前端退出后让智能体继续运行。TUI 首次使用时会自动拉起守护进程；`autonomics run` 以无头方式驱动同一个守护进程。

```text
Ratatui TUI（瘦客户端）   ──REST/SSE──▶  gateway 守护进程（`autonomics serve`）
无头 CLI（`autonomics run`）                127.0.0.1:8765 · Bearer token
HTTP API + 文献 Web 前端         EventHub 重放 · 前端退出后智能体仍在运行
        |                                              |
        └──────────── events ────────────── RuntimeHost（单写者）
  根位于 /root 的智能体树 … 守护进程启动时恢复
        |
        ├── Researcher 智能体（默认类别）
        |     科学问题、DAG 设计与执行、结果解释、
        |     文献、写作、记忆
        |     spawn_agent + delegate_to ─────────────┐
        |                                           | 仅接口契约与合成
        └── Developer 智能体（子代理）  ◀───────────┘ 样例——绝不接触
              插件/节点实现、运行环境选择、             科研数据集
              聚焦容器验证、安装卸载
        |
        | ToolFunction 调用（委派是唯一的智能体间通道：
        | 仅父 → 直接子）
        v
Data-engine actor（每会话一个）
  节点发现、新增/更新节点（`plugin/node`）、边、
  即发即忘运行、输出保留、历史
        |
        v
节点注册表 + 调度器
  `core/` 下的内置 bundle + fail-closed 的 manifest 插件
  类型化输入输出端口、JSON Schema 校验的 spec
  可插拔执行器：local / process / remote
        |
        +--> DataFusion DataFrame 与 Arrow RecordBatch 节点
        |
        +--> 证据通道：File(evidence) 边在文献与
        |    DAG 之间传递规范化引文
        |
        +--> OCI 容器节点
              基于 digest 钉死的镜像的临时 Podman 运行
              内容寻址工作目录（可安全重跑）
              只读挂载、不可变面板包
              artifact 发布到 VFS

自我改进基座（智能体起草 · 门禁校验 · 人类晋升）:
  skills      evo_observe → 确定性蒸馏 → 提案 → 人工审批
  plugin RSI  请求 → 开发工作区 → 校验门禁 → 安装/发布
  environments  /environments/dev 镜像编写 → 构建 → 冒烟 → 目录

共享数据面：
  VFS（OpenDAL）-> 本地与可选的 S3/OSS 挂载
  数据目录      -> 不可变、带版本的 Hugging Face 数据包
  biofusion     -> VCF/BCF/FASTA/FASTQ/BED/GTF/GFF/SAM/BAM/CRAM/BigWig/BigBed 读取器
```

## Researcher 与 Developer 智能体

智能体构成一棵根为 `/root` 的树，每个智能体恰有一个类别：

- **Researcher**（默认）——生物医学科研助手。它拥有科学问题、分析设计、DAG 构建、执行与解释，并持有文献库、写作与科研 API 能力。系统提示词明确告诉它：插件与节点生态是*动态的*，缺失或不完善的节点是正常发现，不是死路。
- **Developer**——DAG 节点与插件开发者。它通过宿主拥有的生命周期构建、校验、安装与卸载插件，选择运行环境，并做聚焦的容器验证。它会明确拒绝科研或数据分析请求：交接内容只包含接口契约、schema、失败场景与小型合成样例——绝不包含生产或科研数据集。

编排被刻意约束。智能体间通信只有委派一种，且严格限定父 → 直接子；一个智能体只能看到自己、父代理和子代理。委派是一等记录，带状态（pending/running/completed/interrupted/failed）并跨守护进程重启持久化。九个宿主工具为 `spawn_agent`、`delegate_to`、`route_task`、`get_agent_info`、`list_agents`、`list_delegations`、`get_agent_history`、`shutdown_agent`、`interrupt_agent`。

Developer 交付时，交接物包含插件名、完整 `plugin/node` 地址、节点文档、spec schema、端口布局、安装状态、最小 DAG 用法示例与验证证据。`plugin_install` 通过 `reload_plugin` 把不可变快照激活进实时注册表，Researcher 在同一会话内即可使用新节点。

## 自我改进

Autonomics 沿三条受监督的回路自我改进，共享同一个安全模型：智能体提案、确定性基础设施校验、人类审批。

### Skills

一个 skill 是一个含 `SKILL.md`（YAML frontmatter + 操作性正文）的目录，与 Anthropic `skills` 生态遵循同一契约，第三方技能包可以直接安装。skill 分三层——`builtin`（编译进二进制）、`global`（`~/.autonomics/skills`）与 `workspace`——每个智能体的系统提示词都会携带技能库的单行索引，正文按需经 `skill_search` / `skill_get` 获取。skill 还可捆绑带参数的 DAG 工作流模板与评测用例。

进化回路中没有任何辅助 LLM：

1. **观察** —— `evo_observe` 把持久的失败、配方与注意事项记录为内容寻址的观察（重复记录自动去重）。评测失败会被自动捕获。
2. **路由** —— 每条观察按 DAG 节点地址锚定，路由到 skill 域、插件域、两者或待分诊。
3. **蒸馏** —— 纯代码按节点类型与规范化错误签名对观察聚类；同一模式出现三次即合成一个保守的 skill 提案，提案只引用已记录的修复，绝不发明建议。
4. **审查** —— 提案暂存于 `~/.autonomics/skill-proposals/`，在 TUI 技能进化面板或 gateway API 中批准或驳回。自动审批默认关闭，智能体署名的提案永远不自动批准。
5. **度量** —— 工具使用遥测作为适应度信号，跨重启持久化。

### Plugin RSI

`plugin-rsi` 是基于插件的递归自我改进基座。结构化请求（来自用户、智能体、工作流运行、评测失败或观察）进入内容寻址的请求记录；Developer 智能体在挂载于 `/plugins/dev/<name>` 的 git 管理开发工作区中工作；校验门禁检查 manifest、运行环境策略（已审批的 digest 钉死镜像、允许的解释器、插件不得自带 Dockerfile、只读 rootfs、隔离网络）、节点地址冲突、脚本静态检查、注册表编译与密钥泄露——全部在任何安装之前完成。`plugin_install` 激活一个不可变的本地快照；后台蒸馏器之后把通过审查的工作发布到 GitHub（更新走 pull-request 流程），且不阻塞智能体。卸载保留工作区与历史以供审计。

### 运行环境（镜像开发）

每个插件都运行在某个**运行环境**上：一个由宿主审批、digest 钉死的基础镜像及其解释器，保存在预置了 39 个条目的目录中——基础 OS 镜像（alpine、debian、ubuntu、python、rocker-verse、bioconductor）、生物信息工具链（samtools、bcftools、bwa、minimap2、fastqc、salmon、gatk4、ensembl-vep、bedtools、macs2、blast、mafft、iqtree、kraken2 等）与医学影像（orthanc、ohif-viewer、monai）。用户可通过 gateway（Docker Hub 搜索 + 审批）批准更多镜像。新运行环境以 git 工作区形式在 `/environments/dev/<id>` 编写，含 `manifest.toml` 与 `Containerfile`；必须通过六道门禁（四道静态，加构建与冒烟测试）才能本地激活——构建产物的 digest 被钉进目录，随后后台蒸馏器可将其发布到 `ghcr.io/auto-nomics/environments`。构建与推送由宿主完成——智能体从不直接接触 Podman 或镜像仓库。

## DAG 调度与容器执行

两套机制让系统在生物医学工作中可信：

- **基于 DAG 的调度。** 科研请求由 Researcher 解析，规划为类型化、经 JSON Schema 校验的节点 DAG，经同一注册表装配。同一个 DAG 可以混合快速的进程内 DataFusion / Arrow 变换与重型外部容器；调度器经可插拔执行器契约（`local`、`process`，以及带 artifact store 的协调端 `remote` 执行器，为将来接入批调度器留好位置）异步分发任务，并保留输出以供快照、diff 与分支操作。
- **容器化节点。** 每个外部工具运行在自己的临时 Podman 容器中。镜像以 sha256 digest 钉死，参考面板从 `manifest.json` 校验和验证后只读挂载，rootfs 为 `--read-only` 并施加 `no-new-privileges` 与显式的 CPU / PID / shm / UID 上限，声明的输出以 `FileRef = size + SHA-256` 经 pending-object + 原子改名写入 VFS，消费者永远不会看到半成品 artifact。容器工作目录内容寻址：完全相同的重跑会复用先前的工作区（Nextflow 式 resume），输入默认以只读 bind mount 暂入，后台清扫器负责回收过期工作区。

## 能力地图

| 领域 | 主要 crate | 提供什么 |
| --- | --- | --- |
| 模型编排 | `agentik-sdk`, `agentik-types`, `agentik-proc`, `agentik-core`, `agentik-network`, `runtime` | 流式 LLM 客户端、工具 schema 与调用、持久记忆、智能体类别与 profile、仅委派的多智能体拓扑、sync-to-async 宿主。 |
| 分析执行 | `dag-core`, `data-engine`, `data-engine-tools`, `crates/node-bundles/*` | 节点 trait、插件注册表、类型化端口、带可插拔执行器的调度器、JSON schema spec、智能体工具。 |
| 自我改进 | `plugin-rsi`, `container-plugin`, `skills`, `evolution-core` | 插件 RSI 生命周期与工具、manifest 编译/加载、skill 库与无 LLM 进化回路、共享观察存储与路由。 |
| 数据基础设施 | `vfs`, `data-catalog`, `container-runtime`, `biofusion` | OpenDAL 支撑的 VFS、Hugging Face 托管的版本化数据包、Podman 执行、不可变面板缓存、生物格式 DataFusion 读取器。 |
| 统计与流行病学 | `statkit`, `epi`, `hypothesize`, `nodes-power`, `cmprsk`, `crrkit`, `survey`, `mice`, `hierint` | 描述统计与回归；因果推断与中介；可组合检验与 p 值工作流；前瞻性效能与样本量设计；竞争风险；调查设计；插补；层次交互模型。 |
| 机器学习与深度学习 | `ml`, `dl` | 预处理、特征工程、聚类、监督模型、集成、异常检测、降维；基于 Burn 的 MLP、DeepSurv、DeepHit、RNN、Transformer 与自编码器工作流。广义随机森林以容器化 `grf` 插件家族发布（官方 R grf）。 |
| 统计遗传学 | `ldsc`, `mr`, `lava`, `mrlap`, `lcv`, `cpassoc`, `magma`, `coloc`, `bkmr`, `evalue`, `genomic_sem`, `lcmm` | LD score 回归、孟德尔随机化、局部遗传相关、共定位、贝叶斯核机器回归、E 值分析、Genomic SEM、潜类别混合模型及相关移植。 |
| 断点回归 | `rdrobust`, `rdpower`, `rdmulti`, `rddensity`, `rdlocrand` | 局部多项式 RD 估计、效能与样本量计算、多断点设计、操纵检验、局部随机化推断。 |
| 科研数据客户端 | `eutils`, `opengwas`, `gwascatalog-sdk`, `opentargets`, `chembl`, `uniprot`, `string-sdk`, `enrichr-sdk`, `kegg`, `reactome`, `ensembl`, `rcsb`, `alphafold`, `interpro`, `pubchem`, `protocolio`, `clinicaltrials`, `nhanes` | PubMed/Entrez、OpenGWAS、GWAS Catalog、Open Targets、ChEMBL、UniProt、STRING、Enrichr、KEGG、Reactome、Ensembl、RCSB、AlphaFold、InterPro、PubChem、protocols.io、ClinicalTrials.gov 与 NHANES 的 SDK、智能体工具及部分 DAG source 节点。 |
| 文献、写作与知识 | `arxiv`, `biorxiv`, `openalex`, `crossref`, `embase`, `europepmc`, `semantic-scholar`, `bib-types`, `bib-base`, `writing-types`, `writing-base`, `kms`, `kms-tools` | 统一文献检索与全文管理、DAG 边上的证据通道、内容寻址文档、BibTeX/RIS/Markdown/CSL 导出、LaTeX AST 操作、引文解析、编译与知识树工具。 |
| 接口 | `apps/autonomics`, `gateway`, `headless`, `api-server`, `ascii-dag-core` | 常驻 gateway 守护进程、流式终端对话与 DAG 视图、无头 CLI 执行、文献库 CLI/API/前端、TUI DAG 渲染器。 |

默认 `data-engine` 构建启用全部 node-bundle Cargo feature。库使用者可以关闭默认 feature,只选择需要的 `bundle-*`。

## 插件家族

所有容器化分析节点都以 **manifest 插件** 形式发布：每个家族一个目录，内含 `manifest.toml`（节点契约：参数、端口、面板、镜像出处）、执行脚本与镜像构建树。插件从钉死 commit SHA 的 git 仓库安装，由守护进程启动时的插件预检加载——格式与工作流见[容器插件编写](docs/plugins/README_zh.md)；替换既有硬编码 wrapper 时使用[迁移工作流](docs/plugin-node-migration_zh.md)。面板数据包由 `autonomics panels sync` 供给：它下载并对校验和验证本地目录缓存中缺失的每一个 `[[panels]]` 数据集引用；启动预检只在本地做有界、离线安全的存在性检查，守护进程就绪从不等待网络。无人值守部署可设置 `AUTONOMICS_PANEL_SYNC=1` 让 `autonomics serve` 内联执行供给。

当前发布 28 个精选家族 / 99 个节点类型，在注册表中以 `family/node_kind` 寻址（内置节点使用 `core/` 命名空间）：

| 家族 | 节点类型 | 工具 |
| --- | --- | --- |
| [ldsc](https://github.com/auto-nomics/ldsc-plugin) | `ldsc_h2`, `ldsc_munge`, `ldsc_rg` | LD score 回归（h2 / munge / rg） |
| [magma](https://github.com/auto-nomics/magma-plugin) | `magma_annotate` | MAGMA SNP 到基因注释 |
| [mrpresso](https://github.com/auto-nomics/mrpresso-plugin) | `mrpresso` | MR-PRESSO 异质性 / 离群检验 |
| [mvmr](https://github.com/auto-nomics/mvmr-plugin) | `mvmr` | 多变量孟德尔随机化 |
| [coloc](https://github.com/auto-nomics/coloc-plugin) | `coloc_abf` | 共定位（coloc.abf） |
| [deseq2](https://github.com/auto-nomics/deseq2-plugin) | `deseq2_de` | 差异表达（DESeq2） |
| [gcta](https://github.com/auto-nomics/gcta-plugin) | `gcta_cojo_select`, `gcta_sblup`, `gcta_fastbat`, `gcta_acat` | GCTA 汇总统计套件 |
| [pathway-gsea](https://github.com/auto-nomics/pathway-gsea-plugin) | `pathway_gsea` | fgsea 通路富集 |
| [clusterprofiler](https://github.com/auto-nomics/clusterprofiler-plugin) | `clusterprofiler_ora`, `clusterprofiler_gsea` | ORA 与 GSEA 富集（clusterProfiler） |
| [plink2](https://github.com/auto-nomics/plink2-plugin) | `plink2_clump` | LD clumping（PLINK2） |
| [visualization](https://github.com/auto-nomics/visualization-plugin) | `visualization` | 用户脚本 R 绘图渲染 |
| [mtag](https://github.com/auto-nomics/mtag-plugin) | `mtag` | GWAS 多性状分析 |
| [smr](https://github.com/auto-nomics/smr-plugin) | `smr_heidi`, `smr_heidi_eqtlgen` | SMR & HEIDI（Westra / eQTLGen） |
| [susie](https://github.com/auto-nomics/susie-plugin) | `susie_rss` | SuSiE 精细定位（RSS） |
| [twosamplemr](https://github.com/auto-nomics/twosamplemr-plugin) | `twosamplemr`, `twosamplemr_harmonise` | TwoSampleMR + harmonisation |
| [mixer](https://github.com/auto-nomics/mixer-plugin) | `mixer_fit1`, `mixer_fit2` | gsa-mixer fit1 / fit2 |
| [music](https://github.com/auto-nomics/music-plugin) | `music_deconvolution` | MuSiC 细胞类型解卷积 |
| [mutation](https://github.com/auto-nomics/mutation-plugin) | `mutation_analysis`, `mutation_analysis_clinical` | maftools 突变全景 |
| [timesfm](https://github.com/auto-nomics/timesfm-plugin) | `timesfm_forecast` | TimesFM 时序预测（离线 checkpoint） |
| [twas](https://github.com/auto-nomics/twas-plugin) | `twas_fusion` | FUSION TWAS 基因表达 |
| [hdl](https://github.com/auto-nomics/hdl-plugin) | `hdl_l`, `hdl_l_scan` | HDL-L 遗传力 + 染色体扫描 |
| [lava](https://github.com/auto-nomics/lava-plugin) | `lava`, `lava_scan` | 局部遗传相关 + 扫描 |
| [single-cell](https://github.com/auto-nomics/single-cell-plugin) | `single_cell_preprocessor`、10 个 `h5ad_*` / `gene_set_score` / `sc_dense_ingest` 变体 | scRNA 预处理与 H5AD 分析 |
| [radiomics](https://github.com/auto-nomics/radiomics-plugin) | 21 个 `radiomics_*` / `pyradiomics_*` 变体 | 影像特征提取流水线 |
| [pathology](https://github.com/auto-nomics/pathology-plugin) | 7 个 `pathology_*` 变体 | WSI 摄入 / QC / 嵌入 / IHC |
| [bulk-rnaseq](https://github.com/auto-nomics/bulk-rnaseq-plugin) | `limma_voom`, `wgcna` | limma+voom 差异表达、WGCNA 模块 |
| [hyprcoloc](https://github.com/auto-nomics/hyprcoloc-plugin) | `hyprcoloc` | HyPrColoc 多性状共定位 |
| [grf](https://github.com/auto-nomics/grf-plugin) | 23 个 `grf_*` 类型：12 个森林训练器、`grf_predict_forest`、ATE / best-linear-projection / calibration / scores、forest weights / split frequencies / variable importance / get-tree / merge、`grf_generate_causal_data` | 广义随机森林（官方 R grf 2.6.1） |

上表是精选起点而非上限：上文描述的开发者回路已经产出由智能体编写的家族，例如 `phylo-treeness`、`phylo-pis`、`donor-paired-composition`、`python-script` 与 `h5ad-obs`，并经同一套渠道发布。

### 构建插件

- [插件编写概览](docs/plugins/README_zh.md)：生命周期、核心规则与文档地图。
- [插件编写指南](docs/plugins/authoring-guide_zh.md)：以 `clusterProfiler` ORA 为例的端到端教程。
- [Manifest 参考](docs/plugins/manifest-reference_zh.md)：规范性 schema、模板语义与启动校验。
- [测试与发布清单](docs/plugins/testing-and-release_zh.md)：测试金字塔、镜像 digest 发布、Git 钉定与净室审查。
- [Plugin-based RSI 设计](docs/design/plugin-based-rsi.md)：开发者回路背后的请求、提案、校验门禁与可信发布模型。

### 安装插件

在 `~/.autonomics/plugins.toml` 中声明家族；`autonomics serve` 在守护进程启动前运行预检，按钉定的版本安装每个家族（git clone 或本地 `path` 符号链接），fail-closed 校验每个 manifest，并报告注册的类型：

```toml
[[plugin]]
name = "ldsc"
git = "https://github.com/auto-nomics/ldsc-plugin.git"
rev = "6f7118d61dd60ca7ce95d7d524ccec3880d96026"   # 钉定的 commit SHA
```

本地开发使用 `path` 来源（符号链接——改动在下次重启生效，无需重装）：

```toml
[[plugin]]
name = "mtag"
path = "/mnt/projects/node-plugins/mtag"
```

## 工作空间结构

```text
autonomics/
├── apps/autonomics/                     终端应用与 CLI 子命令
├── crates/
│   ├── agentik-*/                LLM SDK、类型、proc 宏、运行时、网络
│   ├── dag-core/                 DAG trait、注册表、调度器、执行器契约
│   ├── data-engine/              DataFusion 引擎与 node-bundle 装配
│   ├── data-engine-tools/        DAG 操作的智能体 ToolFunction 适配器
│   ├── node-bundles/             feature 门控的分析节点插件
│   ├── plugin-rsi/               递归自我改进基座（插件）
│   ├── container-plugin/         manifest 编译、加载、同步、工厂
│   ├── skills/, evolution-core/  skill 库与无 LLM 进化回路
│   ├── vfs/                      OpenDAL 支撑的虚拟文件系统与 vbash 工具
│   ├── data-catalog/             版本化数据包目录
│   ├── container-runtime/        Podman 执行与不可变面板缓存
│   ├── gateway/, headless/       常驻守护进程与无头 CLI 执行
│   ├── biofusion*/               生物格式 DataFusion 读取器与缓存
│   ├── runtime/                  RuntimeHost 与共享智能体基础设施
│   ├── bib-*/writing-*/kms*      文献、稿件与知识系统
│   └── ...                       科研 API SDK 与支撑 crate
├── bio_crates/                   遗传学、随机森林、RD 及相关方法
├── stat_crates/                  统计、流行病学、ML、插补与 DL
├── fixtures/                     有代表性的合法与畸形测试输入
├── docs/                         设计文档与专题指南
├── infra/                        本地参考数据准备工具
├── dendrite/                     独立的嵌套知识管理工作空间
└── scripts/                      安装、面板构建与维护脚本
```

`reference/` 目录存放第三方与对比材料，不属于根 Cargo 构建。每用户状态位于 `~/.autonomics` 之下：`plugins.toml` 与 v2 插件树（`plugins/dev`、`plugins/snapshots`、`plugins/runtime`）、运行环境目录（`plugin-environments.toml`、`environments/dev`）、skill 库及其进化存储（`skills/`、`skill-observations/`、`skill-proposals/`、`skill-usage.toml`）、面板缓存（`panels/`）以及 SQLite 数据库（`agent.db`、`knowledge.db`、`bib.db`、`writing.db`、`dag-history.db`）。

## 快速开始

### 安装预编译二进制

在出现带 tag 的 GitHub release 之后，无需重新构建即可安装平台二进制：

```bash
curl --fail --location https://raw.githubusercontent.com/auto-nomics/autonomics/main/scripts/install.sh | bash
```

脚本下载匹配的 Linux 或 macOS 二进制，校验 `SHA256SUMS`，并安装到 `~/.local/bin`。可用 `AUTONOMICS_INSTALL_DIR` 覆盖目标目录，用 `AUTONOMICS_VERSION=v0.1.0` 钉定 release，或用 `AUTONOMICS_REPO=owner/repo` 指向 fork。

### 构建并运行

要求：

- Rust 1.85 或更新版本；构建使用检入的 `Cargo.lock` 以保证可复现。
- C/C++ 编译器（vendored GRF core 需要）。
- 使用 OCI 节点、面板包、radiomics、visualization 或 TimesFM 时需要 Podman。
- 可选：编译稿件用 XeLaTeX、TUI HTTP 前端用 Bun、部分交叉验证脚本用 R。

```bash
git clone --recurse-submodules <repository-url>
cd autonomics
cargo run -p autonomics
```

首次完整工作空间构建规模很大。若默认 target 目录不合适，可设置 `CARGO_TARGET_DIR=/path/to/target`。

二进制名为 `autonomics`；不带子命令时默认启动 TUI：

```bash
autonomics tui                     # 交互式终端前端（默认）
autonomics serve --daemon          # 启动分离模式的 gateway 守护进程
autonomics serve status | stop     # 查看 / 优雅停止守护进程
autonomics run "..." --json        # 经守护进程的一次无头提问
autonomics export-run <run_id> --format crate --out <dir>   # RO-Crate / PROV-JSON 溯源
autonomics panels sync             # 供给插件面板数据包
autonomics kms                     # 知识管理 TUI
autonomics cache refresh-opengwas
autonomics bib list
```

使用 Cargo 时把子命令放在 `--` 之后，例如：

```bash
cargo run -p autonomics -- run "总结最近关于 BMI 的 GWAS meta 分析。" --json
```

### 配置运行时

模型服务商在 TUI（Select model）或经 gateway 的 model-config API 配置，存储在应用数据库中。内置服务商预设包括 Aliyun Bailian、DeepSeek、MiMo、MiniMax、Moonshot、OpenAI/ChatGPT、OpenRouter、SenseNova、StepFun 与 Z.ai，也支持自定义 Anthropic 兼容端点。模型按智能体以 `provider:model` 选择。

科研 API 凭据必须存在于进程环境中。仓库附带导出 `.env` 的 direnv 包装；否则请在 shell 或服务定义中导出变量。以 [`.env.example`](.env.example) 为模板，只为你使用的服务提供凭据。常见示例：

- `OPENGWAS_TOKEN`
- `OPENALEX_API_KEY`
- `EUTILS_API_KEY`
- `PROTOCOLS_IO_ACCESS_TOKEN`
- `EMBASE_API_KEY`、`EMBASE_INSTTOKEN` 及相关 Embase token
- `UMLS_API_KEY`
- `MINERU_API_KEY`（PDF 全文抽取）
- 数据目录用 `HUGGING_FACE_TOKEN` 或 `HF_TOKEN`；S3/OSS 凭据仅可选的非目录 VFS 后端需要

默认状态存于 `~/.autonomics`，下载文件存于 `~/.autonomics/data`，VFS 挂载从 `$AUTONOMICS_STATE_DIR/vfs.toml` 读取。用 `AUTONOMICS_STATE_DIR`、`AUTONOMICS_DATA_DIR`、`AUTONOMICS_BIB_DB`、`AUTONOMICS_WRITING_DB`、`AUTONOMICS_KMS_DB` 重定位状态。

### 使用本地 HTTP API

gateway 守护进程默认在 `127.0.0.1:8765` 上提供文献前端、Swagger UI 与 `/api/v1` API。可用 `AUTONOMICS_HTTP_API_ADDR` 覆盖地址。若服务暴露到回环之外，请设置 `AUTONOMICS_HTTP_API_TOKEN` 以对 API 路由启用 Bearer 认证。

```bash
curl http://127.0.0.1:8765/api/health
curl 'http://127.0.0.1:8765/api/v1/bib/articles?query=gwas&limit=10'
```

API 还提供实时智能体控制（`/api/v1/agents…`）、SSE 事件流（`/api/v1/events`）、插件与运行环境管理（`/api/v1/plugins…`）以及 skill 库与进化状态（`/api/v1/skills…`）。前端开发工作流见 [docs/api-server_zh.md](docs/api-server_zh.md)。

## 分析模型

系统通过注册的节点地址（`plugin/node`，内置为 `core/node`）与 JSON spec 暴露分析能力。spec 经各工厂的 JSON Schema 校验后经插件注册表实例化。

```rust,no_run
use data_engine::DataEngine;
use serde_json::json;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let mut engine = DataEngine::builder().build();

engine.add_node_from_registry(
    "read",
    "file_to_dataframe",
    json!({ "path": "input.csv", "format": "csv" }),
)?;
engine.add_node_from_registry(
    "filter",
    "sql",
    json!({ "sql_query": "SELECT * FROM port_0 WHERE value > 0" }),
)?;
engine.add_node_from_registry(
    "write",
    "dataframe_to_file",
    json!({ "path": "output.parquet", "format": "parquet", "mode": "overwrite" }),
)?;
engine.add_edge("read", "filter", 0, 0)?;
engine.add_edge("filter", "write", 0, 0)?;

let report = engine.run().await?;
assert!(report.ok, "pipeline errors: {:?}", report.errors);
# Ok(())
# }
```

智能体工具暴露同样的操作，无需直接可变访问引擎：

- 发现节点类型、spec、端口与文档
- 新增、更新、检视、删除节点与边
- 运行并检视 DAG
- 查看 Graphviz DOT 输出
- 创建 ref、检视历史、展示快照、diff 快照与分支

确切目录取决于运行时：node bundle 是 Cargo feature，manifest 插件按部署安装。运行中的智能体用 `list_node_factories`；Rust 中用 `DataEngine::list_nodes()`。

## 可复现数据与容器

外部分析运行时经 `container-runtime` 与 Podman 隔离。一次容器运行有声明的镜像 digest、argv 命令、输入、输出契约、超时、资源限制、网络策略与 artifact 前缀。参考数据从不可变目录包解析，经面板缓存校验和验证后挂载进临时工作区。镜像与数据包独立版本化，所有已发布工具镜像都以不可变 manifest digest 钉死在 `ghcr.io/auto-nomics/autonomics` 之下——见 [GHCR 容器镜像](docs/ghcr-migration.md)。

容器执行要求 Podman 运行时与 VFS 工作区、面板缓存位于同一主机可达范围。默认 TUI 容器镜像不挂载 Podman socket，因此 OCI 节点面向主机直跑的 TUI 或显式配置的远程运行时。

## 开发

```bash
# 只编译全部测试目标，不运行测试。
cargo test --workspace --no-run

# 聚焦测试循环。
cargo test -p data-engine
cargo test -p dag-core
cargo test -p container-plugin
cargo test -p plugin-rsi
cargo test -p skills
cargo test -p biofusion
cargo test -p agentik-core
cargo test -p runtime
cargo test -p gateway
cargo test -p api-server
cargo test -p epi
cargo test -p statkit
cargo test -p ldsc
cargo test -p mr
cargo test -p nodes-io

# 格式化与 lint。
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

部分测试调用在线公共 API、需要凭据或私有镜像、下载大型面板或调用 Podman。许多外部资源测试标记为 `#[ignore]`；仅当依赖可用时用 `cargo test -- --ignored` 显式运行。插件行为由 `crates/container-plugin/tests/` 中的黄金迁移测试覆盖（每个家族一套），运行环境开发生命周期由 `crates/plugin-rsi/tests/env_dev.rs` 覆盖。

TUI HTTP 前端使用 Bun：

```bash
cd crates/api-server/frontend
bun install
bun test
bun run typecheck
bun run build
```

## 文档索引

多数专题指南同时提供中文版（`*_zh.md`）与英文原版。

### 平台与基础设施

- [SDK 指南](docs/sdk_zh.md)：消息、流式、服务商适配器、工具、文件、批处理与用量。
- [智能体运行时](docs/agent-runtime_zh.md)：智能体循环、上下文、记忆、生命周期与编排。
- [工具编写](docs/tool-authoring_zh.md)：类型化 `ToolFunction` 输入与生成的 schema。
- [Gateway 架构设计](docs/design/gateway-architecture.md)：常驻守护进程、线协议与多前端设计。
- [Headless 运行模式设计](docs/headless-run-design.md)：`autonomics run`、`RunEvent` 契约与退出码。
- [VFS 设计](docs/vfs.md)：挂载、并发写、目录叠加与参考数据。
- [数据目录](docs/data-catalog.md)：数据包布局、发布与面板引用。
- [运行时 bundle](docs/data-bundles.md)：内置 bundle 标识符与运行时叠加。
- [证据通道](docs/evidence-channel.md)：引文作为 DAG 文件边上的类型化数据。
- [容器执行](docs/container-execution-design.md)：Podman 契约与生命周期。
- [容器迁移工作流](docs/container-node-migration.md)：镜像、数据包与 wrapper 验收标准。
- [容器插件编写](docs/plugins/README_zh.md)：独立插件仓库、manifest 契约、测试与不可变发布。
- [Plugin-based RSI 设计](docs/design/plugin-based-rsi.md)：从反馈到可信发布的受监督插件进化。
- [GHCR 容器镜像](docs/ghcr-migration.md)：registry 命名空间与 digest 钉定。

### 分析方法

- [统计与流行病学节点](docs/sta_epi_nodes_zh.md)
- [假设检验设计](docs/hypothesize-design.md)
- [LD Score Regression](docs/stat-genetics/ldsc.md)
- [TwoSampleMR](docs/stat-genetics/mr.md)
- [LAVA](docs/stat-genetics/lava.md)
- [MiXeR](docs/stat-genetics/mixer.md)
- [SuSiE-RSS](docs/stat-genetics/susie-rss.md)
- [TWAS/FUSION](docs/stat-genetics/twas-fusion.md)
- [Radiomics Stage-A 节点](docs/radiomics_nodes.md)
- [Visualization 容器](docs/visualization_zh.md)

### 科研工作流

- [TUI 指南](docs/tui_zh.md)
- [API Server（HTTP API）](docs/api-server_zh.md)
- [写作系统设计](docs/writing-system-design.md)
- [Dendrite 知识管理工作空间](dendrite/README.md)
- [TimesFM 服务](../../node-plugins/timesfm/README.md)（timesfm 插件检出）
- [本地基础设施](infra/README.md)

## 范围与边界

- Biofusion 当前实现生物格式的读取路径，不含写入器。
- 外部 API 行为、速率限制、凭据、数据集可用性与服务条款仍由调用方负责。
- 容器化分析需要能访问对应镜像与数据包。缺失的私有镜像或面板是环境前提，不会回退到未验证的本地安装。
- 自我改进在设计上就是受监督的：确定性门禁与人工审查站在每个提案与运行系统之间，智能体不持有 git、registry 或主机文件系统凭据。
- KEGG 供学术使用；非学术使用需要相应的 KEGG 许可。
- TimesFM checkpoint 有各自特定的许可。在生产或商业工作中使用前请查阅容器文档。
- 数值移植在可行处与 R、Python 或原始实现对齐验证，但本系统是科研软件，不是经认证的临床或监管决策系统。

## 许可证

工作空间元数据为继承它的 Autonomics 包声明 MIT 许可；当前未检入独立的顶层许可证文件。Vendored 与容器化的第三方软件保留其上游许可。GPL 许可的工具（如 grf R 包）只在 digest 钉死的容器插件内运行，像所有其他工具家族一样在镜像边界隔离；没有任何 GPL 代码被编译进或链接进工作空间二进制。数据集与模型 checkpoint 携带各自条款。
