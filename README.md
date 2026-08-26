# autonomics / agentik

[English](README.md) | [中文](README_zh.md)

`autonomics` is a Rust workspace for **agent-driven epidemiological and statistical-genetics research**. Rather than scripts and notebooks, an analyst converses with an LLM agent that builds, runs, and inspects real computational pipelines — composing a DataFusion DAG of typed nodes, reading genomic formats (VCF / BGEN / PLINK), fitting statistical-genetics models, running epidemiological and clinical-biostatistics analyses (causal inference, mediation, survival, regression, mixture models, …), and persisting results through VFS-mounted storage.

Four pieces, usually kept separate, are integrated here:

- **LLM SDK + agent runtime** (`agentik-*`) — an Anthropic-compatible client with multi-provider support, SSE streaming, and tool / function calling, on top of an agent loop that handles memory compaction, lifecycle management, and multi-agent orchestration.
- **DataFusion DAG engine** (`data-engine`) — a typed, concurrently-scheduled node graph where each step transforms `DataFrame`s. The agent assembles and runs pipelines through tool calls; heavy computation — statistical genetics (MiXeR, LDSC, MR, LAVA) and epidemiology/clinical biostatistics (causal inference, mediation, survival, ROC, LASSO, WQS, …) — runs as pure-Rust node logic over [`faer`](https://github.com/sarah-ek/faer).
- **Bioinformatics I/O** (`biofusion`, `vfs`) — DataFusion readers for common genomic formats and mount-aware file access to reference panels and derived datasets.
- **Scientific data clients** (`eutils`, `opengwas`, `gwascatalog-sdk`, `opentargets`) — fetch metadata and summary statistics from NCBI, OpenGWAS, the GWAS Catalog, and the Open Targets Platform without leaving the conversation.

## Demo

### Article Retrieve

![pubmed query](docs/pubmed.gif)

## Architecture

```text
┌──────────────────────────────────────────────────────────────────┐
│                           tui (Ratatui)                          │
└──────────────────────────────┬───────────────────────────────────┘
                               │ MPSC events
┌──────────────────────────────▼───────────────────────────────────┐
│                    runtime + agentik-core                         │
│  Agent loop → Tool dispatch → Memory compaction → Lifecycle       │
└──────────────────────────────┬───────────────────────────────────┘
                               │ ToolFunction calls
        ┌──────────────────────┼──────────────────────┐
        │                      │                      │
        ▼                      ▼                      ▼
  agentik-tools          runtime tools         data-engine-tools
  (bash, lifecycle)      (list/query tables)    (add_node, run_dag)
                                                        │
                                               ┌────────▼────────┐
                                               │  data-engine     │
                                               │  ┌─────────────┐ │
                                               │  │  DAG graph  │ │
                                               │  │  + scheduler│ │
                                               │  └──────┬──────┘ │
                                               │         │        │
                                               │  ┌──────▼──────┐ │
                                               │  │   nodes     │ │
                                               │  │ source_file │ │
                                               │  │ sql_node    │ │
                                               │  │ ldsc_h2_cont│ │
                                               │  │ univariate  │ │
                                               │  │ _mixer      │ │
                                               │  │ two_sample  │ │
                                               │  │ _mr         │ │
                                               │  │ cox_regress │ │
                                               │  │ survival    │ │
                                               │  │ causal      │ │
                                               │  │ mediation   │ │
                                               │  │ cmest       │ │
                                               │  │ epi_roc     │ │
                                               │  │ epi_rcs     │ │
                                               │  │ epi_lasso   │ │
                                               │  │ epi_wqs     │ │
                                               │  │ sink_file   │ │
                                               │  │ viz         │ │
                                               │  │ ...         │ │
                                               │  └─────────────┘ │
                                               └────────┬────────┘
                                                        │ reads
        ┌───────────────────────────────────────────────┼──────────┐
        │                  Data Infrastructure           │          │
        │                                               ▼          │
        │  ┌──────────┐  ┌───────────┐  ┌─────────────┐           │
        │  │ af.eur_af│  │ld_matrix. │  │ mixer.       │           │
        │  │          │  │eur_chr{N} │  │ eur_tagsuff  │           │
        │  └──────────┘  └───────────┘  └─────────────┘           │
        │       ▲              ▲                ▲                   │
        │       │              │                │                   │
        │  ┌────┴──────────────┴────────────────┴──┐                │
        │  │         VFS catalog                │                │
        │  │    (VFS mounts + biofusion readers)      │                │
        │  └────────────────────────────────────────┘                │
        │                                                           │
        │  Offline:                                                  │
        │    precompute_tags ──► eur_tagsuff (sufficient stats)      │
        │    precompute_tags ──► eur_subgraph (tag-induced LD edges) │
        └───────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────┐
│              bio_crates (statistical genetics ports)              │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────┐  ┌──────────┐ │
│  │    mixer     │  │    ldsc      │  │    mr    │  │   lava   │ │
│  │ fit1 / fit2  │  │ h² / rg / cts│  │ IVW, etc │  │ bivariate│ │
│  │ spike & slab │  │ LDSC regress │  │ MR tests │  │ local rg │ │
│  └──────┬───────┘  └──────┬───────┘  └────┬─────┘  └────┬─────┘ │
│         │                 │               │              │       │
│  ┌──────┴───────┐  ┌──────┴──────┐  ┌─────┴────┐               │
│  │    hdl       │  │    mtag     │  │  mrlap   │  ┌──────────┐  │
│  │  HDL-L       │  │ multi-trait │  │ overlap  │  │   lcv    │  │
│  │  local rg    │  │   GWAS      │  │  MR      │  │ latent   │  │
│  └──────────────┘  └─────────────┘  └──────────┘  └──────────┘  │
│  ┌──────────────┐  ┌──────────────┐                               │
│  │   cpassoc    │  │    magma     │                               │
│  │ SHom / SHet  │  │ gene-set     │                               │
│  └──────────────┘  └──────────────┘                               │
│                              │                                    │
│                         faer (linear algebra)                     │
└──────────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────┐
│         stat_crates (epidemiology & biostatistics)                │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐    │
│  │                      statkit                              │    │
│  │  OLS / WLS / logistic / Cox regression, descriptive stats │    │
│  └──────────────────────────┬───────────────────────────────┘    │
│                             │                                     │
│  ┌──────────────────────────▼───────────────────────────────┐    │
│  │                        epi                                │    │
│  │  causal (IPTW/PSM) · mediation · cmest (CMAverse)         │    │
│  │  survival (KM/log-rank) · competing risk · multistate     │    │
│  │  ROC/AUC/DeLong · RCS · LASSO · WQS · chi-square          │    │
│  │  CLPM · GBTM · LCA · SEM · Random Forest + SHAP           │    │
│  └──────────────────────────────────────────────────────────┘    │
│                              │                                    │
│                         faer (linear algebra)                     │
└──────────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────┐
│                     External Data Sources                         │
│                                                                  │
│  GWAS Catalog  │  OpenGWAS  │  NCBI E-utilities  │  VCF / BGEN   │
│  (gwascatalog) │ (opengwas) │     (eutils)       │  (biofusion)  │
│  Open Targets (opentargets) — target–disease association scores  │
└──────────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────┐
│                    Test Data (OSS archive)                        │
│                                                                  │
│  aliyun://autonomics-data/mixer/test-data/                        │
│  ├── fixtures/          (cross_validation.rs)                     │
│  └── scz-chr22-repro/   (scz_chr22_repro.rs)                      │
│                                                                  │
│  Restore: rclone copy aliyun://autonomics-data/<path>/ <local>/   │
└──────────────────────────────────────────────────────────────────┘
```

An agent receives tools from `agentik-core`. The data-engine tools communicate with one serialized `DataEngineServer` through channels, so a conversation can create, inspect, run, and clear a data-processing DAG without sharing mutable engine state directly. The DAG reads data into DataFusion `DataFrame`s, transforms it, and can persist file outputs.

Heavy computation runs in pure Rust within DAG nodes. Statistical-genetics algorithms (MiXeR, LDSC, MR, LAVA, HDL, MTAG, …) and epidemiological/clinical-biostatistics methods (causal inference, mediation, survival analysis, regression, mixture models, …) are both built on `faer`. LD reference data and precomputed sufficient statistics are loaded from VFS mounts; the offline `precompute_tags` pipeline materializes per-tag summary scalars so that runtime fitting never scans the full LD matrix.

API clients for GWAS Catalog, OpenGWAS, NCBI E-utilities, and the Open Targets Platform let agents fetch metadata, summary statistics, and target–disease association scores without leaving the conversation. Large test fixtures (LD matrices, gold-standard outputs) are kept in a private OSS bucket and restored via `rclone`.

## Workspace

| Area                         | Members                                                                                                                                    | Responsibility                                                                                                                                                     |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Agent platform               | `agentik-types`, `agentik-sdk`, `agentik-proc`, `agentik-core`, `agentik-tools`, `runtime`                                                 | API types and clients, declarative tool schemas, agent lifecycle/memory, tool implementations, and sync-to-async hosting.                                          |
| Data analysis                | `data-engine`, `data-engine-tools`, `vfs`, `biofusion`, `biofusion-cache`, `visualization`                    | DAG execution, Agent-exposed DAG operations, mount-aware OpenDAL files, biological-format ingestion, and R/ggplot2 visualization.             |
| Statistical genetics         | `ldsc`, `mr`, `mixer`, `lava`, `hdl`, `mtag`, `mrlap`, `lcv`, `cpassoc`, `magma`                                                           | Pure-Rust ports of LD Score Regression, TwoSampleMR, MiXeR (spike-and-slab causal mixture), LAVA (local genetic correlation), HDL-L, MTAG, MRlap, LCV, CPASSOC, and MAGMA, built on `faer`. |
| Epidemiology & biostatistics | `statkit`, `epi`                                                                                                                           | Foundational statistics (OLS/WLS/logistic/Cox regression, descriptive stats) and higher-level epidemiological methods (causal inference, mediation, survival, ROC, RCS, LASSO, WQS, CLPM, GBTM, LCA, SEM, competing risks, multistate, Random Forest + SHAP), built on `faer`. |
| Scientific data clients      | `eutils`, `opengwas`, `gwascatalog-sdk`, `opentargets`                                                                                     | Clients for NCBI E-utilities, OpenGWAS, the GWAS Catalog, and the Open Targets Platform.                                                                          |
| User interface and rendering | `tui`                                                                                                                                      | Terminal Agent UI.                                                                                                                                                 |

`fixtures/` contains representative and malformed genomics files used by reader and integration tests.

## Getting started

Requirements:

- Rust 1.85 or later (edition 2024)
- A writable Cargo target directory. This checkout configures `/mnt/disk2/target`; override it if unavailable.

```bash
# Compile tests without running network-dependent integration tests.
CARGO_TARGET_DIR=/tmp/autonomics-target cargo test --workspace --no-run

# Run the terminal UI. Configure a provider and model in its Config tab.
CARGO_TARGET_DIR=/tmp/autonomics-target cargo run -p tui
```

For direct SDK use, copy `.env.example` to `.env` and provide only the credentials for the provider you intend to use.

## Documentation

### Agent Platform

- [**SDK (`agentik-sdk`)**](docs/sdk.md) — Messages API, SSE streaming, multi-provider abstraction, model pool, token & cost tracking, quick-start examples, and configuration.
- [**Agent Runtime (`agentik-core`)**](docs/agent-runtime.md) — Uniform agent loop, reactive context, memory compaction, toolset, lifecycle, multi-agent `ProcessManager`, and proc macros.
- [**Tool Authoring**](docs/tool-authoring.md) — How to implement `ToolFunction` with strongly-typed inputs via `#[derive(ToolInput)]`.
- [**Container Development**](docs/container-development.md) — Persistent k3s development workspaces, captured exec, and reproducible OCI packaging.
- [**Container Node Migration**](docs/container-node-migration.md) — Standard workflow for moving analysis tools to OCI images, cataloged panels, and thin DAG wrappers.

### Statistical Genetics

- [**LD Score Regression (`ldsc`)**](docs/stat-genetics/ldsc.md) — h², rg, cell-type-specific analysis, LD-score computation, sumstat munging.
- [**TwoSampleMR (`mr`)**](docs/stat-genetics/mr.md) — Wald ratio, IVW, MR-Egger, median/mode, harmonisation, Steiger filtering.
- [**MiXeR (`mixer`)**](docs/stat-genetics/mixer.md) — Univariate/bivariate spike-and-slab causal mixture, sufficient-statistics compression, DAG node pipeline.
- [**LAVA (`lava`)**](docs/stat-genetics/lava.md) — Local genetic correlation from GWAS summary statistics.

### Epidemiology & Biostatistics

- [**Statistical & Epi Nodes**](docs/sta_epi_nodes.md) — DAG node catalogue for epidemiological and clinical-biostatistics analyses: linear/logistic/Cox regression, survival (KM + log-rank), causal inference (IPTW/PSM), causal mediation (VanderWeele / CMAverse), ROC/AUC/DeLong, RCS, LASSO, WQS, chi-square, meta-analysis.

### Visualization

- [**Visualization (`visualization`)**](docs/visualization.md) — DataFusion → PNG rendering via R/ggplot2 (Arrow IPC bridge, opendal output).

## Workspace structure

```
autonomics/
├── apps/
│   └── tui/                 # Ratatui terminal application
├── crates/
│   ├── agentik-*/           # LLM API client, type system, macros, and Agent runtime
│   ├── data-engine/         # DataFusion DAG model, nodes, and scheduler
│   ├── data-engine-tools/   # ToolFunction adapters for data-engine operations
│   ├── biofusion/           # DataFusion readers for genomics file formats
│   ├── biofusion-cache/     # Caching layer for biofusion readers
│   ├── vfs/                 # OpenDAL-backed mount-aware file storage and tools
│   ├── visualization/       # DataFusion → PNG rendering via R/ggplot2 (VizNode)
│   ├── eutils/              # NCBI E-utilities client
│   ├── opengwas/            # OpenGWAS client
│   ├── gwascatalog-sdk/     # GWAS Catalog client
│   ├── opentargets/         # Open Targets Platform GraphQL client (target–disease associations)
│   ├── runtime/             # Synchronous host bridge for Agentik
├── bio_crates/
│   ├── ldsc/                # Pure-Rust LD Score Regression (h²/rg/cts) port
│   ├── mr/                  # Pure-Rust TwoSampleMR (Mendelian randomization) port
│   ├── mixer/               # Pure-Rust MiXeR univariate + bivariate (spike-and-slab) port
│   ├── lava/                # Pure-Rust LAVA local genetic correlation port
│   ├── hdl/                 # Pure-Rust HDL-L enhanced local genetic correlation port
│   ├── mtag/                # Pure-Rust MTAG (multi-trait GWAS) port
│   ├── mrlap/               # Pure-Rust MRlap (sample-overlap-aware MR) port
│   ├── lcv/                 # Pure-Rust LCV (latent causal variable) port
│   ├── cpassoc/             # Pure-Rust CPASSOC (cross-phenotype meta-analysis) port
│   ├── magma/               # Pure-Rust MAGMA (gene-set / gene-property analysis) port
├── stat_crates/
│   ├── statkit/             # Foundational stats: OLS/WLS/logistic/Cox regression, descriptive stats
│   └── epi/                 # Epidemiology & clinical biostatistics: causal inference, mediation,
│                            #   survival, competing risks, ROC, RCS, LASSO, WQS, CLPM, GBTM, LCA, SEM, RF+SHAP
├── fixtures/                # Valid and malformed genomics input fixtures
├── Cargo.toml               # Workspace manifest
└── .cargo/config.toml       # Default Cargo target directory
```

## Development

Run an individual package while iterating on it:

```bash
CARGO_TARGET_DIR=/tmp/autonomics-target cargo test -p data-engine
CARGO_TARGET_DIR=/tmp/autonomics-target cargo test -p biofusion
CARGO_TARGET_DIR=/tmp/autonomics-target cargo test -p agentik-core
CARGO_TARGET_DIR=/tmp/autonomics-target cargo test -p ldsc
CARGO_TARGET_DIR=/tmp/autonomics-target cargo test -p mr
CARGO_TARGET_DIR=/tmp/autonomics-target cargo test -p epi
CARGO_TARGET_DIR=/tmp/autonomics-target cargo test -p statkit
```

Some integration tests call external public APIs or require provider credentials. Treat those as opt-in when running in CI or offline environments.

## Test data & reproducibility

Large test data (LD matrices, GWAS sumstats, original-software gold-standard outputs) is **not committed to git**. It lives in a private OSS object storage bucket and is restored via `rclone`:

```bash
# Example: restore mixer cross-validation fixtures
rclone copy aliyun://autonomics-data/mixer/test-data/fixtures/ \
  bio_crates/mixer/tests/fixtures/
```

> **Note:** The bucket is currently private. Contact the repository owner for access credentials. Once provisioned, configure `rclone` with the provided endpoint, key, and secret — the `aliyun:` remote name in the examples above should point to that configuration.


## Requirements

- Rust 1.85+ (edition 2024)
- **R (optional)** — only needed for the `visualization` node. Install R with the `arrow` and `ggplot2` packages and ensure `Rscript` is on `PATH` (or set `VISUALIZATION_RSCRIPT`). Without R, the rest of the workspace builds and runs unchanged.

## License

MIT
