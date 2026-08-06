# Container Execution Mode for DAG Engine

## Design Document — 2026-08-06

## 1. Motivation

### 1.1 Current State

The DAG engine currently has two execution modalities:

1. **Native Rust execution** (`DagNode::execute`) — the dominant path. Each
   node kind is a hand-ported Rust reimplementation of a reference R/Python/C++
   bioinformatics tool (LDSC, MiXeR, LAVA, SuSiE, MAGMA, coloc, BKMR, …).
   The scheduler spawns `execute` as a tokio task and the node runs in-process,
   sharing the engine's `RuntimeEnv`, Iceberg catalog, and opendal filesystem.

2. **Subprocess execution** — the recently-introduced "faithful port" pattern
   (univariate/bivariate MiXeR). The Rust node is a thin shim that writes the
   upstream DataFrame to a temp file, invokes `python mixer.py fit1 …` via
   `std::process::Command`, parses the JSON output, and builds a result
   DataFrame. This guarantees 100% numerical fidelity to the original tool but
   requires the host machine to have `python`, `libbgmg.so`, reference panels,
   and all dependencies pre-installed and on PATH.

3. **Codegen** (`compile_dag`) — emits a standalone R or Python script
   reproducing the DAG, but does *not* execute it. The user must run the
   script manually in an environment with the right runtime + packages.

### 1.2 Problem

The host-dependency requirement is a growing pain point:

| Issue | Impact |
|-------|--------|
| **Environment conflicts** | R 4.5 vs system R; conda envs (`r45`) vs system Python; `libgfortran` / `libR` version mismatches across tools. The viz node already hardcodes an `r45` conda env requirement. |
| **Deployment complexity** | A new server needs a manual "install R + 200 CRAN packages + Python + bioconda tools + PLINK + MAGMA binary + libbgmg.so + …" runbook. No reproducibility guarantee. |
| **Unported tools** | Many tools (MAGMA binary, PLINK, GCTA, REGENIE, SAIGE, METAL, …) have no Rust port and no subprocess node. Users cannot use them in a DAG without writing a custom node. |
| **Reproducibility** | Two runs on different hosts can produce different results if the underlying R/Python package versions differ. |
| **Codegen gap** | `compile_dag` produces a script but the user must set up the execution environment — the "last mile" is manual. |

### 1.3 Proposed Solution

Introduce a **container execution mode** that lets a DAG node run inside a
Docker (or Docker-compatible: Podman, Apptainer/Singularity) container. The
container image encapsulates the tool's entire dependency stack (runtime,
packages, native libraries, reference data mounts). The engine handles:

- Serializing the node's upstream `DataFrame`(s) to a staging area.
- Building/pulling the container image (or using a pre-built one).
- Mounting data volumes (opendal root, Iceberg warehouse, reference panels).
- Invoking the container with the right entrypoint.
- Deserializing the output back into a `DataFrame`.

This unifies the "faithful port" subprocess pattern, the codegen "last mile",
and opens the door to running *any* native tool without a Rust port.

---

## 2. Goals & Non-Goals

### Goals

- **G1**: Any `DagNode` can declare an alternative **container execution
  strategy** that runs the equivalent computation inside a container.
- **G2**: Zero changes to the scheduler — container execution is a property
  of the node's `execute` method, transparent to the scheduling layer.
- **G3**: Bi-directional data flow: upstream DataFrames enter the container
  as files; the container's output files are read back into DataFrames.
- **G4**: Reference data (LD panels, gene annotations, genome FASTA) is
  mounted read-only from the host, not baked into images.
- **G5**: Support Docker as the primary runtime, with a trait-based backend
  so Podman / Apptainer can be added later.
- **G6**: Per-node image specification — different nodes can use different
  images (R-based, Python-based, custom binaries).
- **G7**: The existing native-Rust execution path remains the default and
  fastest; container mode is opt-in per node or per DAG run.

### Non-Goals

- **N1**: Kubernetes / remote cluster orchestration. This design is for
  single-host container execution. (A future `RemoteContainerBackend` could
  extend this to k8s, but that is out of scope.)
- **N2**: Replacing the Rust-native nodes. The ~50 ported crates stay as-is;
  container mode is an alternative, not a replacement.
