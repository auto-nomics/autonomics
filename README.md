# autonomics / agentik

[English](README.md) | [中文](README_zh.md)

Autonomics is a complete, self-extending research system for biomedical science built around LLM agents. Two agent roles divide the work: the **Researcher** owns the scientific question — literature, evidence, typed DataFusion DAGs, execution, interpretation, and manuscript writing — while the **Developer** owns capability: when an analysis node does not exist yet, the Researcher records the gap, spawns a Developer child agent, and the Developer drafts a new container plugin (manifest, scripts, and if needed a new OCI image), passes deterministic validation gates, and installs an immutable snapshot into the live node registry. Operational know-how is captured the same way: observations from real runs are distilled into reviewable skills. Epidemiology, statistical genetics, clinical and survey analysis, machine learning, scientific databases, reproducible execution, and now supervised self-improvement are parts of one research system rather than separate applications.

The root Cargo workspace contains 101 crates at the time of this README update. It is an active research codebase: APIs, node contracts, and configuration paths may still change. The `dendrite/` directory is a separate nested Rust workspace for the knowledge-management system.

## System Contract

The system spans five planes plus one protocol:

- **Model cockpit**: a resident gateway daemon, streaming conversations, persistent memory, and a constrained multi-agent topology with `researcher` and `developer` agent kinds.
- **Capability registry**: JSON-schema-validated tools and typed DAG nodes addressed as `plugin/node` (built-ins live under `core/`), ranging over analysis, I/O, source, sink, and writing operations, plus manifest plugins that can be installed while the system runs.
- **Self-improvement substrate**: plugin RSI (recursive self-improvement), skill evolution, and environment (image) development. Agents draft; deterministic gates validate; humans approve; only then does anything reach the running system.
- **Evidence plane**: biomedical APIs, a unified literature gateway, bibliographic citations flowing as typed data on DAG edges, full-text storage, bibliography records, and knowledge-tree access in one runtime.
- **Execution plane**: DataFusion, Arrow, a pluggable task-executor contract (local and remote today; SLURM/Kubernetes-shaped tomorrow), VFS, immutable data packages, Podman, and reference-panel caches.
- **Research protocol**: DAG history, snapshots, diffs, branches, provenance export (RO-Crate / W3C PROV-JSON), and numerical cross-validation make agent work inspectable, replayable, and publishable.

Autonomics is not a general-purpose chat application, a notebook replacement built around free-form scripts, or a monolithic pipeline. Nor is it an autonomous mutator: agents never receive raw git, registry, host-filesystem, or live-registry access. Improvements are proposals that pass gates and review — the model may draft and test; humans and trusted infrastructure promote.

## Demo

![PubMed article retrieval](docs/pubmed.gif)

## System Architecture

![System architecture: thin frontends, gateway daemon, researcher and developer agents, capability registry, self-improvement substrate, execution plane, shared data plane, research protocol](docs/diagrams/architecture.png)

Thin frontends talk to one resident daemon. `autonomics serve` owns the `RuntimeHost` as the single writer of all state: it restores the persisted agent layout at boot, serves REST + SSE on `127.0.0.1:8765` (bearer-token guarded, with `Last-Event-ID` replay), and keeps agents running when frontends exit. The TUI auto-spawns the daemon on first use; `autonomics run` drives the same daemon headlessly.

