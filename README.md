# autonomics / agentik

[English](README.md) | [中文](README_zh.md)

Autonomics is a purpose-built harness for biomedical research. It places an LLM agent in control of a typed, auditable research surface: the agent discovers registered analysis nodes, assembles a DataFusion DAG, reads scientific data, runs pure-Rust or OCI-container methods, persists results through a virtual file system, and can carry those results into a literature-backed manuscript. Epidemiology, statistical genetics, clinical and survey analysis, machine learning, scientific databases, and reproducible execution are treated as parts of one research workflow rather than as separate applications.

The root Cargo workspace contains 94 crates at the time of this README update. It is an active research codebase: APIs, node contracts, and configuration paths may still change. The `dendrite/` directory is a separate nested Rust workspace for the knowledge-management system.

## Harness Contract

Here, "harness" means the load-bearing infrastructure around the model and the methods:

- **Model cockpit**: provider/model configuration, streaming conversations, tool schemas, memory, lifecycle, and multi-agent orchestration.
- **Capability registry**: JSON-schema-validated tools and typed DAG nodes expose analysis, I/O, source, sink, and writing operations to the model.
- **Evidence plane**: biomedical APIs, literature services, full-text storage, bibliography records, citations, and knowledge-tree access share one runtime.
- **Execution plane**: DataFusion, Arrow, VFS, immutable data packages, Podman, and reference-panel caches execute and preserve the work.
- **Research protocol**: DAG history, snapshots, diffs, branches, provenance, immutable artifacts, and numerical cross-validation make agent work inspectable and replayable.

Autonomics is not a general-purpose chat application, a notebook replacement built around free-form scripts, or a monolithic pipeline. It is the harness that connects a model to checked research capabilities and records what was actually run.

## Demo

![PubMed article retrieval](docs/pubmed.gif)

## Harness Architecture

```text
Ratatui TUI + local HTTP API
        |
        v
RuntimeHost
  agents, profiles, sessions, memory, host tools
        |
        | ToolFunction calls
        v
Data-engine actor
  node discovery, add/update nodes, edges, run/history
        |
        v
Node registry + DAG scheduler
  JSON-schema validated node specs
  typed input and output ports
  asynchronous execution and retained outputs
        |
        +--> SQL, I/O, epi, genetics, ML, DL, survey,
        |     survival, causal, MR, RD, writing bundles
        |
        +--> DataFusion DataFrame and Arrow RecordBatch nodes
        |
        +--> OCI container nodes
              ephemeral Podman runs
              immutable panel bundles
              artifact publication to VFS

Shared data plane:
  VFS (OpenDAL) -> local, S3, OSS, and catalog-backed mounts
  data catalog  -> immutable, versioned data packages
  biofusion     -> VCF/BCF/FASTA/FASTQ/BED/GTF/GFF/SAM/BAM/CRAM/BigWig/BigBed readers
```

The architecture separates model orchestration from research capabilities. Each agent session gets its own `DataEngine` actor, while immutable registry and runtime infrastructure are shared across the process. DAG execution is fire-and-forget from the agent command loop, so one long run does not block other agents. When DAG history is enabled, runs create snapshot lineages that can be inspected, diffed, branched, and checked out. Selected graphs can also be reverse-compiled to R or Python source.

## Capability Map