- **N3**: Building images from scratch at runtime. Images are pre-built
  (manually or via CI) and referenced by tag/digest. The engine does `docker
  pull`, not `docker build`, during a DAG run.
- **N4**: Interactive/long-lived containers. Each node gets a fresh
  container that runs to completion and is removed.

---

## 3. Architecture Overview

```
 ┌──────────────────────────────────────────────────────────────────┐
 │                       DataEngineServer                           │
 │                                                                  │
 │  ┌────────────┐   ┌────────────────────────────────────────┐    │
 │  │ DAG        │   │ NodeRegistry                            │    │
 │  │ Scheduler  │──▶│   NodeFactory::build() ──▶ DagNode      │    │
 │  │ (unchanged)│   │                          │              │    │
 │  └────────────┘   └──────────────────────────┼──────────────┘    │
 │        │                                      │                   │
 │        │ execute(ctx, inputs, reporter)       │                   │
 │        ▼                                      ▼                   │
 │  ┌─────────────────────────────────────────────────────────┐     │
 │  │              DagNode::execute                            │     │
 │  │  ┌─────────────────┐    ┌──────────────────────────┐    │     │
 │  │  │ Native (in-proc)│    │ Container-backed          │    │     │
 │  │  │ (existing path) │    │ ContainerNode wrapping    │    │     │
 │  │  │                 │    │ a ContainerSpec           │    │     │
 │  │  └─────────────────┘    └──────────┬───────────────┘    │     │
 │  └───────────────────────────────────┼────────────────────┘     │
 │                                       │                          │
 │                                ┌──────▼──────┐                   │
 │                                │ Container   │                   │
 │                                │ Runtime     │                   │
 │                                │ (Docker API)│                   │
 │                                └──────┬──────┘                   │
 └───────────────────────────────────────┼──────────────────────────┘
                                         │
                            ┌────────────▼────────────┐
                            │   Container (ephemeral)  │
                            │  ┌────────────────────┐  │
                            │  │ R / Python / binary│  │
                            │  │ + packages          │  │
                            │  └────────────────────┘  │
                            │  Mounts:                  │
                            │   /data (opendal root)    │
                            │   /warehouse (iceberg)    │
                            │   /reference (read-only)  │
                            │   /staging (read-write)   │
                            └───────────────────────────┘
```

### Key Design Principles

1. **Container is an execution strategy, not a node kind.** We do *not*
   introduce a `container` node kind. Instead, any `NodeFactory` can provide
   a `ContainerSpec` alongside (or instead of) its native `execute`. The
   factory decides at build-time whether the node runs natively or in a
   container, based on spec fields or engine configuration.

2. **Data flows through the staging area, not stdin/stdout.** Arrow
   `RecordBatch`es are serialized to Parquet (or CSV for tools that expect
   it) in a staging directory that is bind-mounted into the container. The
   container writes its output to the same staging area. This avoids the
   complexity of streaming Arrow IPC over stdin/stdout (though that remains
   a future optimization for large data).

3. **The scheduler is untouched.** `ContainerNode::execute` is just another
   `async fn` that happens to `docker run` internally. The semaphore,
   topological sort, event reporting, and `RunReport` machinery all work
   unchanged.

---

## 4. Core Types

### 4.1 `ContainerSpec` — declarative container execution contract