```text
Ratatui TUI (thin client)     ──REST/SSE──▶  gateway daemon (`autonomics serve`)
Headless CLI (`autonomics run`)                     127.0.0.1:8765 · bearer token
HTTP API + bibliography web          EventHub replay · agents survive frontends
        |                                              |
        └──────────── events ────────────── RuntimeHost (single writer)
  agent tree rooted at /root … restored at daemon boot
        |
        ├── Researcher agents (default kind)
        |     science, DAG design & execution, interpretation,
        |     literature, writing, memory
        |     spawn_agent + delegate_to  ──────────┐
        |                                          | contracts + synthetic
        └── Developer agents (children)  ◀─────────┘ fixtures only — never
              plugin/node implementation,            research datasets
              environment selection, focused
              container validation, install
        |
        | ToolFunction calls (delegation is the only
        | inter-agent channel: parent → direct child)
        v
Data-engine actor (one per session)
  node discovery, add/update nodes (`plugin/node`), edges,
  fire-and-forget runs, retained outputs, history
        |
        v
Node registry + scheduler
  builtin bundles under `core/` + fail-closed manifest plugins
  typed input and output ports, JSON-schema validated specs
  pluggable executors: local / process / remote
        |
        +--> DataFusion DataFrame and Arrow RecordBatch nodes
        |
        +--> Evidence channel: File(evidence) edges carry
        |    canonical citations between literature and DAG
        |
        +--> OCI container nodes
              ephemeral Podman runs on digest-pinned images
              content-addressed work dirs (rerun-safe)
              read-only mounts, immutable panel bundles
              artifact publication to VFS

Self-improvement substrate (agents draft · gates validate · humans promote):
  skills     evo_observe → deterministic distill → proposal → human approve
  plugin RSI requests → dev workspaces → validation gates → install/publish
  environments  /environments/dev image authoring → build → smoke → catalog

Shared data plane:
  VFS (OpenDAL) -> local and optional S3/OSS mounts
  data catalog  -> immutable, versioned Hugging Face packages
  biofusion     -> VCF/BCF/FASTA/FASTQ/BED/GTF/GFF/SAM/BAM/CRAM/BigWig/BigBed readers
```

## Researcher and Developer Agents

Agents form a tree rooted at `/root`. Every agent has exactly one kind:

- **Researcher** (default) — the biomedical research assistant. It owns the scientific question, analysis design, DAG construction, execution, and interpretation, with the bibliography, writing, and scientific-API capabilities. The system prompt tells it that the plugin and node ecosystem is *dynamic*: an absent or imperfect node is a normal discovery, not a dead end.
- **Developer** — the node and plugin developer. It builds, validates, installs, and uninstalls plugins through the host-owned lifecycle, selects environments, and runs focused container validation. It explicitly rejects research or data-analysis requests: the handoff carries interface contracts, schemas, failure cases, and small synthetic fixtures — never production or research datasets.

Orchestration is deliberately constrained. Communication is delegation-only and strictly parent → direct child; an agent sees only itself, its parent, and its children. Delegations are first-class records with statuses (pending/running/completed/interrupted/failed) persisted across daemon restarts. The nine host tools are `spawn_agent`, `delegate_to`, `route_task`, `get_agent_info`, `list_agents`, `list_delegations`, `get_agent_history`, `shutdown_agent`, and `interrupt_agent`.

When a Developer delivers, the handoff includes the plugin name, the full `plugin/node` address, node documentation, spec schema, port layout, installation status, a minimal DAG usage example, and validation evidence. `plugin_install` activates an immutable snapshot in the live registry via `reload_plugin`, so the Researcher can use the new node in the same session.

## Self-Improvement

Autonomics improves itself along three supervised loops that share one safety model: agents propose, deterministic infrastructure validates, humans approve.

### Skills

A skill is a directory with a `SKILL.md` (YAML frontmatter + operational body), following the same contract as the Anthropic `skills` ecosystem, so third-party packs install unchanged. Skills live in three tiers — `builtin` (compiled in), `global` (`~/.autonomics/skills`), and `workspace` — and every agent's system prompt carries a one-line index of the library; bodies are fetched on demand through `skill_search` / `skill_get`. Skills may bundle parameterized DAG workflow templates and eval cases.

The evolution loop has no auxiliary LLM anywhere:

1. **Observe** — `evo_observe` records durable failures, recipes, and caveats as content-addressed observations (duplicates are no-ops). Eval failures are captured automatically.
2. **Route** — each observation is routed to the skill domain, the plugin domain, both, or triage, anchored on the DAG node address.
3. **Distill** — pure code clusters anchored observations by node kind and normalized error signature; three occurrences of the same pattern synthesize a conservative skill proposal that only ever cites recorded fixes, never invents advice.
4. **Review** — proposals stage in `~/.autonomics/skill-proposals/` and are approved or rejected from the TUI skill-evolution dashboard or the gateway API. Auto-approval is off by default, and agent-authored proposals are never auto-approved.
5. **Measure** — tool-usage telemetry is the fitness signal, persisted across restarts.

### Plugin RSI

