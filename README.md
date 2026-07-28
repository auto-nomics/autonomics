# autonomics / agentik

`autonomics` is a Rust 2024 workspace for agent-assisted bioinformatics and data analysis. It combines an Anthropic-compatible LLM SDK and agent runtime with a DataFusion-based DAG engine, readers for common genomic formats, and Iceberg data-lake integration.

The Agent SDK originated as a hard fork of [dimichgh/anthropic-sdk-rust](https://github.com/dimichgh/anthropic-sdk-rust). The repository has since grown into a broader analysis platform.

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

API clients for GWAS Catalog, OpenGWAS, and NCBI E-utilities let agents fetch metadata and summary statistics without leaving the conversation. Large test fixtures (LD matrices, gold-standard outputs) are kept in a private OSS bucket and restored via `rclone`.

## Workspace

| Area                         | Members                                                                                                                                    | Responsibility                                                                                                                                                     |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Agent platform               | `agentik-types`, `agentik-sdk`, `agentik-proc`, `agentik-core`, `agentik-tools`, `runtime`                                                 | API types and clients, declarative tool schemas, agent lifecycle/memory, tool implementations, and sync-to-async hosting.                                          |
| Data analysis                | `data-engine`, `data-engine-tools`, `stat-primitives`, `fs`, `datalake`, `datalake-tools`, `biofusion`, `biofusion-cache`, `visualization` | DAG execution, Agent-exposed DAG operations, statistics, OpenDAL files, Iceberg storage and query tools, biological-format ingestion, and R/ggplot2 visualization. |
| Statistical genetics         | `ldsc`, `mr`, `mixer`, `lava`                                                                                                              | Pure-Rust ports of LD Score Regression, TwoSampleMR, MiXeR (spike-and-slab causal mixture), and LAVA (local genetic correlation), built on `faer`.                 |
| Scientific data clients      | `eutils`, `opengwas`, `gwascatalog-sdk`                                                                                                    | Clients for NCBI E-utilities, OpenGWAS, and the GWAS Catalog.                                                                                                      |
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

## Features

### SDK (`agentik-sdk`)

- **Messages API** — Create conversations with system prompts, multi-turn, temperature, top-p, stop sequences
- **Streaming (SSE)** — Token-by-token streaming with event-driven callbacks (`on_text`, `on_message`, `on_error`, `on_end`) and async iteration via the `Stream` trait
- **Stream reliability** — Automatic idle-timeout detection and reconnection before `MessageStart`, graceful HTTP body draining, configurable retry policies with exponential backoff and jitter
- **Tool / function calling** — JSON Schema tools, `tool_use` / `tool_result` blocks, server tools (web search)
- **Vision** — Send images via base64 or URL
- **Files API (Beta)** — Upload, list, download with SHA-256 integrity verification
- **Batch processing (Beta)** — Create and manage batch inference requests
- **Models API** — List and inspect models with capability and pricing metadata
- **Token & cost tracking** — `TokenCounter` with per-model pricing, accumulated usage, and cost estimation
- **Multi-provider abstraction** — `LlmProvider` trait with implementations:
  - Anthropic (direct)
  - DeepSeek (`deepseek-v4-pro`, `deepseek-v4-flash`)
  - MiniMax
  - SenseNova
  - Mimo
  - ZAI
- **Model pool** — Round-robin model selection across providers, with sticky selection by name
- **Flexible auth** — Anthropic `x-api-key`, Bearer token, or custom header for third-party gateways
- **Mock support** — `MockApiClient` via `mockall` for testing

### Agent runtime (`agentik-core`)

- **Uniform agent loop** — One behavioral loop for all agents. Agent personality and tooling are configured _only_ through the toolset and system prompt; no agent-specific code paths in the loop itself (see `crates/agentik-core/src/agent.rs`).
- **Reactive context** — `AgentContext` trait: implement `read()` / `write()`. The loop polls the version at each boundary and injects a `[context-update]` message into memory when it changes. Built-in `InMemoryAgentContext` for tests.
- **Memory with compaction** — `Memory` keeps a rolling list of summarized `MemoryItem`s. When token pressure rises against the model's `context_length`, the oldest segment is summarized by the LLM into a `summary` and a fresh segment is opened.
- **Toolset** — `ToolRegistration` + `Toolset` handle schema exposure, parallel dispatch, and per-tool timeouts. Every `T: ToolFunction` is auto-erased to `DynToolFunction` for heterogeneous storage.
- **Built-in tools** — `attempt_complete`, `abort_task` (lifecycle), `bash` (subprocess with kill-on-drop and tail-truncated output).
- **Lifecycle** — `AgentLifecycle` (IDLE / RUNNING / ABORTED) driven by built-in lifecycle tools, so agents self-terminate without external orchestration.
- **Retry with feedback** — Retryable `AgentError`s trigger exponential backoff and the failure reason is injected back into memory for the next attempt.
- **Observation** — Optional `mpsc` event channel streams `AgentUiEvent`s (Thinking, LlmResponse, ToolCall, ToolResult, Requesting, Done, Error) to a TUI or logger.
- **Snapshots** — `AgentSnapshotStorage` trait with a SQLite backend for persisting agent memory and status.
- **Multi-agent `ProcessManager`** — Spawn, start, stop, restart, and inject messages into multiple agents as independent tokio tasks; aggregates all per-agent events into one `broadcast::Receiver<ProcessEvent>` stream with exit status (`Completed` / `Error` / `Panicked` / `Cancelled` / `Stopped`).

### Proc macros (`agentik-proc`)

- **`#[derive(ToolInput)]`** — Generates `impl ToolInput` from a struct, including the `ToolBuilder` chain, required vs. optional fields (via `Option<T>` or `#[default = ...]`), and `#[desc = "..."]` per-field descriptions. Pair with `#[tool(name = "...", description = "...")]` on the struct.

### Statistical genetics (`ldsc`, `mr`)

- **`ldsc`** — A faithful, library-only pure-Rust port of [LD Score Regression](https://github.com/bulik/ldsc) (Bulik-Sullivan & Finucane): SNP-heritability (h²), genetic correlation (rg), cell-type-specific analysis, LD-score computation from PLINK genotypes, summary-statistic munging, and annotation building. Numerics run on [`faer`](https://github.com/sarah-ek/faer) (no LAPACK/MKL). Point estimates are cross-checked against the Python reference's own test suite and golden fixtures. The `estimate_h2` DataFrame entry point is wired into the data-engine (`data-engine/nodes/ldsc_hsq.rs`).
- **`mr`** — A pure-Rust port of [TwoSampleMR](https://github.com/MRCIEU/TwoSampleMR)'s algorithm API (no IO/plotting) for the DAG engine: Wald ratio, the IVW family, MR-Egger, median and mode estimators, `harmonise_data`, Steiger filtering, and heterogeneity/pleiotropy tests. Built on `faer` + `statrs`, with point estimates validated bit-for-bit against R golden fixtures.

### MiXeR — Causal Mixture Model (`mixer`)

A pure-Rust reimplementation of the MiXeR model (Holland et al. 2020, _PLoS Genetics_): univariate (`fit1`) and bivariate (`fit2`) spike-and-slab causal mixture for GWAS summary statistics.

- **Univariate (`fit1`)** — Fits three parameters (π polygenicity, σ²_β discoverability, σ²_zero intercept) via Gaussian moment-matching cost function. Optimization: differential evolution × N repeats → Nelder-Mead refinement. Cross-validated against the original C++ implementation's gold standard to < 0.2% across all parameters.
- **Sufficient statistics compression** — The LD matrix is folded into two per-SNP scalars (`m1`/`m2`) pre-fit, collapsing O(nnz) CSR to O(n_snp). Cost evaluation is O(1) per tag thereafter, enabling ~10^4 cost evaluations during optimization without touching LD.
- **Two weighting modes** — LdScore (single-pass, `1/(1+Σr²)`, no CSR needed) and Randprune (bit-exact with original C++ `std::mt19937_64`).
- **DAG node** — Exposed as `univariate_mixer` and `bivariate_mixer` node kinds (`data-engine/nodes/univariate_mixer.rs`). Two-phase pipeline: offline `precompute_tags` produces an Iceberg table (`eur_tagsuff`) containing per-tag sufficient statistics; the runtime node loads sumstats, joins with tagsuff by rsid, and invokes `fit1`.
- **Data infrastructure** — See [`docs/data_infra/univariate_mixer.md`](docs/data_infra/univariate_mixer.md) for the full algorithm derivation, pipeline diagram, and decision record.

### LAVA — Local Genetic Correlation (`lava`)

A pure-Rust port of LAVA (Werme et al. 2022) for estimating local genetic correlation from GWAS summary statistics and LD reference data. All 5 cross-validation tests pass against the R reference implementation.

### Visualization (`visualization`)

- **R/ggplot2 rendering** — Render a DataFusion `DataFrame` to a PNG via R's ggplot2. Data crosses the Rust→R boundary as an **Arrow IPC stream** (`arrow::ipc::writer::StreamWriter` → `arrow::read_ipc_stream`), so column types are preserved exactly — no CSV re-inference, no row-wise JSON.
- **Subprocess, not in-process R** — The renderer shells out to `Rscript` rather than linking `libR` in-process. R is therefore a _runtime-only, optional_ dependency: the workspace compiles and the core pipeline (LDSC, MR, …) runs without R installed. A missing or misconfigured R surfaces as a typed `VizError::RscriptNotFound` / `RscriptFailed` at render time, never as a build failure.
- **DAG node** — Exposed as the `visualization` node kind (`data-engine/nodes/viz.rs`), reached by the agent through the generic `add_node` / `run_dag` tools — no dedicated tool. It mirrors `SinkNode`: one untyped input port, no output ports. The rendered path is reported back via `NodeReport.artifact_path`.
- **opendal output** — The PNG is written into the engine's **opendal-virtualized filesystem** (the same isolated space as source/sink data), not the host filesystem. `output_path` is a virtual path (e.g. `/plots/scatter.png`); the opendal handle is threaded from the builder through `NodeCtx.opendal`.
- **Plot spec** — The `r_code` field is ggplot2 R code that runs with a `data.frame` named `df` already bound to the input; it must build a plot and assign it to a variable named `p`. Example: `p <- ggplot(df, aes(x = bp, y = pval)) + geom_point()`. Dimensions (`width`/`height`/`dpi`) are optional.

```text
DataFrame → collect() → Arrow IPC stream bytes → tempdir → Rscript
   (arrow::read_ipc_stream → df → <r_code> → ggsave) → PNG bytes
   → opendal op.write(virtual_path) → NodeReport.artifact_path
```

- **R requirement (optional)** — Rendering needs `Rscript` on `PATH` with the `arrow` and `ggplot2` packages installed. Override the binary with the `VISUALIZATION_RSCRIPT` env var. The engine does **not** detect or pin an R version — whichever `Rscript` the launching process resolves wins. A conda env is the cleanest way to provision a known-good R (see the crate's tests for the expected packages).

## Quick Start

### Talking to a model directly via the SDK

```toml
[dependencies]
agentik-sdk = "0.3"
```

```rust
use agentik_sdk::Anthropic;
use agentik_types::MessageCreateBuilder;

#[tokio::main]
async fn main() -> agentik_sdk::Result<()> {
    let client = Anthropic::new("your-api-key", "https://api.anthropic.com")?;

    let message = client.messages().create(
        MessageCreateBuilder::new("claude-sonnet-4-20250514", 1024)
            .system("You are a helpful assistant.")
            .user("Hello, Claude!")
            .build(),
    ).await?;

    println!("Response: {:?}", message.content);
    Ok(())
}
```

### Running an agent with a custom tool

```toml
[dependencies]
agentik-core = { path = "..." }
agentik-sdk  = { path = "..." }
agentik-proc = { path = "..." }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust
use std::sync::Arc;

use agentik_core::agent::{Agent, AgentConfig};
use agentik_core::context::InMemoryAgentContext;
use agentik_core::tools::{ToolFunction, ToolResult, ToolRegistration, error::ToolError};
use agentik_core::toolset::Toolset;
use agentik_sdk::model::model_pool::ModelPool;
use agentik_sdk::provider::mimo::{MimoProvider, MODEL_MIMO_V2_5};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// Declarative tool schema via proc macro.
#[derive(Debug, Deserialize, Serialize, agentik_proc::ToolInput)]
#[tool(name = "echo", description = "Echo back a message")]
pub struct EchoInput {
    #[desc = "The text to echo back"]
    pub text: String,
}

pub struct EchoTool;

#[async_trait]
impl ToolFunction for EchoTool {
    type Input = EchoInput;

    async fn run(&self, input: EchoInput) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::success("echo", format!("echo: {}", input.text)))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = MimoProvider::new(None, std::env::var("MIMO_API_KEY")?);
    let model = provider.get_model(MODEL_MIMO_V2_5)?;

    let mut pool = ModelPool::new();
    pool.add_model(model);

    let ctx = Arc::new(InMemoryAgentContext::new());

    let mut agent = Agent::builder()
        .with_model_pool(Arc::new(pool))
        .with_context(ctx)
        .with_system_prompt_identity("You are a minimal demo agent.")
        .with_config(AgentConfig::default())
        .build()
        .await?;

    agent.register_tool(ToolRegistration::from(EchoTool))?;
    agent.start().await?;
    Ok(())
}
```

### Multi-agent orchestration

```rust
use agentik_core::process::ProcessManager;

let manager = ProcessManager::new();

// Spawn (registers but does not start) — returns the agent ID.
let id = manager.spawn(builder).await?;

manager.start(&id)?;
manager.inject_message(&id, vec![/* ContentBlock::Text { ... } */])?;

let mut events = manager.events();
while let Ok(ev) = events.recv().await {
    println!("{ev:?}");
}

// Graceful shutdown of every agent.
let exits = manager.shutdown().await;
```

## Tool authoring

Tools implement `ToolFunction` with an associated strongly-typed `Input`. The framework deserializes the LLM's JSON into `Input` before `run` is called, so tool bodies stay free of `serde_json::Value` plumbing.

```rust
#[derive(Deserialize, agentik_proc::ToolInput)]
#[tool(name = "get_weather", description = "Current weather for a city")]
struct WeatherInput {
    #[desc = "City name"]
    city: String,

    #[desc = "Units: metric or imperial"]
    #[default = "metric"]
    units: Option<String>,
}

struct WeatherTool;

#[async_trait]
impl ToolFunction for WeatherTool {
    type Input = WeatherInput;
    async fn run(&self, i: WeatherInput) -> Result<ToolResult, ToolError> {
        // ... fetch weather ...
        Ok(ToolResult::success("weather", format!("{}: sunny", i.city)))
    }
}
```

The built-in lifecycle tools (`attempt_complete`, `abort_task`) drive agent state transitions (`IDLE` / `ABORTED`).

## Configuration

```rust
use agentik_sdk::{Anthropic, ClientConfig, LogLevel, AuthMethod};
use std::time::Duration;

let config = ClientConfig::new("your-api-key", "https://api.anthropic.com")
    .with_timeout(Duration::from_secs(120))
    .with_max_retries(3)
    .with_log_level(LogLevel::Info)
    .with_auth_method(AuthMethod::Anthropic);

let client = Anthropic::with_config(config)?;
```

### Environment variables

Copy `.env.example` to `.env` and fill in only what you need. Keys are grouped by concern:

```env
# LLM providers (set the ones you use)
MIMO_API_KEY=
SENSENOVA_API_KEY=
MINIMAX_API_KEY=        # MiniMax provider
DEEPSEEK_API_KEY=
ZAI_API_KEY=

# Iceberg data lake (datalake crate)
ICEBERG_REST_URI=
ICEBERG_S3_ACCESS_KEY_ID=
ICEBERG_S3_SECRET_ACCESS_KEY=

# Scientific data clients
OPENGWAS_TOKEN=         # OpenGWAS / GWAS Catalog bearer token
EUTILS_API_KEY=         # optional; raises NCBI rate limit 3→10 req/s
```

## API Resources (SDK)

| Resource            | Description                             |
| ------------------- | --------------------------------------- |
| `client.messages()` | Create messages and streaming responses |
| `client.batches()`  | Manage batch inference requests         |
| `client.files()`    | Upload and manage files                 |
| `client.models()`   | List and inspect models                 |

## Architecture notes

- **Agent loop design.** The core loop only provides generic capabilities — request/response cycling, lifecycle management, effect application, memory compaction. It never encodes agent-specific behavior, tool selection, or prompt engineering. Configure those exclusively via the toolset and system prompt (`agent.rs:1` documents this contract).
- **Termination.** A response with no tool calls signals completion (matches the model's trained prior). `attempt_complete` is retained for compatibility but deprecated; the loop flips to `IDLE` on the no-tool-call branch regardless.
- **Type erasure.** `ToolFunction::Input` is an associated type, so heterogeneous storage erases to `DynToolFunction` via a blanket impl — concrete call sites keep full type information.
- **Multi-agent.** Each agent runs in its own tokio task with its own command channel, status watch, and cancellation token. A forwarder task merges per-agent `AgentUiEvent`s, lifecycle changes, and task-exit signals into the manager's `ProcessEvent` broadcast stream.

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
│   ├── stat-primitives/     # Descriptive statistics, distributions, regression
│   ├── runtime/             # Synchronous host bridge for Agentik
├── bio_crates/
│   ├── ldsc/                # Pure-Rust LD Score Regression (h²/rg/cts) port
│   ├── mr/                  # Pure-Rust TwoSampleMR (Mendelian randomization) port
│   ├── mixer/               # Pure-Rust MiXeR univariate + bivariate (spike-and-slab) port
│   └── lava/                # Pure-Rust LAVA local genetic correlation port
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