```rust
//! crates/data-engine/src/container/spec.rs

use serde::{Deserialize, Serialize};

/// Declares how a node runs inside a container.
///
/// Produced by a `NodeFactory`'s `container_spec()` method. The engine's
/// `ContainerRuntime` interprets this to stage data, invoke the container,
/// and collect results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerSpec {
    /// Container image reference, e.g. `"ghcr.io/autonomics/r45:latest"`
    /// or `"docker.io/statgen/magma:v1.10"`.
    pub image: String,

    /// The command to run inside the container (entrypoint is defined by
    /// the image; this is the argv after the entrypoint).
    ///
    /// Supports `{{mustache}}` template placeholders resolved from
    /// `template_vars` (see below).
    pub command: Vec<String>,

    /// Working directory inside the container.
    /// Defaults to `"/staging"`.
    #[serde(default = "default_workdir")]
    pub workdir: String,

    /// Host paths to bind-mount read-only into the container.
    /// Maps `host_path → container_path`.
    /// Typical: reference panels, genome FASTA, LD scores.
    #[serde(default)]
    pub read_only_mounts: Vec<Mount>,

    /// Environment variables to set inside the container.
    #[serde(default)]
    pub env: Vec<(String, String)>,

    /// How upstream DataFrames are serialized into the container's
    /// staging area, and how the output is read back.
    #[serde(default)]
    pub io: ContainerIo,

    /// CPU limit (Docker `--cpus`). `None` = no limit.
    #[serde(default)]
    pub cpus: Option<f64>,

    /// Memory limit (e.g. `"8g"`). `None` = no limit.
    #[serde(default)]
    pub memory: Option<String>,

    /// Whether to keep the container's stdout/stderr in the node report
    /// for debugging. Default `true` (truncated to a few KB).
    #[serde(default = "default_true")]
    pub capture_logs: bool,

    /// Network mode: `"none"` (default, isolated), `"host"`, or a custom
    /// network name. Tools that download reference data need `"host"`;
    /// pure-compute tools should stay isolated.
    #[serde(default = "default_network")]
    pub network: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mount {
    pub host_path: String,
    pub container_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContainerIo {
    /// Format for serializing upstream DataFrames into `/staging/input/`.
    /// `parquet` (default), `csv`, `tsv`, `json`.
    #[serde(default = "default_input_format")]
    pub input_format: DataFormat,

    /// How many input files to expect (one per connected input port).
    /// Files are named `input_port_0.parquet`, `input_port_1.parquet`, …
    /// The container reads them from `/staging/input/`.

    /// Format the container should write its output in.
    /// The engine reads `/staging/output/` after the container exits.
    #[serde(default = "default_input_format")]
    pub output_format: DataFormat,

    /// Filenames to look for in `/staging/output/`. Defaults to
    /// `["output_port_0.{ext}"]`. Multi-output nodes specify one per port.
    #[serde(default)]
    pub output_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DataFormat {
    Parquet,
    Csv,
    Tsv,
    Json,
    ArrowIpc,
    /// Raw file (e.g. PNG from viz) — no DataFrame deserialization.
    Raw,
}

impl Default for DataFormat {
    fn default() -> Self {
        Self::Parquet
    }
}
```

### 4.2 `ContainerRuntime` — the execution backend trait

```rust
//! crates/data-engine/src/container/runtime.rs

use async_trait::async_trait;

#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    /// Ensure the image is available locally (`docker pull` if needed).
    /// Called before the first run of a node using this image.
    async fn ensure_image(&self, image: &str) -> Result<(), ContainerError>;

    /// Run a container to completion.
    ///
    /// `staging_dir` is a host tempdir that is bind-mounted to
    /// `/staging` inside the container. The engine has already written
    /// upstream DataFrames to `staging_dir/input/` before calling this.
    /// After this returns, the engine reads `staging_dir/output/`.
    async fn run(&self, req: RunRequest) -> Result<RunResult, ContainerError>;

    /// Human-readable backend name ("docker", "podman", "apptainer").
    fn name(&self) -> &'static str;
}

pub struct RunRequest {
    pub image: String,
    pub command: Vec<String>,
    pub workdir: String,
    pub env: Vec<(String, String)>,
    pub read_only_mounts: Vec<Mount>,
    pub staging_dir: std::path::PathBuf,
    pub cpus: Option<f64>,
    pub memory: Option<String>,
    pub network: String,
}

pub struct RunResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration: std::time::Duration,
}
```

### 4.3 `DockerRuntime` — the default backend

```rust
//! crates/data-engine/src/container/docker.rs

pub struct DockerRuntime {
    /// Path to the docker binary (or "docker" on PATH).
    binary: String,
}

#[async_trait]
impl ContainerRuntime for DockerRuntime {
    async fn ensure_image(&self, image: &str) -> Result<(), ContainerError> {
        // docker image inspect <image>  → if fails, docker pull <image>
    }

    async fn run(&self, req: RunRequest) -> Result<RunResult, ContainerError> {
        // Build: docker run --rm
        //   -v {staging_dir}:/staging
        //   -v {host_path}:{container_path}:ro   (per read_only_mount)
        //   --cpus={cpus}  --memory={memory}
        //   --network={network}
        //   --workdir {workdir}
        //   -e KEY=VAL ...
        //   {image} {command...}
        //
        // Run via tokio::process::Command, capture stdout/stderr.
    }

    fn name(&self) -> &'static str { "docker" }
}
```