`plugin-rsi` is the plugin-based recursive self-improvement substrate. Structured requests (from users, agents, workflow runs, eval failures, or observations) flow into content-addressed request records; Developer agents work in git-managed dev workspaces mounted at `/plugins/dev/<name>`; validation gates check the manifest, environment policy (approved digest-pinned image, allowed interpreter, no plugin-owned Dockerfile, read-only rootfs, isolated network), node-address collisions, script statics, registry compilation, and secrets — all before anything installs. `plugin_install` activates an immutable local snapshot; a background distiller later publishes reviewed work to GitHub (pull-request flow for updates) without blocking the agent. Uninstall retains the workspace and history for audit.

### Environments (image development)

Every plugin runs against an **environment**: a host-approved, digest-pinned base image plus its interpreters, held in a catalog seeded with 39 references — base OS images (alpine, debian, ubuntu, python, rocker-verse, bioconductor), the bioinformatics toolchain (samtools, bcftools, bwa, minimap2, fastqc, salmon, gatk4, ensembl-vep, bedtools, macs2, blast, mafft, iqtree, kraken2, …) and medical imaging (orthanc, ohif-viewer, monai). Users approve additional images through the gateway (Docker Hub search + approve). New environments are authored as git workspaces at `/environments/dev/<id>` with a `manifest.toml` and a `Containerfile`; six validation gates (four static, then build and smoke test) must pass before local activation pins the built digest into the catalog, after which a background distiller can publish it to `ghcr.io/auto-nomics/environments`. Building and pushing are host-owned — agents never touch Podman or a registry directly.

## DAG Dispatch and Container Execution

Two mechanisms make the system trustworthy for biomedical work:

- **DAG-based dispatch.** A research request is parsed by the Researcher, planned as a typed DAG of JSON-Schema-validated nodes, and assembled through one registry. The same DAG can mix fast in-process DataFusion / Arrow transforms with heavy external containers; the scheduler dispatches asynchronously through a pluggable executor contract (`local`, `process`, and a coordinator-side `remote` executor with an artifact store, so batch schedulers can slot in later) and retains outputs for snapshot, diff, and branch operations.
- **Containerized nodes.** Every external tool runs in its own ephemeral Podman container. Images are pinned by sha256 digest, reference panels are checksum-verified from `manifest.json` and mounted read-only, the rootfs is `--read-only` with `no-new-privileges` and explicit CPU / PID / shm / UID caps, and declared outputs are streamed to VFS as `FileRef = size + SHA-256` using a pending-object + atomic rename so consumers never observe partial artifacts. Container work directories are content-addressed: an identical re-run reuses the previous workspace (Nextflow-style resume), inputs stage in as read-only bind mounts by default, and a background sweeper garbage-collects stale workspaces.

## Capability Map

