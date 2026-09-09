# autonomics / agentik

[English](README.md) | [中文](README_zh.md)

`autonomics` is a Rust workspace for **agent-driven epidemiological and statistical-genetics research**. Rather than scripts and notebooks, an analyst converses with an LLM agent that builds, runs, and inspects real computational pipelines — composing a DataFusion DAG of typed nodes, reading genomic formats (VCF / BGEN / PLINK), fitting statistical-genetics models, running epidemiological and clinical-biostatistics analyses (causal inference, mediation, survival, regression, mixture models, …), and persisting results through VFS-mounted storage.

Four pieces, usually kept separate, are integrated here:

- **LLM SDK + agent runtime** (`agentik-*`) — an Anthropic-compatible client with multi-provider support, SSE streaming, and tool / function calling, on top of an agent loop that handles memory compaction, lifecycle management, and multi-agent orchestration.
- **DataFusion DAG engine** (`data-engine`) — a typed, concurrently-scheduled node graph where each step transforms `DataFrame`s. The agent assembles and runs pipelines through tool calls. Native statistical transforms use [`faer`](https://github.com/sarah-ek/faer), while external statistical-genetics runtimes execute in pinned OCI containers with cataloged panels.
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
                                               │  │ file → df    │ │
                                               │  │ sql_node    │ │
                                               │  │ ldsc_h2_cont│ │
                                               │  │ univariate  │ │
│  │ mixer_cont  │ │
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
                                               │  │ df → file    │ │
                                               │  │ viz_cont    │ │
                                               │  │ ...         │ │
                                               │  └─────────────┘ │
                                               └────────┬────────┘
                                                        │ reads
        ┌───────────────────────────────────────────────┼──────────┐
        │                  Data Infrastructure           │          │
        │                                               ▼          │
        │  ┌──────────┐  ┌───────────┐  ┌─────────────┐           │
        │  │ af.eur_af│  │ld_matrix. │  │ catalog      │           │
        │  │          │  │eur_chr{N} │  │ panels       │           │
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
│         Remaining native statistical-genetics crates             │
│                                                                  │
│  ldsc · mr · lava · mrlap · lcv · cpassoc · magma                 │
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
│                    Reproducible test data                         │
│                                                                  │
│  Container fixtures: containers/<tool>/fixtures/                  │
│  Published panels: data catalog with immutable digests            │
└──────────────────────────────────────────────────────────────────┘
```

An agent receives tools from `agentik-core`. The data-engine tools communicate with one serialized `DataEngineServer` through channels, so a conversation can create, inspect, run, and clear a data-processing DAG without sharing mutable engine state directly. The DAG reads data into DataFusion `DataFrame`s, transforms it, and can persist file outputs.

Some computation remains in pure Rust DAG nodes, especially in-process statistical transforms and methods without an official runtime yet. External statistical-genetics tools run through pinned OCI container nodes with cataloged reference panels. Epidemiological/clinical-biostatistics methods remain built on `faer`.

API clients for GWAS Catalog, OpenGWAS, NCBI E-utilities, and the Open Targets Platform let agents fetch metadata, summary statistics, and target–disease association scores without leaving the conversation. Large test fixtures (LD matrices, gold-standard outputs) are kept in a private OSS bucket and restored via `rclone`.

## Workspace

| Area                         | Members                                                                                                                                    | Responsibility                                                                                                                                                     |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Agent platform               | `agentik-types`, `agentik-sdk`, `agentik-proc`, `agentik-core`, `agentik-tools`, `runtime`                                                 | API types and clients, declarative tool schemas, agent lifecycle/memory, tool implementations, and sync-to-async hosting.                                          |
| Data analysis                | `data-engine`, `data-engine-tools`, `vfs`, `biofusion`, `biofusion-cache`                                      | DAG execution, Agent-exposed DAG operations, mount-aware OpenDAL files, biological-format ingestion, and OCI-backed R/ggplot2 visualization. |
| Statistical genetics         | `ldsc`, `mr`, `lava`, `mrlap`, `lcv`, `cpassoc`, `magma`                                                                                   | Remaining Rust ports plus OCI container nodes for LDSC h²/rg, HDL-L, MiXeR, SuSiE-RSS, MR-PRESSO, MVMR, MTAG, FUSION TWAS, and other external tools. |
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
- [**Container Node Migration**](docs/container-node-migration.md) — Standard workflow for moving analysis tools to OCI images, cataloged panels, and thin DAG wrappers.

### Statistical Genetics

- [**LD Score Regression (`ldsc`)**](docs/stat-genetics/ldsc.md) — h², rg, cell-type-specific analysis, LD-score computation, sumstat munging.
- [**TwoSampleMR (`mr`)**](docs/stat-genetics/mr.md) — Wald ratio, IVW, MR-Egger, median/mode, harmonisation, Steiger filtering.
- [**MiXeR (`mixer_fit1_container` / `mixer_fit2_container`)**](docs/stat-genetics/mixer.md) — Official gsa-mixer fit1/fit2 runtime with a cataloged 1000G EUR panel.
- [**LAVA (`lava`)**](docs/stat-genetics/lava.md) — Local genetic correlation from GWAS summary statistics.

### Epidemiology & Biostatistics

- [**Statistical & Epi Nodes**](docs/sta_epi_nodes.md) — DAG node catalogue for epidemiological and clinical-biostatistics analyses: linear/logistic/Cox regression, survival (KM + log-rank), causal inference (IPTW/PSM), causal mediation (VanderWeele / CMAverse), ROC/AUC/DeLong, RCS, LASSO, WQS, chi-square, meta-analysis.

- [**Radiomics Stage-A Nodes**](docs/radiomics_nodes.md) — DICOM/NIfTI/RTSTRUCT ingestion, geometry validation, preprocessing, PyRadiomics extraction, QC, and feature-set assembly.

### Visualization

- [**Visualization (`visualization_container`)**](docs/visualization.md) — File-to-File PNG rendering via isolated R/ggplot2 OCI containers.

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
│   ├── eutils/              # NCBI E-utilities client
│   ├── opengwas/            # OpenGWAS client
│   ├── gwascatalog-sdk/     # GWAS Catalog client
│   ├── opentargets/         # Open Targets Platform GraphQL client (target–disease associations)
│   ├── runtime/             # Synchronous host bridge for Agentik
├── bio_crates/
│   ├── ldsc/                # Pure-Rust LD Score Regression (h²/rg/cts) port
│   ├── mr/                  # Pure-Rust TwoSampleMR (Mendelian randomization) port
│   ├── lava/                # Pure-Rust LAVA local genetic correlation port
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

Some integration tests call external public APIs, require provider credentials, or depend on published catalog panels. Treat those as opt-in when running in CI or offline environments.


## Requirements

- Rust 1.85+ (edition 2024)
- **Podman** — required by OCI-backed DAG nodes, including `visualization_container`. Visualization no longer requires R or `Rscript` on the host.

## License

MIT