### 4.4 `NodeFactory` extension — `container_spec`

The existing `NodeFactory` trait gains one optional method:

```rust
// In node_registry/registry.rs, add to trait NodeFactory:

/// If this node kind supports container execution, return the
/// `ContainerSpec` template. The engine's container executor resolves
/// template variables (`{{staging_dir}}`, `{{input_file}}`, spec fields)
/// before invoking the runtime.
///
/// Default: `None` — node is native-only.
fn container_spec(
    &self,
    _spec: &serde_json::Value,
) -> Option<ContainerSpec> {
    None
}
```

### 4.5 `ContainerNode` — the adapter `DagNode`

Rather than every node implementing container logic, a single generic
adapter wraps a `ContainerSpec` into a `DagNode`:

```rust
//! crates/data-engine/src/container/node.rs

pub struct ContainerNode {
    meta: NodePorts,
    spec: ContainerSpec,
    /// The kind string of the wrapped node (for reporting).
    kind: &'static str,
    /// The original node spec (for template variable resolution).
    node_spec: serde_json::Value,
}

#[async_trait]
impl DagNode for ContainerNode {
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let runtime = ctx.container_runtime
            .ok_or(DagError::Custom("no container runtime configured"))?;

        // 1. Create staging tempdir
        let staging = tempfile::tempdir()?;

        // 2. Serialize inputs → staging/input/
        for (i, input) in inputs.iter().enumerate() {
            let path = staging.path()
                .join("input")
                .join(format!("port_{i}.{}", ext(&self.spec.io.input_format)));
            write_dataframe(&input.data, &path, &self.spec.io.input_format).await?;
        }

        // 3. Resolve template variables in command
        let resolved = resolve_templates(&self.spec, &self.node_spec, staging.path());

        // 4. Run container
        reporter.info(format!("starting container: {}", self.spec.image));
        let result = runtime.run(resolved.into()).await?;

        if result.exit_code != 0 {
            return Err(DagError::NodeError {
                node_type: self.kind.into(),
                msg: format!("container exited {}: {}", result.exit_code, result.stderr),
            });
        }

        // 5. Read outputs ← staging/output/
        let mut outputs = PortOutputs::new();
        for (port_idx, filename) in self.spec.io.output_files.iter().enumerate() {
            let path = staging.path().join("output").join(filename);
            let df = read_dataframe(&path, ctx, &self.spec.io.output_format).await?;
            outputs.insert(port_idx as u8, df);
        }

        reporter.info(format!(
            "container completed in {:.1}s",
            result.duration.as_secs_f64()
        ));
        Ok(outputs)
    }
}
```

### 4.6 `NodeCtx` extension

`NodeCtx` gains an optional container runtime handle:

```rust
#[derive(Clone)]
pub struct NodeCtx {
    pub runtime_env: Arc<RuntimeEnv>,
    pub iceberg_catalog: Option<Arc<dyn CatalogProvider>>,
    pub datalake: Arc<Datalake>,
    pub opendal: Option<Arc<fs::OpendalFileStorage>>,
    /// Container runtime for container-backed node execution.
    /// `None` when the engine is not configured for containers.
    pub container_runtime: Option<Arc<dyn ContainerRuntime>>,
}
```

---

## 5. Execution Flow

### 5.1 Per-node container execution

```
Scheduler dispatches node N
  │
  ▼
ContainerNode::execute(ctx, inputs, reporter)
  │
  ├── 1. Create staging tempdir (host filesystem, under opendal root or /tmp)
  │
  ├── 2. Serialize each input DataFrame
  │      inputs[i].data ──collect()──▶ staging/input/port_i.{parquet|csv}
  │
  ├── 3. Resolve template variables in ContainerSpec.command
  │      {{staging}} → /staging
  │      {{input_file}} → /staging/input/port_0.parquet
  │      {{output_file}} → /staging/output/output_port_0.parquet
  │      {{spec.field}} → value from the node's JSON spec
  │
  ├── 4. ContainerRuntime::run(RunRequest)
  │      docker run --rm \
  │        -v {staging}:/staging \
  │        -v /reference:/reference:ro \
  │        --network none \
  │        {image} {command}
  │
  ├── 5. Check exit code
  │      ≠ 0 → DagError with captured stderr
  │
  └── 6. Deserialize output files
         staging/output/output_port_0.parquet ──▶ DataFrame
         insert into PortOutputs map
```