| Area | Main crates | What it provides |
| --- | --- | --- |
| Model orchestration | `agentik-sdk`, `agentik-types`, `agentik-proc`, `agentik-core`, `agentik-network`, `runtime` | Streaming LLM clients, tool schemas and calls, persistent memory, agent kinds and profiles, delegation-only multi-agent topology, and a sync-to-async host. |
| Analysis execution | `dag-core`, `data-engine`, `data-engine-tools`, `crates/node-bundles/*` | Node traits, plugin registry, typed ports, scheduler with pluggable executors, JSON-schema specs, and agent tools. |
| Self-improvement | `plugin-rsi`, `container-plugin`, `skills`, `evolution-core` | Plugin RSI lifecycle and tooling, manifest compile/load, skill library and no-LLM evolution loop, shared observation store and routing. |
| Data infrastructure | `vfs`, `data-catalog`, `container-runtime`, `biofusion` | OpenDAL-backed VFS, Hugging Face-hosted versioned packages, Podman execution, immutable panel caches, and biological-format DataFusion readers. |
| Statistics and epidemiology | `statkit`, `epi`, `hypothesize`, `nodes-power`, `cmprsk`, `crrkit`, `survey`, `mice`, `hierint` | Descriptive statistics and regression; causal inference and mediation; composable tests and p-value workflows; prospective power and sample-size design; competing risks; survey designs; imputation; hierarchical interaction models. |
| Machine learning and deep learning | `ml`, `dl` | Preprocessing, feature engineering, clustering, supervised models, ensembles, anomaly detection, dimensionality reduction; Burn-based MLP, DeepSurv, DeepHit, RNN, Transformer, and autoencoder workflows. Generalized random forests ship as the containerized `grf` plugin family (official R grf). |
| Statistical genetics | `ldsc`, `mr`, `lava`, `mrlap`, `lcv`, `cpassoc`, `magma`, `coloc`, `bkmr`, `evalue`, `genomic_sem`, `lcmm` | LD score regression, Mendelian randomization, local genetic correlation, colocalization, Bayesian kernel-machine regression, E-value analysis, Genomic SEM, latent-class mixed models, and related ports. |
| Regression discontinuity | `rdrobust`, `rdpower`, `rdmulti`, `rddensity`, `rdlocrand` | Local-polynomial RD estimation, power and sample-size calculations, multi-cutoff designs, manipulation testing, and local randomization inference. |
| Scientific data clients | `eutils`, `opengwas`, `gwascatalog-sdk`, `opentargets`, `chembl`, `uniprot`, `string-sdk`, `enrichr-sdk`, `kegg`, `reactome`, `ensembl`, `rcsb`, `alphafold`, `interpro`, `pubchem`, `protocolio`, `clinicaltrials`, `nhanes` | SDKs, agent tools, and selected DAG source nodes for PubMed/Entrez, OpenGWAS, GWAS Catalog, Open Targets, ChEMBL, UniProt, STRING, Enrichr, KEGG, Reactome, Ensembl, RCSB, AlphaFold, InterPro, PubChem, protocols.io, ClinicalTrials.gov, and NHANES. |
| Literature, writing, and knowledge | `arxiv`, `biorxiv`, `openalex`, `crossref`, `embase`, `europepmc`, `semantic-scholar`, `bib-types`, `bib-base`, `writing-types`, `writing-base`, `kms`, `kms-tools` | Unified literature search and full-text management, the evidence channel on DAG edges, content-addressed documents, BibTeX/RIS/Markdown/CSL export, LaTeX AST operations, citation resolution, compilation, and knowledge-tree tools. |
| Interfaces | `apps/autonomics`, `gateway`, `headless`, `api-server`, `ascii-dag-core` | Resident gateway daemon, streaming terminal chat and DAG view, headless CLI execution, bibliography CLI/API/frontend, and the TUI DAG renderer. |

The default `data-engine` build enables all node-bundle Cargo features. A library consumer can disable default features and select only needed `bundle-*` features.

## Plugin Families

All containerized analysis nodes ship as **manifest plugins**: one directory
per family holding `manifest.toml` (the node contract: params, ports,
panels, image provenance), execution scripts, and the image build tree.
Plugins are installed from git repositories pinned to a commit SHA and
loaded at daemon startup by the plugin preflight — see
[Container Plugin Authoring](docs/plugins/README.md) for the format and
workflow; use the [migration workflow](docs/plugin-node-migration.md) when
replacing an existing hardcoded wrapper. Panel data bundles are provisioned by
`autonomics panels sync`, which downloads and checksum-verifies every
`[[panels]]` dataset reference missing from the local catalog cache; startup preflight
only checks presence locally (bounded, offline-safe), so daemon readiness
never waits on the network. Set `AUTONOMICS_PANEL_SYNC=1` to run the
provisioning inline during `autonomics serve` for unattended deployments.

28 curated families / 99 node kinds are currently published, addressed in the
registry as `family/node_kind` (built-ins use the `core/` namespace):