| Area | Main crates | What it provides |
| --- | --- | --- |
| Model orchestration | `agentik-sdk`, `agentik-types`, `agentik-proc`, `agentik-core`, `agentik-network`, `runtime` | Streaming LLM clients, tool schemas and calls, persistent memory, lifecycle, multi-agent topology, and a sync-to-async host. |
| Analysis execution | `dag-core`, `data-engine`, `data-engine-tools`, `crates/node-bundles/*`, `workflow-editor` | Node traits, plugin registry, typed ports, scheduler, JSON-schema specs, agent tools, snapshots, code generation, and reusable workflow skills. |
| Data infrastructure | `vfs`, `data-catalog`, `container-runtime`, `biofusion` | OpenDAL-backed VFS, versioned object-storage packages, Podman execution, immutable panel caches, and biological-format DataFusion readers. |
| Statistics and epidemiology | `statkit`, `epi`, `hypothesize`, `cmprsk`, `survey`, `mice`, `hierint` | Descriptive statistics and regression; causal inference and mediation; composable tests and p-value workflows; competing risks; survey designs; imputation; hierarchical interaction models. |
| Machine learning and deep learning | `ml`, `dl`, `grf`, `grf-sys` | Preprocessing, feature engineering, clustering, supervised models, ensembles, anomaly detection, dimensionality reduction; Burn-based MLP, DeepSurv, DeepHit, RNN, Transformer, and autoencoder workflows; generalized random forests through the vendored C++ core. |
| Statistical genetics | `ldsc`, `mr`, `lava`, `mrlap`, `lcv`, `cpassoc`, `magma`, `coloc`, `bkmr`, `evalue`, `genomic_sem`, `lcmm` | LD score regression, Mendelian randomization, local genetic correlation, colocalization, Bayesian kernel-machine regression, E-value analysis, Genomic SEM, latent-class mixed models, and related ports. |
| Regression discontinuity | `rdrobust`, `rdpower`, `rdmulti`, `rddensity`, `rdlocrand` | Local-polynomial RD estimation, power and sample-size calculations, multi-cutoff designs, manipulation testing, and local randomization inference. |
| Scientific data clients | `eutils`, `opengwas`, `gwascatalog-sdk`, `opentargets`, `chembl`, `uniprot`, `string-sdk`, `kegg`, `reactome`, `ensembl`, `rcsb`, `alphafold`, `interpro`, `pubchem`, `clinicaltrials` | SDKs, agent tools, and selected DAG source nodes for PubMed/Entrez, OpenGWAS, GWAS Catalog, Open Targets, ChEMBL, UniProt, STRING, KEGG, Reactome, Ensembl, RCSB, AlphaFold, InterPro, PubChem, and ClinicalTrials.gov. |
| Literature, writing, and knowledge | `arxiv`, `biorxiv`, `openalex`, `crossref`, `embase`, `europepmc`, `semantic-scholar`, `bib-types`, `bib-base`, `writing-types`, `writing-base`, `kms`, `kms-tools` | Unified literature search and full-text management, content-addressed documents, BibTeX/RIS/Markdown/CSL export, LaTeX AST operations, citation resolution, compilation, and knowledge-tree tools. |
| Harness interface | `tui`, `tui-http`, `workflow-editor` | Streaming terminal chat, provider/model configuration, DAG view, bibliography CLI/API/frontend, KMS browser, and workflow editor components. |

The default `data-engine` build enables all node-bundle Cargo features. A library consumer can disable default features and select only needed `bundle-*` features.

## Harness Workspace Layout

```text
autonomics/
├── apps/tui/                     Terminal application and CLI subcommands
├── crates/
│   ├── agentik-*/                LLM SDK, types, proc macros, runtime, networking
│   ├── dag-core/                 DAG traits, registry, scheduler, and codegen
│   ├── data-engine/              DataFusion engine and node-bundle wiring
│   ├── data-engine-tools/        Agent ToolFunction adapters for DAG operations
│   ├── node-bundles/             Feature-gated analysis-node plugins
│   ├── vfs/                      OpenDAL-backed virtual file system and vbash tools
│   ├── data-catalog/             Versioned data-package catalog
│   ├── container-runtime/        Podman execution and immutable panel cache
│   ├── biofusion*/               Biological-format DataFusion readers and cache
│   ├── runtime/                  RuntimeHost and shared agent infrastructure
│   ├── bib-*/writing-*/kms*      Literature, manuscript, and knowledge systems
│   └── ...                       Scientific API SDKs and supporting crates
├── bio_crates/                   Genetics, causal-forest, RD, and related methods
├── stat_crates/                  Statistics, epidemiology, ML, imputation, and DL
├── containers/                   OCI definitions for external analysis runtimes
├── fixtures/                     Representative valid and malformed test inputs
├── docs/                         Design documents and topic guides
├── infra/                        Local reference-data preparation utilities
├── dendrite/                     Separate nested knowledge-management workspace
└── tests/                        Cross-language validation helpers
```

The `reference/` directory contains third-party and comparison material and is not part of the root Cargo build.

## Getting Started

### Build and run the TUI

Requirements:

- Rust 1.85 or newer; the checked-in `Cargo.lock` is used for reproducible builds.
- A C/C++ compiler for the vendored GRF core.
- Podman when using OCI-backed nodes, panel bundles, radiomics, visualization, or TimesFM.
- Optional: XeLaTeX for manuscript compilation, Bun for the TUI HTTP frontend, and R for selected cross-validation scripts.

```bash
git clone --recurse-submodules <repository-url>
cd autonomics
cargo run -p tui
```

The first full workspace build is large. If the default target directory is unsuitable, set `CARGO_TARGET_DIR=/path/to/target`.

The workspace binary is named `tui`. After installing or copying it as `autonomics-tui`, the same CLI is available as:

```bash
autonomics-tui tui
autonomics-tui kms
autonomics-tui cache refresh-opengwas
autonomics-tui bib list
```

With Cargo, pass the subcommand after `--`; for example:

```bash
cargo run -p tui -- cache refresh-opengwas
cargo run -p tui -- bib list
```

### Configure the runtime

Model providers are configured in the TUI Config tab and stored in the application database. Built-in presets include DeepSeek, MiMo, MiniMax, Moonshot, OpenAI/ChatGPT, OpenRouter, SenseNova, and Z.ai, and custom Anthropic-compatible endpoints are supported.

Scientific API credentials must be present in the process environment. The repository includes a direnv wrapper that exports `.env`; otherwise, export the variables in your shell or service definition. Use [`.env.example`](.env.example) as the template, and provide only the credentials for services you use. Common examples include:

- `OPENGWAS_TOKEN`
- `OPENALEX_API_KEY`
- `EUTILS_API_KEY`
- `EMBASE_API_KEY`, `EMBASE_INSTTOKEN`, and related Embase tokens
- `UMLS_API_KEY`
- OSS/S3 credentials for catalog and VFS backends

By default, state is stored under `~/.autonomics`, downloaded files under `~/.autonomics/data`, and VFS mounts are read from `$AUTONOMICS_STATE_DIR/vfs.toml`. Use `AUTONOMICS_STATE_DIR`, `AUTONOMICS_DATA_DIR`, `AUTONOMICS_BIB_DB`, and `AUTONOMICS_WRITING_DB` to relocate state.

### Use the local HTTP API

When the TUI starts, it serves a local bibliography frontend and `/api/v1` API on `127.0.0.1:8765` by default. Override the address with `AUTONOMICS_HTTP_API_ADDR`. If the server is exposed beyond loopback, set `AUTONOMICS_HTTP_API_TOKEN` to require bearer authentication for API routes.

```bash
curl http://127.0.0.1:8765/api/health
curl 'http://127.0.0.1:8765/api/v1/bib/articles?query=gwas&limit=10'
```

The frontend development workflow is documented in [docs/tui-http-api_zh.md](docs/tui-http-api_zh.md).

## Analysis Model

The harness exposes analysis through registered node `kind` values and JSON specs. Specs are validated against each factory's JSON Schema and instantiated through the plugin registry.

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

Agent tools expose the same operations without direct mutable engine access:

- discover node kinds, specs, ports, and documentation
- add, update, inspect, and remove nodes and edges
- run and inspect a DAG
- view Graphviz DOT output
- compile a DAG to R or Python
- create refs, inspect history, show snapshots, diff snapshots, and branch

The exact catalog is runtime-dependent because node bundles are Cargo features. In a running agent, use `list_node_factories`; in Rust, use `DataEngine::list_nodes()`.

## Reproducible Data and Containers

External analysis runtimes are isolated through `container-runtime` and Podman. A container run has a declared image digest or tag, argv command, inputs, output contracts, timeout, resource limits, network policy, and artifact prefix. Reference data is resolved from immutable catalog packages, checksum-verified in a panel cache, and mounted into an ephemeral workspace. Images and data packages are versioned independently.

Available OCI assets include LDSC, HDL-L, LAVA, MiXeR, SuSiE-RSS, MR-PRESSO, MVMR, MTAG, FUSION TWAS, SMR, MAGMA annotation, HyPrColoc, coloc, GCTA, PLINK2, PyRadiomics, visualization, and TimesFM. See [containers/README.md](containers/README.md), [docs/container-execution-design.md](docs/container-execution-design.md), and [docs/container-node-migration.md](docs/container-node-migration.md).