### 5.2 Factory build path

When a `NodeFactory` supports container execution, the build path gains a
branch:

```rust
// In NodeRegistry::build_node or DataEngine::add_node_from_registry:
//
// The engine checks (per-node) whether container mode is requested.
// This is controlled by:
//   (a) a global engine config flag: engine.use_containers
//   (b) a per-node spec field: {"_container": true}
//   (c) the factory only supports containers (no native execute)

fn build_node(&self, kind: &str, spec: Value, container_mode: bool) -> Result<Box<dyn DagNode>> {
    let factory = self.get_factory(kind)?;

    if container_mode {
        if let Some(c_spec) = factory.container_spec(&spec)? {
            // Wrap in ContainerNode — ports come from factory.ports()
            return Ok(Box::new(ContainerNode::new(
                factory.ports(),
                c_spec,
                kind,
                spec,
            )));
        }
        // Fall through to native if no container spec
    }

    // Existing native path
    factory.build(spec, self.node_ctx.clone())
}
```

### 5.3 Mode selection

Container mode is selected by precedence (highest first):

1. **Per-node spec**: `"_container": true/false` in the node's JSON spec.
   This lets the agent choose per-node at DAG-build time.

2. **Per-run override**: `DataEngineClient::run_dag_with_mode(ExecutionMode)`.
   `ExecutionMode::Native` (default) or `ExecutionMode::Container`.

3. **Engine default**: `DataEngineBuilder::container_mode(bool)` — sets the
   default for all nodes that have a `container_spec`.

4. **Auto**: `ExecutionMode::Auto` — use container for nodes that have
   `container_spec` but *no* native `execute` (future "external-tool-only"
   node kinds). Use native for nodes that have both.

---

## 6. Data Serialization

### 6.1 Input staging

Each upstream `DataFrame` is collected (eagerly materialized) and written to
`staging/input/port_{i}.{ext}`. Format is controlled by `ContainerIo::input_format`:

| Format | Extension | Use Case |
|--------|-----------|----------|
| `Parquet` | `.parquet` | Default. Preserves types, compact. Best for Rust→Rust or Arrow-aware tools. |
| `Csv` | `.csv` | Tools that read CSV (PLINK `.pheno`, most R scripts). |
| `Tsv` | `.tsv` | GWAS summary stats convention. |
| `Json` | `.json` | Tools that consume JSON config/data. |
| `ArrowIpc` | `.arrow` | Streaming Arrow — for very large data. |
| `Raw` | (passthrough) | Sink/artifact nodes (viz PNG) — engine does not deserialize. |

### 6.2 Output staging

The container writes to `staging/output/`. The engine reads back:

- `Parquet`/`ArrowIpc`: direct `SessionContext::read_parquet()` / `read_arrow()`.
- `Csv`/`Tsv`: `SessionContext::read_csv()` with schema inference.
- `Json`: `read_json()`.
- `Raw`: no DataFrame; the file path is surfaced via `NodeReport::artifact_path`.

### 6.3 Template variable resolution

The `command` field supports `{{placeholder}}` substitution:

| Placeholder | Resolves to | Example |
|-------------|-------------|---------|
| `{{staging}}` | Container path `/staging` | `--out {{staging}}/output/result.json` |
| `{{input_file}}` | First input file path | `--sumstats {{input_file}}` |
| `{{input_file:N}}` | Nth input file (port N) | `--exposure {{input_file:0}} --outcome {{input_file:1}}` |
| `{{output_file}}` | First output file path | `--out {{output_file}}` |
| `{{spec.X}}` | Value of spec field `X` | `--chr {{spec.chr}}` |

Resolution happens on the host before the `docker run` call, so all paths
are container-relative (`/staging/...`).

---