| Family | Node kinds | Tool |
| --- | --- | --- |
| [ldsc](https://github.com/auto-nomics/ldsc-plugin) | `ldsc_h2`, `ldsc_munge`, `ldsc_rg` | LD score regression (h2 / munge / rg) |
| [magma](https://github.com/auto-nomics/magma-plugin) | `magma_annotate` | MAGMA SNP-to-gene annotation |
| [mrpresso](https://github.com/auto-nomics/mrpresso-plugin) | `mrpresso` | MR-PRESSO heterogeneity / outlier test |
| [mvmr](https://github.com/auto-nomics/mvmr-plugin) | `mvmr` | multivariable Mendelian randomization |
| [coloc](https://github.com/auto-nomics/coloc-plugin) | `coloc_abf` | colocalization (coloc.abf) |
| [deseq2](https://github.com/auto-nomics/deseq2-plugin) | `deseq2_de` | differential expression (DESeq2) |
| [gcta](https://github.com/auto-nomics/gcta-plugin) | `gcta_cojo_select`, `gcta_sblup`, `gcta_fastbat`, `gcta_acat` | GCTA summary-statistics suite |
| [pathway-gsea](https://github.com/auto-nomics/pathway-gsea-plugin) | `pathway_gsea` | fgsea pathway enrichment |
| [clusterprofiler](https://github.com/auto-nomics/clusterprofiler-plugin) | `clusterprofiler_ora`, `clusterprofiler_gsea` | ORA and GSEA enrichment (clusterProfiler) |
| [plink2](https://github.com/auto-nomics/plink2-plugin) | `plink2_clump` | LD clumping (PLINK2) |
| [visualization](https://github.com/auto-nomics/visualization-plugin) | `visualization` | R plot rendering from user scripts |
| [mtag](https://github.com/auto-nomics/mtag-plugin) | `mtag` | multi-trait analysis of GWAS |
| [smr](https://github.com/auto-nomics/smr-plugin) | `smr_heidi`, `smr_heidi_eqtlgen` | SMR & HEIDI (Westra / eQTLGen) |
| [susie](https://github.com/auto-nomics/susie-plugin) | `susie_rss` | SuSiE fine-mapping (RSS) |
| [twosamplemr](https://github.com/auto-nomics/twosamplemr-plugin) | `twosamplemr`, `twosamplemr_harmonise` | TwoSampleMR + harmonisation |
| [mixer](https://github.com/auto-nomics/mixer-plugin) | `mixer_fit1`, `mixer_fit2` | gsa-mixer fit1 / fit2 |
| [music](https://github.com/auto-nomics/music-plugin) | `music_deconvolution` | MuSiC cell-type deconvolution |
| [mutation](https://github.com/auto-nomics/mutation-plugin) | `mutation_analysis`, `mutation_analysis_clinical` | maftools mutation landscapes |
| [timesfm](https://github.com/auto-nomics/timesfm-plugin) | `timesfm_forecast` | TimesFM forecasting (offline checkpoint) |
| [twas](https://github.com/auto-nomics/twas-plugin) | `twas_fusion` | FUSION TWAS gene expression |
| [hdl](https://github.com/auto-nomics/hdl-plugin) | `hdl_l`, `hdl_l_scan` | HDL-L heritability + chromosome scan |
| [lava](https://github.com/auto-nomics/lava-plugin) | `lava`, `lava_scan` | local genetic correlation + scan |
| [single-cell](https://github.com/auto-nomics/single-cell-plugin) | `single_cell_preprocessor`, 10 `h5ad_*` / `gene_set_score` / `sc_dense_ingest` variants | scRNA preprocessing and H5AD analyses |
| [radiomics](https://github.com/auto-nomics/radiomics-plugin) | 21 `radiomics_*` / `pyradiomics_*` variants | imaging feature extraction pipeline |
| [pathology](https://github.com/auto-nomics/pathology-plugin) | 7 `pathology_*` variants | WSI ingest / QC / embedding / IHC |
| [bulk-rnaseq](https://github.com/auto-nomics/bulk-rnaseq-plugin) | `limma_voom`, `wgcna` | limma+voom differential expression, WGCNA modules |
| [hyprcoloc](https://github.com/auto-nomics/hyprcoloc-plugin) | `hyprcoloc` | HyPrColoc multi-trait colocalization |
| [grf](https://github.com/auto-nomics/grf-plugin) | 23 `grf_*` kinds: 12 forest trainers, `grf_predict_forest`, ATE / best-linear-projection / calibration / scores, forest weights / split frequencies / variable importance / get-tree / merge, `grf_generate_causal_data` | generalized random forests (official R grf 2.6.1) |

The table is the curated starting point, not a ceiling: the developer loop
described above has already produced agent-authored families such as
`phylo-treeness`, `phylo-pis`, `donor-paired-composition`, `python-script`,
and `h5ad-obs`, published through the same channels.

### Building plugins

- [Plugin authoring overview](docs/plugins/README.md): lifecycle, core rules, and the document map.
- [Plugin authoring guide](docs/plugins/authoring-guide.md): end-to-end tutorial using a `clusterProfiler` ORA example.
- [Manifest reference](docs/plugins/manifest-reference.md): normative schema, template semantics, and startup validation.
- [Testing and release checklist](docs/plugins/testing-and-release.md): test pyramid, image digest publication, Git pinning, and clean-room review.
- [Plugin-based RSI design](docs/design/plugin-based-rsi.md): requests, proposals, validation gates, and the trusted-publication model behind the developer workflow.

### Installing plugins

Declare families in `~/.autonomics/plugins.toml`; `autonomics serve`
runs a preflight before the daemon starts that installs each family at
its pinned revision (git clone or local `path` symlink), validates every
manifest fail-closed, and reports the registered kinds:

```toml
[[plugin]]
name = "ldsc"
git = "https://github.com/auto-nomics/ldsc-plugin.git"
rev = "6f7118d61dd60ca7ce95d7d524ccec3880d96026"   # pinned commit SHA
```

Local development uses a `path` source (symlinked — edits apply on the
next restart without reinstalling):

```toml
[[plugin]]
name = "mtag"
path = "/mnt/projects/node-plugins/mtag"
```

## Workspace Layout

```text
autonomics/
├── apps/autonomics/                     Terminal application and CLI subcommands
├── crates/
│   ├── agentik-*/                LLM SDK, types, proc macros, runtime, networking
│   ├── dag-core/                 DAG traits, registry, scheduler, executor contracts
│   ├── data-engine/              DataFusion engine and node-bundle wiring
│   ├── data-engine-tools/        Agent ToolFunction adapters for DAG operations
│   ├── node-bundles/             Feature-gated analysis-node plugins
│   ├── plugin-rsi/               Recursive self-improvement substrate (plugins)
│   ├── container-plugin/         Manifest compile, load, sync, factories
│   ├── skills/, evolution-core/  Skill library and no-LLM evolution loop
│   ├── vfs/                      OpenDAL-backed virtual file system and vbash tools
│   ├── data-catalog/             Versioned data-package catalog
│   ├── container-runtime/        Podman execution and immutable panel cache
│   ├── gateway/, headless/       Resident daemon and headless CLI execution
│   ├── biofusion*/               Biological-format DataFusion readers and cache
│   ├── runtime/                  RuntimeHost and shared agent infrastructure
│   ├── bib-*/writing-*/kms*      Literature, manuscript, and knowledge systems
│   └── ...                       Scientific API SDKs and supporting crates
├── bio_crates/                   Genetics, causal-forest, RD, and related methods
├── stat_crates/                  Statistics, epidemiology, ML, imputation, and DL
├── fixtures/                     Representative valid and malformed test inputs
├── docs/                         Design documents and topic guides
├── infra/                        Local reference-data preparation utilities
├── dendrite/                     Separate nested knowledge-management workspace
└── scripts/                      Install, panel-build, and maintenance scripts
```

The `reference/` directory contains third-party and comparison material and is not part of the root Cargo build. Per-user state lives under `~/.autonomics`: `plugins.toml` plus the v2 plugin tree (`plugins/dev`, `plugins/snapshots`, `plugins/runtime`), the environment catalog (`plugin-environments.toml`, `environments/dev`), the skill library and its evolution stores (`skills/`, `skill-observations/`, `skill-proposals/`, `skill-usage.toml`), panel caches (`panels/`), and the SQLite databases (`agent.db`, `bib.db`, `writing.db`, `dag-history.db`).

## Getting Started

### Install a prebuilt binary

Install the platform binary without rebuilding:

```bash
curl --fail --location https://raw.githubusercontent.com/auto-nomics/autonomics/main/scripts/install.sh | bash
```

The script downloads the matching Linux or macOS binary, verifies `SHA256SUMS`, and installs it to `~/.local/bin`. Override the destination with `AUTONOMICS_INSTALL_DIR`, pin a release with `AUTONOMICS_VERSION=v0.1.0`, or use `AUTONOMICS_REPO=owner/repo` for a fork.

### Build and run

Requirements:

- Rust 1.85 or newer; the checked-in `Cargo.lock` is used for reproducible builds.
- A C/C++ compiler for the vendored GRF core.
- Podman when using OCI-backed nodes, panel bundles, radiomics, visualization, or TimesFM.
- Optional: XeLaTeX for manuscript compilation, Bun for the TUI HTTP frontend, and R for selected cross-validation scripts.

```bash
git clone --recurse-submodules <repository-url>
cd autonomics
cargo run -p autonomics
```

The first full workspace build is large. If the default target directory is unsuitable, set `CARGO_TARGET_DIR=/path/to/target`.

The binary is `autonomics`; the TUI is the default when no subcommand is given:

```bash
autonomics tui                     # interactive terminal frontend (default)
autonomics serve --daemon          # start the detached gateway daemon
autonomics serve status | stop     # inspect / gracefully stop the daemon
autonomics run "..." --json        # one headless prompt through the daemon
autonomics export-run <run_id> --format crate --out <dir>   # RO-Crate / PROV-JSON provenance
autonomics panels sync             # provision plugin panel data bundles
autonomics kms                     # knowledge-management TUI
autonomics cache refresh-opengwas
autonomics bib list
```

With Cargo, pass the subcommand after `--`; for example:

```bash
cargo run -p autonomics -- run "Summarize recent GWAS meta-analyses of BMI." --json
```

### Configure the runtime

Model providers are configured in the TUI (Select model) or through the gateway's model-config API and stored in the application database. Built-in provider presets include Aliyun Bailian, DeepSeek, MiMo, MiniMax, Moonshot, OpenAI/ChatGPT, OpenRouter, SenseNova, StepFun, and Z.ai, and custom Anthropic-compatible endpoints are supported. Models are selected per agent as `provider:model`.

Scientific API credentials must be present in the process environment. The repository includes a direnv wrapper that exports `.env`; otherwise, export the variables in your shell or service definition. Use [`.env.example`](.env.example) as the template, and provide only the credentials for services you use. Common examples include:

- `OPENGWAS_TOKEN`
- `OPENALEX_API_KEY`
- `EUTILS_API_KEY`
- `PROTOCOLS_IO_ACCESS_TOKEN`
- `EMBASE_API_KEY`, `EMBASE_INSTTOKEN`, and related Embase tokens
- `UMLS_API_KEY`
- `MINERU_API_KEY` for PDF full-text extraction
- `HUGGING_FACE_TOKEN` or `HF_TOKEN` for the data catalog; S3/OSS credentials are needed only for optional non-catalog VFS backends

By default, state is stored under `~/.autonomics`, downloaded files under `~/.autonomics/data`, and VFS mounts are read from `$AUTONOMICS_STATE_DIR/vfs.toml`. Use `AUTONOMICS_STATE_DIR`, `AUTONOMICS_DATA_DIR`, `AUTONOMICS_BIB_DB`, and `AUTONOMICS_WRITING_DB` to relocate state.

### Use the local HTTP API

The gateway daemon serves the bibliography frontend, a Swagger UI, and the `/api/v1` API on `127.0.0.1:8765` by default. Override the address with `AUTONOMICS_HTTP_API_ADDR`. If the server is exposed beyond loopback, set `AUTONOMICS_HTTP_API_TOKEN` to require bearer authentication for API routes.

```bash
curl http://127.0.0.1:8765/api/health
curl 'http://127.0.0.1:8765/api/v1/bib/articles?query=gwas&limit=10'
```

The API also exposes live agent control (`/api/v1/agents…`), the SSE event stream (`/api/v1/events`), plugin and environment management (`/api/v1/plugins…`), and the skill library and evolution status (`/api/v1/skills…`). The frontend development workflow is documented in [docs/api-server_zh.md](docs/api-server_zh.md).

## Analysis Model

The system exposes analysis through registered node addresses (`plugin/node`, or `core/node` for built-ins) and JSON specs. Specs are validated against each factory's JSON Schema and instantiated through the plugin registry.

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
- create refs, inspect history, show snapshots, diff snapshots, and branch

The exact catalog is runtime-dependent because node bundles are Cargo features and manifest plugins are installed per deployment. In a running agent, use `list_node_factories`; in Rust, use `DataEngine::list_nodes()`.

## Reproducible Data and Containers

External analysis runtimes are isolated through `container-runtime` and Podman. A container run has a declared image digest, argv command, inputs, output contracts, timeout, resource limits, network policy, and artifact prefix. Reference data is resolved from immutable catalog packages, checksum-verified in a panel cache, and mounted into an ephemeral workspace. Images and data packages are versioned independently, and every published tool image is pinned by immutable manifest digest under `ghcr.io/auto-nomics/autonomics` — see [GHCR container images](docs/ghcr-migration.md).

Container execution requires a Podman runtime reachable on the same host as the VFS workspace and panel cache. The default TUI container image does not mount a Podman socket, so OCI-backed nodes are intended for a host-run TUI or an explicitly configured remote runtime.

## Development

```bash
# Compile all test targets without running tests.
cargo test --workspace --no-run

# Focused test loops.
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

# Formatting and lint.
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Some tests call live public APIs, require credentials or private images, download large panels, or invoke Podman. Many external-resource tests are marked `#[ignore]`; run them explicitly with `cargo test -- --ignored` only when the dependency is available. Plugin behavior is covered by golden migration tests in `crates/container-plugin/tests/` (one suite per family), and the environment-development lifecycle by `crates/plugin-rsi/tests/env_dev.rs`.

The TUI HTTP frontend uses Bun:

```bash
cd crates/api-server/frontend
bun install
bun test
bun run typecheck
bun run build
```

## Documentation

Many topic guides have Chinese editions (`*_zh.md`) alongside the English originals.

### Platform and infrastructure

- [SDK guide](docs/sdk.md): messages, streaming, provider adapters, tools, files, batches, and usage.
- [Agent runtime](docs/agent-runtime.md): agent loop, context, memory, lifecycle, and orchestration.
- [Tool authoring](docs/tool-authoring.md): typed `ToolFunction` inputs and generated schemas.
- [Gateway architecture](docs/design/gateway-architecture.md): the resident daemon, wire protocol, and multi-frontend design (Chinese).
- [Headless run design](docs/headless-run-design.md): `autonomics run`, the `RunEvent` contract, and exit codes (Chinese).
- [VFS design](docs/vfs.md): mounts, concurrent writes, catalog overlays, and reference data.
- [Data catalog](docs/data-catalog.md): package layout, publication, and panel references.
- [Runtime bundles](docs/data-bundles.md): built-in bundle identifiers and runtime overlays.
- [Evidence channel](docs/evidence-channel.md): citations as typed data on DAG file edges.
- [Container execution](docs/container-execution-design.md): Podman contracts and lifecycle.
- [Container migration workflow](docs/container-node-migration.md): image, data package, and wrapper acceptance criteria.
- [Container plugin authoring](docs/plugins/README.md): independent plugin repositories, manifest contracts, testing, and immutable release.
- [Plugin-based RSI design](docs/design/plugin-based-rsi.md): supervised plugin evolution from feedback to trusted publication.
- [GHCR container images](docs/ghcr-migration.md): registry namespace and digest pinning.

### Analysis methods

- [Statistical and epidemiological nodes](docs/sta_epi_nodes.md)
- [Hypothesis-testing design](docs/hypothesize-design.md)
- [LD Score Regression](docs/stat-genetics/ldsc.md)
- [TwoSampleMR](docs/stat-genetics/mr.md)
- [LAVA](docs/stat-genetics/lava.md)
- [MiXeR](docs/stat-genetics/mixer.md)
- [SuSiE-RSS](docs/stat-genetics/susie-rss.md)
- [TWAS/FUSION](docs/stat-genetics/twas-fusion.md)
- [Radiomics Stage-A nodes](docs/radiomics_nodes.md)
- [Visualization container](docs/visualization.md)

### Research workflow

- [TUI guide](docs/tui.md)
- [API Server (HTTP API)](docs/api-server_zh.md) (Chinese)
- [Writing-system design](docs/writing-system-design.md)
- [Dendrite knowledge-management workspace](dendrite/README.md)
- [TimesFM service](../../node-plugins/timesfm/README.md) (timesfm plugin checkout)
- [Local infrastructure](infra/README.md)

## Scope and Boundaries

- Biofusion currently implements read paths, not writers for biological formats.
- External API behavior, rate limits, credentials, dataset availability, and service terms remain the responsibility of the caller.
- Container-backed analyses require access to the corresponding image and data package. A missing private image or panel is an environment prerequisite, not a fallback to an unverified local installation.
- Self-improvement is supervised by design: deterministic gates and human review stand between every proposal and the running system, and agents never hold git, registry, or host-filesystem credentials.
- KEGG is available for academic use; nonacademic use requires an appropriate KEGG license.
- TimesFM checkpoints have checkpoint-specific licenses. Review the container documentation before using a checkpoint in production or commercial work.
- Numerical ports are validated against R, Python, or original implementations where practical, but this system is research software and is not a certified clinical or regulatory decision system.

## License

The workspace metadata declares the MIT license for Autonomics packages that inherit it; no standalone top-level license file is currently checked in. Vendored and containerized third-party software retains its upstream license. GPL-licensed tooling (e.g. the grf R package) runs only inside digest-pinned container plugins, isolated at the image boundary like every other tool family; nothing GPL is compiled into or linked with the workspace binaries. Dataset and model checkpoints carry their own terms.
