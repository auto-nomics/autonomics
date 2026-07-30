# autonomics / agentik

[English](README.md) | [中文](README_zh.md)

`autonomics` is a Rust workspace for **agent-driven bioinformatics and data analysis**. Rather than scripts and notebooks, an analyst converses with an LLM agent that builds, runs, and inspects real computational pipelines — composing a DataFusion DAG of typed nodes, reading genomic formats (VCF / BGEN / PLINK), fitting statistical-genetics models, and persisting results to an Iceberg data lake.

Four pieces, usually kept separate, are integrated here:

- **LLM SDK + agent runtime** (`agentik-*`) — an Anthropic-compatible client with multi-provider support, SSE streaming, and tool / function calling, on top of an agent loop that handles memory compaction, lifecycle management, and multi-agent orchestration.
- **DataFusion DAG engine** (`data-engine`) — a typed, concurrently-scheduled node graph where each step transforms `DataFrame`s. The agent assembles and runs pipelines through tool calls; heavy statistical-genetics computation (MiXeR, LDSC, MR, LAVA) runs as pure-Rust node logic over [`faer`](https://github.com/sarah-ek/faer).
- **Bioinformatics I/O** (`biofusion`, `datalake`) — DataFusion readers for common genomic formats and an Iceberg-backed lake holding LD reference panels and precomputed sufficient-statistics tables.
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
  agentik-tools          datalake-tools         data-engine-tools
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
        │  │         Iceberg catalog                │                │
        │  │    (datalake + biofusion readers)      │                │
        │  └────────────────────────────────────────┘                │
        │                                                           │
        │  Offline:                                                  │
        │    precompute_tags ──► eur_tagsuff (sufficient stats)      │
        │    precompute_tags ──► eur_subgraph (tag-induced LD edges) │
        └───────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────┐
│                    bio_crates (pure algorithm ports)              │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────┐  ┌──────────┐ │
│  │    mixer     │  │    ldsc      │  │    mr    │  │   lava   │ │
│  │ fit1 / fit2  │  │ h² / rg / cts│  │ IVW, etc │  │ bivariate│ │
│  │ spike & slab │  │ LDSC regress │  │ MR tests │  │ local rg │ │
│  └──────┬───────┘  └──────┬───────┘  └────┬─────┘  └────┬─────┘ │
│         │                 │               │              │       │
│         └─────────────────┴───────────────┴──────────────┘       │
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

Heavy statistical-genetics computation (MiXeR, LDSC) runs in pure Rust within DAG nodes. LD reference data and precomputed sufficient statistics are stored in an Iceberg data lake; the offline `precompute_tags` pipeline materializes per-tag summary scalars so that runtime fitting never scans the full LD matrix.

API clients for GWAS Catalog, OpenGWAS, NCBI E-utilities, and the Open Targets Platform let agents fetch metadata, summary statistics, and target–disease association scores without leaving the conversation. Large test fixtures (LD matrices, gold-standard outputs) are kept in a private OSS bucket and restored via `rclone`.

## Workspace

| Area                         | Members                                                                                                                                    | Responsibility                                                                                                                                                     |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Agent platform               | `agentik-types`, `agentik-sdk`, `agentik-proc`, `agentik-core`, `agentik-tools`, `runtime`                                                 | API types and clients, declarative tool schemas, agent lifecycle/memory, tool implementations, and sync-to-async hosting.                                          |
| Data analysis                | `data-engine`, `data-engine-tools`, `stat-primitives`, `fs`, `datalake`, `datalake-tools`, `biofusion`, `biofusion-cache`, `visualization` | DAG execution, Agent-exposed DAG operations, statistics, OpenDAL files, Iceberg storage and query tools, biological-format ingestion, and R/ggplot2 visualization. |
| Statistical genetics         | `ldsc`, `mr`, `mixer`, `lava`, `hdl`, `mtag`, `mrlap`, `lcv`                                                                               | Pure-Rust ports of LD Score Regression, TwoSampleMR, MiXeR (spike-and-slab causal mixture), LAVA (local genetic correlation), HDL-L, MTAG, MRlap, and LCV, built on `faer`. |
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

### Statistical Genetics

- [**LD Score Regression (`ldsc`)**](docs/stat-genetics/ldsc.md) — h², rg, cell-type-specific analysis, LD-score computation, sumstat munging.
- [**TwoSampleMR (`mr`)**](docs/stat-genetics/mr.md) — Wald ratio, IVW, MR-Egger, median/mode, harmonisation, Steiger filtering.
- [**MiXeR (`mixer`)**](docs/stat-genetics/mixer.md) — Univariate/bivariate spike-and-slab causal mixture, sufficient-statistics compression, DAG node pipeline.
- [**LAVA (`lava`)**](docs/stat-genetics/lava.md) — Local genetic correlation from GWAS summary statistics.

### Data Infrastructure & Visualization

- [**Visualization (`visualization`)**](docs/visualization.md) — DataFusion → PNG rendering via R/ggplot2 (Arrow IPC bridge, opendal output).
- [Data Infrastructure](docs/data_infra.md) — LD matrix, subgraph, and univariate MiXeR pipeline docs.

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
│   ├── datalake/            # Iceberg REST catalog and DataFusion integration
│   ├── datalake-tools/      # Agent tools for querying and describing Iceberg tables
│   ├── fs/                  # OpenDAL-backed file storage and file tools
│   ├── visualization/       # DataFusion → PNG rendering via R/ggplot2 (VizNode)
│   ├── eutils/              # NCBI E-utilities client
│   ├── opengwas/            # OpenGWAS client
│   ├── gwascatalog-sdk/     # GWAS Catalog client
│   ├── opentargets/         # Open Targets Platform GraphQL client (target–disease associations)
│   ├── stat-primitives/     # Descriptive statistics, distributions, regression
│   ├── runtime/             # Synchronous host bridge for Agentik
├── bio_crates/
│   ├── ldsc/                # Pure-Rust LD Score Regression (h²/rg/cts) port
│   ├── mr/                  # Pure-Rust TwoSampleMR (Mendelian randomization) port
│   ├── mixer/               # Pure-Rust MiXeR univariate + bivariate (spike-and-slab) port
│   ├── lava/                # Pure-Rust LAVA local genetic correlation port
│   ├── hdl/                 # Pure-Rust HDL-L enhanced local genetic correlation port
│   ├── mtag/                # Pure-Rust MTAG (multi-trait GWAS) port
│   ├── mrlap/               # Pure-Rust MRlap (sample-overlap-aware MR) port
│   └── lcv/                 # Pure-Rust LCV (latent causal variable) port
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

Every test file that depends on external data documents its archive path and restore command in a header comment. See [`docs/data_infra/univariate_mixer.md`](docs/data_infra/univariate_mixer.md) and the memory entry `test-data-archive-convention` for the full convention.

## Requirements

- Rust 1.85+ (edition 2024)
- **R (optional)** — only needed for the `visualization` node. Install R with the `arrow` and `ggplot2` packages and ensure `Rscript` is on `PATH` (or set `VISUALIZATION_RSCRIPT`). Without R, the rest of the workspace builds and runs unchanged.

## License

MIT