## 7. Image Strategy

### 7.1 Pre-built images

Images are built via CI or manually, tagged, and pushed to a registry.
The engine does `docker pull` (cached locally) but never `docker build`.

**Base images** (layered):

```
ghcr.io/autonomics/base-r:4.5      # R 4.5 + CRAN deps + arrow
ghcr.io/autonomics/base-python:3.12 # Python 3.12 + numpy/scipy/pandas
ghcr.io/autonomics/r45-mosaic:latest # R 4.5 + MOSAIC + all CRAN deps
```

**Tool-specific images**:

```
ghcr.io/autonomics/magma:1.10       # MAGMA binary + dependencies
ghcr.io/autonomics/mixer:2.2.1      # gsa-mixer + libbgmg.so + Python
ghcr.io/autonomics/ldsc:1.4.1       # LDSC + Python + reference data refs
ghcr.io/autonomics/plink:1.9        # PLINK 1.9
ghcr.io/autonomics/regenie:3.2      # REGENIE
```

### 7.2 Image manifest

A `container-images.toml` at the engine config level maps node kinds to
default images:

```toml
[images]
univariate_mixer = "ghcr.io/autonomics/mixer:2.2.1"
bivariate_mixer  = "ghcr.io/autonomics/mixer:2.2.1"
visualization    = "ghcr.io/autonomics/r45-ggplot2:latest"
magma_gene       = "ghcr.io/autonomics/magma:1.10"
# …
```

Each `NodeFactory::container_spec()` can override this with a spec-supplied
`image` field, falling back to the manifest default.

### 7.3 Reference data mounts

Reference data is **not** baked into images. It lives on the host (or a
shared NFS) and is bind-mounted read-only:

```toml
[reference_mounts]
"/reference/ldsc"       = "/mnt/data/ldsc_ref"
"/reference/mixer"      = "/mnt/data/mixer_ref"
"/reference/1kg"        = "/mnt/data/1000G"
"/reference/grch38"     = "/mnt/data/grch38"
```

These are global engine-level mounts, merged with per-node `read_only_mounts`.

---

## 8. Security Considerations

| Risk | Mitigation |
|------|------------|
| Container breakout | Run with `--user $(id -u):$(id -g)`, `--read-only` rootfs, `--security-opt no-new-privileges`. Network `none` by default. |
| Malicious image | Pin images by digest (`@sha256:...`) in production. Only pull from trusted registries. |
| Resource exhaustion | Per-node CPU/memory limits (`--cpus`, `--memory`). Scheduler's existing semaphore controls concurrency. |
| Host path exposure | Only `staging`, opendal root, and explicitly declared reference mounts are visible. No blanket host mount. |
| Secret leakage | No env vars with credentials passed to containers by default. Iceberg catalog credentials are resolved on the host; containers receive data via files, not tokens. |

---

## 9. Module Layout

```
crates/data-engine/src/
├── container/
│   ├── mod.rs              # Public exports
│   ├── spec.rs             # ContainerSpec, ContainerIo, Mount, DataFormat
│   ├── runtime.rs          # ContainerRuntime trait, RunRequest, RunResult
│   ├── docker.rs           # DockerRuntime (tokio::process::Command-based)
│   ├── node.rs             # ContainerNode (DagNode adapter)
│   ├── io.rs               # DataFrame ↔ file serialization helpers
│   ├── template.rs         # {{placeholder}} resolution
│   ├── config.rs           # ContainerConfig (image manifest, reference mounts)
│   └── error.rs            # ContainerError
├── nodes/
│   └── (existing nodes gain optional container_spec implementations)
└── (rest unchanged)
```

---

## 10. Integration Points

### 10.1 `NodeCtx` (additive)

```rust
pub struct NodeCtx {
    // ... existing fields ...
    pub container_runtime: Option<Arc<dyn ContainerRuntime>>,
    pub container_config: Option<Arc<ContainerConfig>>,
}
```

### 10.2 `NodeFactory` trait (additive)

```rust
fn container_spec(&self, spec: &Value) -> Option<Result<ContainerSpec, ContainerError>> {
    None  // default: native-only
}
```

### 10.3 `DataEngineBuilder`

