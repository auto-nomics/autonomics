# autonomics / agentik

[English](README.md) | [中文](README_zh.md)

`autonomics` 是一个面向**智能体驱动的生物信息学与数据分析**的 Rust 工作空间。分析人员不再依赖脚本和笔记本，而是与 LLM 智能体对话，由智能体构建、运行并检查真实的计算流水线——组装一个由类型化节点组成的 DataFusion DAG，读取基因组格式（VCF / BGEN / PLINK），拟合统计遗传学模型，并将结果持久化到 Iceberg 数据湖。

本项目整合了通常彼此分离的四个部分：

- **LLM SDK + 智能体运行时**（`agentik-*`）——兼容 Anthropic 的客户端，支持多服务商、SSE 流式输出和工具/函数调用；其上是处理记忆压缩、生命周期管理和多智能体编排的智能体循环。
- **DataFusion DAG 引擎**（`data-engine`）——一个类型化、并发调度的节点图，每个步骤变换 `DataFrame`。智能体通过工具调用组装并运行流水线；重计算量的统计遗传学计算（MiXeR、LDSC、MR、LAVA）作为纯 Rust 节点逻辑运行于 [`faer`](https://github.com/sarah-ek/faer) 之上。
- **生物信息学 I/O**（`biofusion`、`datalake`）——针对常见基因组格式的 DataFusion 读取器，以及由 Iceberg 支撑、存储 LD 参考面板和预计算充分统计量表的数据湖。
- **科学数据客户端**（`eutils`、`opengwas`、`gwascatalog-sdk`）——无需离开对话即可从 NCBI、OpenGWAS 和 GWAS Catalog 获取元数据与汇总统计数据。

## 演示

### 文献检索

![pubmed 查询](docs/pubmed.gif)

## 架构

```text
┌──────────────────────────────────────────────────────────────────┐
│                           tui (Ratatui)                          │
└──────────────────────────────┬───────────────────────────────────┘
                               │ MPSC 事件
┌──────────────────────────────▼───────────────────────────────────┐
│                    runtime + agentik-core                         │
│  智能体循环 → 工具分发 → 记忆压缩 → 生命周期                        │
└──────────────────────────────┬───────────────────────────────────┘
                               │ ToolFunction 调用
        ┌──────────────────────┼──────────────────────┐
        │                      │                      │
        ▼                      ▼                      ▼
  agentik-tools          datalake-tools         data-engine-tools
  (bash, 生命周期)       (列出/查询表)           (add_node, run_dag)
                                                        │
                                               ┌────────▼────────┐
                                               │  data-engine     │
                                               │  ┌─────────────┐ │
                                               │  │  DAG 图     │ │
                                               │  │  + 调度器    │ │
                                               │  └──────┬──────┘ │
                                               │         │        │
                                               │  ┌──────▼──────┐ │
                                               │  │   节点      │ │
                                               │  │ source_file │ │
                                               │  │ sql_node    │ │
                                               │  │ ldsc_hsq    │ │
                                               │  │ univariate  │ │
                                               │  │ _mixer      │ │
                                               │  │ bivariate   │ │
                                               │  │ _mixer      │ │
                                               │  │ sink_file   │ │
                                               │  │ viz         │ │
                                               │  │ ...         │ │
                                               │  └─────────────┘ │
                                               └────────┬────────┘
                                                        │ 读取
        ┌───────────────────────────────────────────────┼──────────┐
        │                  数据基础设施                    │          │
        │                                               ▼          │
        │  ┌──────────┐  ┌───────────┐  ┌─────────────┐           │
        │  │ af.eur_af│  │ld_matrix. │  │ mixer.       │           │
        │  │          │  │eur_chr{N} │  │ eur_tagsuff  │           │
        │  └──────────┘  └───────────┘  └─────────────┘           │
        │       ▲              ▲                ▲                   │
        │       │              │                │                   │
        │  ┌────┴──────────────┴────────────────┴──┐                │
        │  │         Iceberg catalog                │                │
        │  │    (datalake + biofusion 读取器)       │                │
        │  └────────────────────────────────────────┘                │
        │                                                           │
        │  离线：                                                     │
        │    precompute_tags ──► eur_tagsuff (充分统计量)             │
        │    precompute_tags ──► eur_subgraph (tag 诱导的 LD 边)      │
        └───────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────┐
│                    bio_crates (纯算法移植)                         │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────┐  ┌──────────┐ │
│  │    mixer     │  │    ldsc      │  │    mr    │  │   lava   │ │
│  │ fit1 / fit2  │  │ h² / rg / cts│  │ IVW, 等  │  │ 双变量   │ │
│  │ spike & slab │  │ LDSC 回归     │  │ MR 检验  │  │ 局部 rg  │ │
│  └──────┬───────┘  └──────┬───────┘  └────┬─────┘  └────┬─────┘ │
│         │                 │               │              │       │
│         └─────────────────┴───────────────┴──────────────┘       │
│                              │                                    │
│                         faer (线性代数)                            │
└──────────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────┐
│                     外部数据源                                    │
│                                                                  │
│  GWAS Catalog  │  OpenGWAS  │  NCBI E-utilities  │  VCF / BGEN   │
│  (gwascatalog) │ (opengwas) │     (eutils)       │  (biofusion)  │
└──────────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────┐
│                    测试数据 (OSS 归档)                             │
│                                                                  │
│  aliyun://autonomics-data/mixer/test-data/                        │
│  ├── fixtures/          (cross_validation.rs)                     │
│  └── scz-chr22-repro/   (scz_chr22_repro.rs)                      │
│                                                                  │
│  恢复：rclone copy aliyun://autonomics-data/<path>/ <local>/      │
└──────────────────────────────────────────────────────────────────┘
```

智能体从 `agentik-core` 获取工具。数据引擎工具通过通道与一个串行化的 `DataEngineServer` 通信，因此一次对话可以创建、检查、运行和清空数据处理 DAG，而无需直接共享可变的引擎状态。DAG 将数据读入 DataFusion `DataFrame`，对其进行变换，并可将文件输出持久化。

重计算量的统计遗传学计算（MiXeR、LDSC）在 DAG 节点内以纯 Rust 运行。LD 参考数据和预计算的充分统计量存储在 Iceberg 数据湖中；离线 `precompute_tags` 流水线物化了每个 tag 的汇总标量，使运行时拟合永远无需扫描完整 LD 矩阵。

GWAS Catalog、OpenGWAS 和 NCBI E-utilities 的 API 客户端让智能体无需离开对话即可获取元数据和汇总统计数据。大型测试夹具（LD 矩阵、金标准输出）保存在私有 OSS 存储桶中，通过 `rclone` 恢复。

## 工作空间

| 领域                         | 成员                                                                                                                                        | 职责                                                                                                                                                              |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 智能体平台                   | `agentik-types`、`agentik-sdk`、`agentik-proc`、`agentik-core`、`agentik-tools`、`runtime`                                                 | API 类型和客户端、声明式工具 schema、智能体生命周期/记忆、工具实现，以及同步到异步的托管。                                                                         |
| 数据分析                     | `data-engine`、`data-engine-tools`、`stat-primitives`、`fs`、`datalake`、`datalake-tools`、`biofusion`、`biofusion-cache`、`visualization` | DAG 执行、智能体暴露的 DAG 操作、统计量、OpenDAL 文件、Iceberg 存储与查询工具、生物格式导入，以及 R/ggplot2 可视化。                                               |
| 统计遗传学                   | `ldsc`、`mr`、`mixer`、`lava`                                                                                                              | LD Score Regression、TwoSampleMR、MiXeR（spike-and-slab 因果混合模型）和 LAVA（局部遗传相关）的纯 Rust 移植，基于 `faer`。                                          |
| 科学数据客户端               | `eutils`、`opengwas`、`gwascatalog-sdk`                                                                                                    | NCBI E-utilities、OpenGWAS 和 GWAS Catalog 的客户端。                                                                                                              |
| 用户界面与渲染               | `tui`                                                                                                                                      | 终端智能体 UI。                                                                                                                                                    |

`fixtures/` 包含读取器和集成测试使用的代表性及格式错误的基因组文件。

## 快速上手

环境要求：

- Rust 1.85 或更高版本（edition 2024）
- 可写的 Cargo target 目录。本检出版本配置了 `/mnt/disk2/target`；如果不可用，请覆盖此设置。

```bash
# 编译测试但不运行依赖网络的集成测试。
CARGO_TARGET_DIR=/tmp/autonomics-target cargo test --workspace --no-run

# 运行终端 UI。在其 Config 标签页中配置服务商和模型。
CARGO_TARGET_DIR=/tmp/autonomics-target cargo run -p tui
```

如需直接使用 SDK，请将 `.env.example` 复制为 `.env`，并仅填写你打算使用的服务商所需的凭据。

## 文档索引

### 智能体平台

- [**SDK（`agentik-sdk`）**](docs/sdk_zh.md) — Messages API、SSE 流式输出、多服务商抽象、模型池、Token 与成本追踪、快速开始示例和配置。
- [**智能体运行时（`agentik-core`）**](docs/agent-runtime_zh.md) — 统一智能体循环、响应式上下文、记忆压缩、工具集、生命周期、多智能体 `ProcessManager` 和过程宏。
- [**工具编写**](docs/tool-authoring_zh.md) — 如何通过 `#[derive(ToolInput)]` 实现带强类型输入的 `ToolFunction`。

### 统计遗传学

- [**LD Score Regression（`ldsc`）**](docs/stat-genetics/ldsc_zh.md) — h²、rg、细胞类型特异性分析、LD score 计算、汇总统计清洗。
- [**TwoSampleMR（`mr`）**](docs/stat-genetics/mr_zh.md) — Wald ratio、IVW、MR-Egger、中位数/众数、harmonisation、Steiger 过滤。
- [**MiXeR（`mixer`）**](docs/stat-genetics/mixer_zh.md) — 单变量/双变量 spike-and-slab 因果混合模型、充分统计量压缩、DAG 节点流水线。
- [**LAVA（`lava`）**](docs/stat-genetics/lava_zh.md) — 从 GWAS 汇总统计估计局部遗传相关。

### 数据基础设施与可视化

- [**可视化（`visualization`）**](docs/visualization_zh.md) — 通过 R/ggplot2 将 DataFusion 渲染为 PNG（Arrow IPC 桥接、opendal 输出）。
- [数据基础设施](docs/data_infra_zh.md) — LD 矩阵、子图和单变量 MiXeR 流水线文档。

## 工作空间结构

```
autonomics/
├── apps/
│   └── tui/                 # Ratatui 终端应用
├── crates/
│   ├── agentik-*/           # LLM API 客户端、类型系统、宏和智能体运行时
│   ├── data-engine/         # DataFusion DAG 模型、节点和调度器
│   ├── data-engine-tools/   # data-engine 操作的 ToolFunction 适配器
│   ├── biofusion/           # 基因组文件格式的 DataFusion 读取器
│   ├── biofusion-cache/     # biofusion 读取器的缓存层
│   ├── datalake/            # Iceberg REST catalog 和 DataFusion 集成
│   ├── datalake-tools/      # 查询和描述 Iceberg 表的智能体工具
│   ├── fs/                  # 基于 OpenDAL 的文件存储和文件工具
│   ├── visualization/       # 通过 R/ggplot2 将 DataFusion 渲染为 PNG (VizNode)
│   ├── eutils/              # NCBI E-utilities 客户端
│   ├── opengwas/            # OpenGWAS 客户端
│   ├── gwascatalog-sdk/     # GWAS Catalog 客户端
│   ├── stat-primitives/     # 描述性统计、分布、回归
│   ├── runtime/             # Agentik 的同步宿主桥接
├── bio_crates/
│   ├── ldsc/                # 纯 Rust LD Score Regression (h²/rg/cts) 移植
│   ├── mr/                  # 纯 Rust TwoSampleMR（孟德尔随机化）移植
│   ├── mixer/               # 纯 Rust MiXeR 单变量 + 双变量 (spike-and-slab) 移植
│   └── lava/                # 纯 Rust LAVA 局部遗传相关移植
├── fixtures/                # 有效和格式错误的基因组输入夹具
├── Cargo.toml               # 工作空间清单
└── .cargo/config.toml       # 默认 Cargo target 目录
```

## 开发

迭代某个包时单独运行它：

```bash
CARGO_TARGET_DIR=/tmp/autonomics-target cargo test -p data-engine
CARGO_TARGET_DIR=/tmp/autonomics-target cargo test -p biofusion
CARGO_TARGET_DIR=/tmp/autonomics-target cargo test -p agentik-core
CARGO_TARGET_DIR=/tmp/autonomics-target cargo test -p ldsc
CARGO_TARGET_DIR=/tmp/autonomics-target cargo test -p mr
```

部分集成测试调用外部公共 API 或需要服务商凭据。在 CI 或离线环境中运行时，请将这些视为可选。

## 测试数据与可复现性

大型测试数据（LD 矩阵、GWAS 汇总统计、原始软件的金标准输出）**未提交到 git**。它们存放在私有 OSS 对象存储桶中，通过 `rclone` 恢复：

```bash
# 示例：恢复 mixer 交叉验证夹具
rclone copy aliyun://autonomics-data/mixer/test-data/fixtures/ \
  bio_crates/mixer/tests/fixtures/
```

> **注意：** 该存储桶目前为私有。请联系仓库所有者获取访问凭据。配置完成后，使用提供的 endpoint、key 和 secret 配置 `rclone`——上述示例中的 `aliyun:` remote 名称应指向该配置。

每个依赖外部数据的测试文件都在头部注释中记录了其归档路径和恢复命令。完整约定见 [`docs/data_infra/univariate_mixer.md`](docs/data_infra/univariate_mixer.md) 和记忆条目 `test-data-archive-convention`。

## 环境要求

- Rust 1.85+（edition 2024）
- **R（可选）** —— 仅 `visualization` 节点需要。安装 R 及 `arrow` 和 `ggplot2` 包，并确保 `Rscript` 在 `PATH` 上（或设置 `VISUALIZATION_RSCRIPT`）。在没有 R 的情况下，工作空间的其余部分可正常编译和运行。

## 许可证

MIT