Container execution requires a Podman runtime reachable on the same host as the VFS workspace and panel cache. The default TUI container image does not mount a Podman socket, so OCI-backed nodes are intended for a host-run TUI or an explicitly configured remote runtime.

## Development

```bash
# Compile all test targets without running tests.
cargo test --workspace --no-run

# Focused test loops.
cargo test -p data-engine
cargo test -p dag-core
cargo test -p biofusion
cargo test -p agentik-core
cargo test -p runtime
cargo test -p tui-http
cargo test -p epi
cargo test -p statkit
cargo test -p ldsc
cargo test -p mr
cargo test -p grf
cargo test -p nodes-io

# Formatting and lint.
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Some tests call live public APIs, require credentials or private images, download large panels, or invoke Podman. Many external-resource tests are marked `#[ignore]`; run them explicitly with `cargo test -- --ignored` only when the dependency is available. Cross-language numerical baselines under `tests/` require R and the packages named by the scripts.

The TUI HTTP frontend uses Bun:

```bash
cd crates/tui-http/frontend
bun install
bun test
bun run typecheck
bun run build
```

## Documentation

### Platform and infrastructure

- [SDK guide](docs/sdk.md): messages, streaming, provider adapters, tools, files, batches, and usage.
- [Agent runtime](docs/agent-runtime.md): agent loop, context, memory, lifecycle, and orchestration.
- [Tool authoring](docs/tool-authoring.md): typed `ToolFunction` inputs and generated schemas.
- [VFS design](docs/vfs.md): mounts, concurrent writes, catalog overlays, and reference data.
- [Data catalog](docs/data-catalog.md): package layout, publication, and panel references.
- [Runtime bundles](docs/data-bundles.md): built-in bundle identifiers and runtime overlays.
- [DAG code generation](docs/dag-codegen-design.md): reverse compilation to R and Python.
- [Container execution](docs/container-execution-design.md): Podman contracts and lifecycle.
- [Container migration workflow](docs/container-node-migration.md): image, data package, and wrapper acceptance criteria.

### Analysis methods

- [Statistical and epidemiological nodes](docs/sta_epi_nodes.md)
- [Hypothesis-testing design](docs/hypothesize-design.md)
- [LD Score Regression](docs/stat-genetics/ldsc.md)
- [TwoSampleMR](docs/stat-genetics/mr.md)
- [LAVA](docs/stat-genetics/lava.md)
- [MiXeR](docs/stat-genetics/mixer.md)
- [SuSiE-RSS](docs/stat-genetics/susie-rss.md)
- [TWAS/FUSION](docs/stat-genetics/twas-fusion.md)
- [GRF port](docs/grf_analysis.md)
- [Radiomics Stage-A nodes](docs/radiomics_nodes.md)
- [Visualization container](docs/visualization.md)

### Research workflow

- [TUI guide](docs/tui.md)
- [TUI HTTP API](docs/tui-http-api_zh.md) (Chinese)
- [Writing-system design](docs/writing-system-design.md)
- [Dendrite knowledge-management workspace](dendrite/README.md)
- [TimesFM service](containers/timesfm/README.md)
- [Local infrastructure](infra/README.md)

## Scope and Boundaries

- Biofusion currently implements read paths, not writers for biological formats.
- External API behavior, rate limits, credentials, dataset availability, and service terms remain the responsibility of the caller.
- Container-backed analyses require access to the corresponding image and data package. A missing private image or panel is an environment prerequisite, not a fallback to an unverified local installation.
- KEGG is available for academic use; nonacademic use requires an appropriate KEGG license.
- TimesFM checkpoints have checkpoint-specific licenses. Review the container documentation before using a checkpoint in production or commercial work.
- Numerical ports are validated against R, Python, or original implementations where practical, but this harness is research software and is not a certified clinical or regulatory decision system.

## License

The workspace metadata declares the MIT license for Autonomics packages that inherit it; no standalone top-level license file is currently checked in. Vendored and containerized third-party software retains its upstream license. In particular, the GRF C++ core is GPL-3, and static linking via `grf-sys` has GPL implications for distributed binaries. Dataset and model checkpoints carry their own terms.