```rust
impl DataEngineBuilder {
    /// Enable container execution mode with the Docker runtime.
    pub fn with_docker(mut self) -> Self { ... }

    /// Enable container execution with a custom runtime (Podman, etc.).
    pub fn with_container_runtime(mut self, rt: Arc<dyn ContainerRuntime>) -> Self { ... }

    /// Path to the image manifest file.
    pub fn container_image_manifest(mut self, path: &str) -> Self { ... }

    /// Reference data mount mapping.
    pub fn reference_mounts(mut self, mounts: HashMap<String, String>) -> Self { ... }
}
```

### 10.4 `SchedulerConfig` (additive)

```rust
pub struct SchedulerConfig {
    // ... existing fields ...

    /// Default execution mode for nodes that support both.
    pub execution_mode: ExecutionMode,
}

pub enum ExecutionMode {
    Native,
    Container,
    Auto,
}
```

### 10.5 Run report (additive)

```rust
pub struct NodeReport {
    // ... existing fields ...

    /// For container-backed nodes: the image used.
    pub container_image: Option<String>,
    /// For container-backed nodes: captured stdout (truncated).
    pub container_stdout: Option<String>,
}
```

---

## 11. Example: MiXeR Univariate in Container Mode

The current `univariate_mixer` node calls `python mixer.py fit1` on the
host. In container mode, the factory provides a `ContainerSpec` instead:

```rust
impl NodeFactory for UnivariateMixerNodeFactory {
    // ... existing methods ...

    fn container_spec(
        &self,
        spec: &Value,
    ) -> Option<Result<ContainerSpec, ContainerError>> {
        let s: UnivariateMixerNodeSpec = serde_json::from_value(spec.clone()).ok()?;

        Ok(Ok(ContainerSpec {
            image: "ghcr.io/autonomics/mixer:2.2.1".into(),
            command: vec![
                "python".into(),
                "/mixer/precimed/mixer.py".into(),
                "fit1".into(),
                "--bim-file".into(),    "{{spec.bim_file}}".into(),
                "--ld-file".into(),     "{{spec.ld_file}}".into(),
                "--lib".into(),         "/mixer/libbgmg.so".into(),
                "--extract".into(),     "{{spec.extract_file}}".into(),
                "--trait1-file".into(), "{{input_file}}".into(),
                "--chr2use".into(),     "{{spec.chr2use}}".into(),
                "--seed".into(),        "{{spec.seed}}".into(),
                "--out".into(),         "{{staging}}/output/result".into(),
            ],
            workdir: "/staging".into(),
            read_only_mounts: vec![
                Mount {
                    host_path: "/mnt/data/mixer_ref".into(),
                    container_path: "/reference/mixer".into(),
                },
            ],
            env: vec![],
            io: ContainerIo {
                input_format: DataFormat::Tsv,  // mixer expects sumstats TSV
                output_format: DataFormat::Json,
                output_files: vec!["result.json".into()],
            },
            cpus: Some(4.0),
            memory: Some("8g".into()),
            network: "none".into(),
            capture_logs: true,
        })).into()
    }
}
```

The scheduler dispatches this identically to any other node. The agent sees
no difference — it adds a `univariate_mixer` node, connects edges, and runs.
The only observable difference is the `container_image` field in the run
report.

---

## 12. Phased Rollout

### Phase 0 — Foundation (1-2 days)

- [ ] Create `crates/data-engine/src/container/` module with `spec.rs`,
      `error.rs`, `runtime.rs` (trait only).
- [ ] Add `ContainerRuntime` trait + `DockerRuntime` skeleton.
- [ ] Add `ContainerConfig` with image manifest + reference mount parsing.
- [ ] Add `container_runtime` / `container_config` to `NodeCtx`.
- [ ] Add `container_spec()` to `NodeFactory` (default `None`).
- [ ] Unit tests for template resolution + IO serialization.

### Phase 1 — ContainerNode + MiXeR pilot (2-3 days)

- [ ] Implement `ContainerNode` (the `DagNode` adapter).
- [ ] Implement `io.rs`: DataFrame ↔ Parquet/CSV/TSV/JSON serialization.
- [ ] Implement `template.rs`: `{{placeholder}}` resolution.
- [ ] Implement `DockerRuntime::run` via `tokio::process::Command`.
- [ ] Add `container_spec()` to `UnivariateMixerNodeFactory`.
- [ ] Build `ghcr.io/autonomics/mixer:2.2.1` image (Dockerfile + CI).
- [ ] Integration test: run `univariate_mixer` in container mode, compare
      output to the existing subprocess-based result.

### Phase 2 — Viz + codegen convergence (2-3 days)

- [ ] Add `container_spec()` to `VizNodeFactory` (R/ggplot2 image).
- [ ] Refactor the existing subprocess MiXeR code to share the container IO
      infrastructure (staging dir, data serialization).
- [ ] Wire `ExecutionMode` into `SchedulerConfig` and `run_dag`.
- [ ] Per-node `_container: true` spec override.

### Phase 3 — External-tool nodes (3-5 days)

- [ ] New node kinds that are **container-only** (no native `execute`):
  - `magma_gene_container` (or: `magma_gene` with container_spec only)
  - `plink` (PLINK 1.9 / 2.0)
  - `regenie` (REGENIE step 1/2)
  - `metal` (METAL meta-analysis)
  - `gcta_bivariate_ldsr` (GCTA-LDSC)
- [ ] Pre-build images for each, push to registry.
- [ ] Documentation: how to add a new container-backed node kind.

### Phase 4 — Backend abstraction + production hardening (ongoing)

- [ ] `PodmanRuntime` (rootless, daemonless — drop-in for Docker).
- [ ] `ApptainerRuntime` (for HPC environments with Apptainer/Singularity).
- [ ] Image digest pinning in production configs.
- [ ] `docker run` streaming: capture stdout/stderr line-by-line and forward
      to `NodeReporter::log()` for live progress.
- [ ] Resource monitoring (container CPU/memory telemetry).
- [ ] GPU support (`--gpus` for GPU-accelerated tools).

---

## 13. Alternatives Considered

### A. Container-per-DAG (batch all nodes into one container)

Run the entire DAG as a single script inside one container (essentially
auto-codegen + `docker run`).

**Rejected**: Loses the per-node parallelism, incremental execution, and
error isolation that the current scheduler provides. Also makes it hard to
mix container and native nodes in the same DAG.

### B. WebAssembly (WASM) tool isolation

Compile tools to WASM, run via wasmtime.

**Rejected**: The bioinformatics toolchain (R, Python, PLINK, C++ libraries
with OpenMP) is nowhere near WASM-compatible. This would require rewriting
every tool. Containers are the pragmatic choice.

### C. Conda-env-per-node (no containers)

Use conda environments on the host, one per tool.

**Rejected**: Still requires host-installed conda, doesn't solve deployment
reproducibility, env conflicts are the problem we're trying to solve, and
no isolation between nodes.

### D. Nix shells per node

Declarative, reproducible, no daemon.

**Considered for future**: Nix provides excellent reproducibility but has a
steep learning curve and the bioinformatics package coverage in nixpkgs is
sparse. Could be an alternative `ContainerRuntime` backend (`NixRuntime`)
in Phase 4, but Docker is the pragmatic default.

---

## 14. Open Questions

1. **GPU passthrough**: Some tools (REGENIE, SAIGE with GPU) need `--gpus
   all`. How to express this in `ContainerSpec`? Proposed: `gpus:
   Option<GpuRequest>` field.

2. **Shared memory**: Some R/Python tools need `--shm-size` larger than the
   default 64MB. Proposed: `shm_size: Option<String>` field.

3. **Iceberg access from containers**: Should containers have direct Iceberg
   REST catalog access (network = host + credentials), or should the engine
   always pre-materialize Iceberg tables into Parquet files in staging?
   Proposed: start with pre-materialization (no network), add direct access
   later if performance demands it.

4. **Container image caching strategy**: Should `ensure_image` check the
   registry for updates (like `docker pull --quiet` always), or rely on
   local cache and only pull on explicit `image_pull` policy? Proposed:
   `PullPolicy` enum (`Always`, `IfNotPresent`, `Never`), default
   `IfNotPresent`.

5. **Multi-arch support**: ARM (Apple Silicon) vs AMD. Images need
   multi-arch builds. Proposed: CI builds `linux/amd64 + linux/arm64`.
