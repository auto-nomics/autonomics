//! Configuration for [`RuntimeHost`](crate::RuntimeHost).
//!
//! Every hard-coded path, env-var override, and prompt string that was
//! previously scattered across `runtime.rs` / `tools.rs` / `app.rs` is
//! collected here so that multiple independent agent runtimes — each with
//! its own storage root, history DB, bibliography, prompts, etc. — can be
//! instantiated side-by-side in a future multi-agent system.
//!
//! ## Construction
//!
//! ```ignore
//! use runtime::config::RuntimeConfig;
//!
//! // Defaults — same behaviour as before.
//! let cfg = RuntimeConfig::default();
//!
//! // Customised for a specific agent.
//! let cfg = RuntimeConfig::builder()
//!     .name("literature-agent")
//!     .data_dir("/data/agents/lit")
//!     .opengwas_token_env("OPENGWAS_TOKEN_LIT")
//!     .build();
//! ```
//!
//! ## Precedence
//!
//! For every field the resolution order is:
//! 1. Explicitly set on the builder (`Builder::build()` copies it verbatim).
//! 2. Environment variable named on the field's doc comment.
//! 3. Hard-coded default.
//!
//! `RuntimeConfig::resolve()` performs step 2→3; the builder performs
//! step 1 by letting the caller override the *entire* resolved config.

use std::path::PathBuf;

use bib_base::BibHttpOptions;

// serde derives are used on RuntimeConfig / RuntimeConfigBuilder for agent
// persistence — they are serialised into the `agents` registry table.

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Default file-storage root — files downloaded by agent tools (OpenGWAS
/// summary stats, GWAS Catalog data, etc.) land here.
///
/// Override via builder `.data_dir(…)` or env `AUTONOMICS_DATA_DIR`.
/// Defaults to `$HOME/.autonomics/data` (absolute, independent of CWD).
const DEFAULT_DATA_DIR: &str = ".autonomics/data";

/// Default directory for agent-internal databases (DAG history).
///
/// Override via builder `.state_dir(…)` or env `AUTONOMICS_STATE_DIR`.
/// Defaults to `~/.autonomics` (absolute, independent of CWD).
const DEFAULT_STATE_DIR: &str = ".autonomics";

/// Default DAG history SQLite filename (relative to `state_dir`).
const DEFAULT_DAG_HISTORY_DB: &str = "dag-history.db";

/// Default agent persistence database filename (relative to `state_dir`).
const DEFAULT_AGENT_DB: &str = "agent.db";

/// Default bibliography database path.
///
/// Override via builder `.bib_db_path(…)` or env `AUTONOMICS_BIB_DB`.
const DEFAULT_BIB_DB: &str = "bib.db";

/// Default writing-system database path.
///
/// Override via builder `.writing_db_path(…)` or env `AUTONOMICS_WRITING_DB`.
const DEFAULT_WRITING_DB: &str = "writing.db";

/// Default TUI application database (model config, settings, etc.).
///
/// Override via builder `.app_db_path(…)` or env `AUTONOMICS_APP_DB`.
const DEFAULT_APP_DB: &str = "config.db";

/// Default agent identity string.
const DEFAULT_AGENT_IDENTITY: &str = "You are a biomedical research assistant \
     specializing in genomics, GWAS analysis, and literature mining.";

// ---------------------------------------------------------------------------
// Environment variable names
// ---------------------------------------------------------------------------

/// Env var overriding the file-storage root.
pub const ENV_DATA_DIR: &str = "AUTONOMICS_DATA_DIR";

/// Env var overriding the internal-state directory.
pub const ENV_STATE_DIR: &str = "AUTONOMICS_STATE_DIR";

/// Env var overriding the bibliography DB path.
pub const ENV_BIB_DB: &str = "AUTONOMICS_BIB_DB";

/// Env var overriding the writing-system DB path.
pub const ENV_WRITING_DB: &str = "AUTONOMICS_WRITING_DB";

/// Env var overriding the TUI application DB path.
pub const ENV_APP_DB: &str = "AUTONOMICS_APP_DB";

/// Env var supplying the OpenGWAS API token.
pub const ENV_OPENGWAS_TOKEN: &str = "OPENGWAS_TOKEN";

/// Env var overriding the OpenGWAS on-disk cache directory.
pub const ENV_OPENGWAS_CACHE_DIR: &str = "OPENGWAS_CACHE_DIR";

/// Env var overriding the bibliography HTTP client's `User-Agent`
/// header.
pub const ENV_HTTP_USER_AGENT: &str = "AUTONOMICS_HTTP_USER_AGENT";

/// Env var overriding the bibliography HTTP client's connect timeout
/// (whole seconds).
pub const ENV_HTTP_CONNECT_TIMEOUT_SECS: &str = "AUTONOMICS_HTTP_CONNECT_TIMEOUT_SECS";

/// Env var overriding the bibliography HTTP client's per-request
/// timeout (whole seconds).
pub const ENV_HTTP_REQUEST_TIMEOUT_SECS: &str = "AUTONOMICS_HTTP_REQUEST_TIMEOUT_SECS";

/// Env var pointing the bibliography HTTP client at an HTTP or SOCKS5
/// proxy (e.g. `http://proxy.corp:3128`).
pub const ENV_HTTP_PROXY: &str = "AUTONOMICS_HTTP_PROXY";

/// Env var enabling or disabling memory read/injection.
pub const ENV_USE_MEMORY: &str = "AUTONOMICS_USE_MEMORY";

/// Env var enabling or disabling startup memory generation.
pub const ENV_GENERATE_MEMORY: &str = "AUTONOMICS_GENERATE_MEMORY";

// ---------------------------------------------------------------------------
// RuntimeConfig
// ---------------------------------------------------------------------------

/// Fully resolved configuration for a single agent runtime.
///
/// Built via [`RuntimeConfig::builder()`] (or [`RuntimeConfig::default()`]
/// for the current hard-coded defaults) and consumed by
/// [`RuntimeHost::open`](crate::RuntimeHost::open).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RuntimeConfig {
    /// Human-readable name for this runtime instance (useful in multi-agent
    /// setups and log messages).
    pub name: String,

    // ── Storage ───────────────────────────────────────────────────────
    /// Root directory for the virtual file system exposed to agent tools
    /// (downloads, scratch files, artifacts).
    pub data_dir: PathBuf,

    /// Directory for agent-internal state (DAG history DB, etc.).
    pub state_dir: PathBuf,

    /// Path to the DAG history SQLite database.
    pub dag_history_db: PathBuf,

    /// Path to the bibliography SQLite database.
    pub bib_db_path: PathBuf,

    /// Path to the writing-system SQLite database.
    pub writing_db_path: PathBuf,

    /// Path to the TUI / application SQLite database (model config, settings).
    pub app_db_path: PathBuf,

    /// Path to the agent persistence database (Turso/SQLite). Stores agent
    /// registry, session logs (WAL), and memory snapshots for cross-process
    /// recovery.
    pub agent_db: PathBuf,
    // ── External service credentials ──────────────────────────────────
    /// OpenGWAS API token. If `None`, the runtime attempts to read it from
    /// the [`ENV_OPENGWAS_TOKEN`] env var at construction time.
    pub opengwas_token: Option<String>,

    /// OpenGWAS on-disk cache directory override. If `None`, the opengwas
    /// crate's own resolution logic is used (env → `$HOME/.cache/opengwas`
    /// → temp dir).
    pub opengwas_cache_dir: Option<PathBuf>,

    // ── Prompts ───────────────────────────────────────────────────────
    /// Short identity string prepended to the system prompt.
    pub agent_identity: String,

    /// Long-form system prompt section describing the agent's competencies
    /// and behavioral guidelines. If `None`, the built-in default
    /// ([`build_system_prompt`]) is used, which dynamically includes only the
    /// sections for enabled tool groups.
    pub system_prompt: Option<String>,

    // ── Feature flags ─────────────────────────────────────────────────
    /// Whether to enable DAG history persistence (snapshots / refs).
    pub enable_dag_history: bool,

    /// Whether to enable bibliography tools (lit_search, bib_*, etc.).
    pub enable_bibliography: bool,

    /// Whether to enable writing tools (doc_create, doc_edit, doc_compile, etc.).
    pub enable_writing: bool,

    /// Whether to enable OpenGWAS tools. Requires a valid token.
    pub enable_opengwas: bool,

    /// Whether to enable Open Targets Platform tools.
    pub enable_opentargets: bool,

    /// Whether to enable GWAS Catalog tools.
    #[serde(default = "default_true")]
    pub enable_gwascatalog: bool,

    /// Whether to enable ChEMBL tools.
    #[serde(default = "default_true")]
    pub enable_chembl: bool,

    /// Whether to enable RCSB PDB structure tools.
    #[serde(default = "default_true")]
    pub enable_rcsb: bool,

    /// Whether to enable STRING protein-association tools.
    #[serde(default = "default_true")]
    pub enable_string: bool,

    /// Whether to enable KEGG tools.
    #[serde(default = "default_true")]
    /// Whether to enable KEGG tools.
    pub enable_kegg: bool,
    /// Inject persistent memory and expose memory read/search tools.
    #[serde(default = "default_true")]
    pub use_memory: bool,
    /// Generate and consolidate persistent memory for root-level agents.
    #[serde(default = "default_true")]
    pub generate_memory: bool,
    /// Expose KMS entity/knowledge/index tools backed by Turso.
    #[serde(default)]
    pub enable_kms: bool,
    /// Run the skill-evolution service: background worker that
    /// distills observations into skill proposals (and revises
    /// installed auto-skills) on observation events and a periodic
    /// sweep. The approval gate still applies per
    /// `skill_evolution_auto_approve`.
    #[serde(default = "default_true")]
    pub enable_skill_evolution: bool,
    /// Let the evolution service auto-approve eligible proposals.
    /// Default false — human review stays in the loop; automation
    /// only writes pending proposals.
    #[serde(default)]
    pub skill_evolution_auto_approve: bool,
    /// Periodic evolution sweep cadence in seconds (default 6h). The
    /// event-driven path (observation recorded) runs independently of
    /// this timer.
    #[serde(default = "default_skill_evolution_interval_secs")]
    pub skill_evolution_interval_secs: u64,

    // ── HTTP client (shared via `BibShared`) ─────────────────────────
    /// Configuration for the process-wide `reqwest::Client` used by
    /// every literature source (PubMed / arXiv / bioRxiv) and by the
    /// Europe PMC full-text fetcher. Touched fields flow through env
    /// vars [`ENV_HTTP_USER_AGENT`], [`ENV_HTTP_CONNECT_TIMEOUT_SECS`],
    /// [`ENV_HTTP_REQUEST_TIMEOUT_SECS`], and [`ENV_HTTP_PROXY`].
    #[serde(default)]
    pub bib_http: BibHttpOptions,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::resolve(None)
    }
}

fn default_true() -> bool {
    true
}

/// Six hours: frequent enough that overnight accumulation is dealt
/// with, rare enough that the sweep is noise on any metric.
const DEFAULT_SKILL_EVOLUTION_INTERVAL_SECS: u64 = 6 * 60 * 60;

fn default_skill_evolution_interval_secs() -> u64 {
    DEFAULT_SKILL_EVOLUTION_INTERVAL_SECS
}

impl RuntimeConfig {
    /// Create a new [`RuntimeConfigBuilder`].
    pub fn builder() -> RuntimeConfigBuilder {
        RuntimeConfigBuilder::new()
    }

    /// Resolve all fields from environment variables + defaults.
    ///
    /// Pass `base` to overlay explicit values from a partially-filled
    /// builder; pass `None` for the pure env+default resolution.
    fn resolve(base: Option<&RuntimeConfigBuilder>) -> Self {
        let data_dir = base
            .and_then(|b| b.data_dir.clone())
            .or_else(|| env_path(ENV_DATA_DIR))
            .unwrap_or_else(|| {
                // Default: $HOME/.autonomics/data (absolute, independent of CWD).
                std::env::var_os("HOME")
                    .map(|h| PathBuf::from(h).join(DEFAULT_DATA_DIR))
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR))
            });

        let state_dir = base
            .and_then(|b| b.state_dir.clone())
            .or_else(|| env_path(ENV_STATE_DIR))
            .unwrap_or_else(|| {
                // Default: ~/.autonomics (absolute, independent of CWD).
                std::env::var_os("HOME")
                    .map(|h| PathBuf::from(h).join(DEFAULT_STATE_DIR))
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIR))
            });

        let dag_history_db = base
            .and_then(|b| b.dag_history_db.clone())
            .unwrap_or_else(|| state_dir.join(DEFAULT_DAG_HISTORY_DB));

        let bib_db_path = base
            .and_then(|b| b.bib_db_path.clone())
            .or_else(|| env_path(ENV_BIB_DB))
            .unwrap_or_else(|| state_dir.join(DEFAULT_BIB_DB));

        let writing_db_path = base
            .and_then(|b| b.writing_db_path.clone())
            .or_else(|| env_path(ENV_WRITING_DB))
            .unwrap_or_else(|| state_dir.join(DEFAULT_WRITING_DB));

        let app_db_path = base
            .and_then(|b| b.app_db_path.clone())
            .or_else(|| env_path(ENV_APP_DB))
            .unwrap_or_else(|| state_dir.join(DEFAULT_APP_DB));

        let agent_db = base
            .and_then(|b| b.agent_db.clone())
            .unwrap_or_else(|| state_dir.join(DEFAULT_AGENT_DB));
        let name = base
            .and_then(|b| b.name.clone())
            .unwrap_or_else(|| "default".to_string());

        let opengwas_token = base
            .and_then(|b| b.opengwas_token.clone())
            .or_else(|| std::env::var(ENV_OPENGWAS_TOKEN).ok());

        let opengwas_cache_dir = base
            .and_then(|b| b.opengwas_cache_dir.clone())
            .or_else(|| env_path(ENV_OPENGWAS_CACHE_DIR));

        let agent_identity = base
            .and_then(|b| b.agent_identity.clone())
            .unwrap_or_else(|| DEFAULT_AGENT_IDENTITY.to_string());

        let system_prompt = base.and_then(|b| b.system_prompt.clone());

        let resolve_flag =
            |b: Option<&RuntimeConfigBuilder>,
             getter: fn(&RuntimeConfigBuilder) -> Option<bool>,
             default: bool| { b.and_then(getter).unwrap_or(default) };
        let resolve_env_flag = |explicit: Option<bool>, env: &'static str| {
            explicit.or_else(|| {
                std::env::var(env)
                    .ok()
                    .and_then(|value| match value.as_str() {
                        "1" | "true" | "yes" | "on" => Some(true),
                        "0" | "false" | "no" | "off" => Some(false),
                        _ => None,
                    })
            })
        };

        Self {
            name,
            data_dir,
            state_dir,
            dag_history_db,
            bib_db_path,
            writing_db_path,
            app_db_path,
            agent_db,
            opengwas_token,
            opengwas_cache_dir,
            agent_identity,
            system_prompt,
            enable_dag_history: resolve_flag(base, |b| b.enable_dag_history, true),
            enable_bibliography: resolve_flag(base, |b| b.enable_bibliography, true),
            enable_writing: resolve_flag(base, |b| b.enable_writing, true),
            enable_opengwas: resolve_flag(base, |b| b.enable_opengwas, true),
            enable_opentargets: resolve_flag(base, |b| b.enable_opentargets, true),
            enable_gwascatalog: resolve_flag(base, |b| b.enable_gwascatalog, true),
            enable_chembl: resolve_flag(base, |b| b.enable_chembl, true),
            enable_rcsb: resolve_flag(base, |b| b.enable_rcsb, true),
            enable_string: resolve_flag(base, |b| b.enable_string, true),
            enable_kegg: resolve_flag(base, |b| b.enable_kegg, true),
            use_memory: resolve_env_flag(base.and_then(|b| b.use_memory), ENV_USE_MEMORY)
                .unwrap_or(true),
            generate_memory: resolve_env_flag(
                base.and_then(|b| b.generate_memory),
                ENV_GENERATE_MEMORY,
            )
            .unwrap_or(true),
            enable_kms: resolve_flag(base, |b| b.enable_kms, false),
            enable_skill_evolution: resolve_flag(base, |b| b.enable_skill_evolution, true),
            skill_evolution_auto_approve: resolve_flag(
                base,
                |b| b.skill_evolution_auto_approve,
                false,
            ),
            skill_evolution_interval_secs: base
                .and_then(|b| b.skill_evolution_interval_secs)
                .unwrap_or(DEFAULT_SKILL_EVOLUTION_INTERVAL_SECS),
            bib_http: resolve_bib_http(base),
        }
    }
}

// ---------------------------------------------------------------------------
// System-prompt section constants
// ---------------------------------------------------------------------------
//
// Each section is a self-contained `&str` so the dynamic builder can include
// only the sections that match the agent profile's enabled tool groups.

const PROMPT_HEADER: &str = "\
## Core Competencies\n\
\n\
You are a biomedical research assistant with expertise in genomics, GWAS analysis, \
and literature mining. You have direct access to specialized tools — use them \
proactively rather than answering from memory alone.";

const PROMPT_BIBLIOGRAPHY: &str = "\n\
### Literature & Evidence\n\
- Literature retrieval runs on the DAG evidence channel: add a \
  `source_literature` node (structured multi-source search: PubMed, arXiv, \
  bioRxiv, OpenAlex, Crossref, Semantic Scholar), a `source_literature_fetch` \
  node (one article by DOI / PMID / arXiv / S2 / OpenAlex id), a \
  `source_literature_citations` node (papers citing a paper, or its reference \
  list), or a `source_s2_recommendations` node, wire them through \
  `evidence_merge` when combining searches, then `run_dag` and `get_output` — \
  evidence outputs render as a compact citation list inline.\n\
- Give every evidence node its own output `path`; chain `evidence_export` \
  (bibtex / ris / markdown) when a bibliography file is needed.\n\
- Use `bib_save` to store articles in your personal library, `bib_search_library` \
  to find saved articles, and `bib_export` to export collections.\n\
- `bib_export` writes rendered citation files directly to a VFS `output_path`.\n\
- Always verify claims against primary literature when possible.";

const PROMPT_OPENGWAS: &str = "\n\
### Genomics & GWAS (OpenGWAS API)\n\
- Use `gwasinfo_count` to check how many records exist in a GWAS dataset.\n\
- Use `ld_matrix` to compute LD matrices between variants.\n\
- Use `download_files` to download summary-statistics files from OpenGWAS.\n\
- For fetching GWAS summary-statistics tables as pipeline inputs, use the DAG \
  source nodes `source_opengwas_*` (e.g. `source_opengwas_phewas`, \
  `source_opengwas_variants`, `source_opengwas_assoc`).\n\
- Interpret results with appropriate statistical context (p-values, effect sizes, \
  odds ratios).";

const PROMPT_OPENTARGETS: &str = "\n\
### Target–Disease Evidence (Open Targets Platform)\n\
- Query the Open Targets Platform for genes, diseases, drugs, studies, and variants.\n\
- Look up target/disease associations, associated diseases for a target (and vice versa), \
  drug info, GWAS study metadata, and variant records.\n\
- Use `opentargets_search` for free-text discovery across all entity types.";

const PROMPT_GWASCATALOG: &str = "\n\
### GWAS Catalog (EBI)\n\
- Search curated GWAS Catalog studies, associations, EFO traits, SNPs, and unpublished \
  submissions (`gwascatalog_*` tools).\n\
- Use `gwascatalog_search` first for cross-resource discovery (Solr full-text across studies, \
  variants, traits, genes, publications).\n\
- Use `gwascatalog_summary_*` tools for per-variant harmonised summary statistics (effect sizes, \
  alleles, p-values) — distinct from the curated REST resources.";

const PROMPT_CHEMBL: &str = "\n\
### Drug & Bioactivity Data (ChEMBL)\n\
- Use `chembl_search` to resolve compound or target names into stable ChEMBL IDs.\n\
- Use `chembl_molecule_summary`, `chembl_target_summary`, and `chembl_activities` for \
  compound properties, protein components, and standardized activity measurements.\n\
- Use `chembl_mechanisms` and `chembl_indications` for mechanisms of action and drug \
  indications. In pipelines, use `source_chembl_activities` or \
  `source_chembl_molecules` to obtain typed tables for SQL and analysis nodes.";

const PROMPT_RCSB: &str = "\
### Structural Biology (RCSB PDB)\n\
- Use `rcsb_search` to discover PDB entries by free text or an expert RCSB query.\n\
- Use `rcsb_entry` to summarize an entry's experiment, resolution, composition, \
deposition dates, and primary citation.\n\
- Use `rcsb_polymer` to inspect sequence length, UniProt mapping, source organism, \
genes, and polymer copy count.\n\
- Use `rcsb_structure_preview` for a bounded mmCIF/PDB/FASTA preview. For complete \
structure files or downstream computation, prefer the `source_rcsb_*` DAG nodes.";

const PROMPT_STRING: &str = "\n\
### Protein Association (STRING)\n\
- Resolve ambiguous genes or proteins with `string_resolve_identifiers` before querying networks.\n\
- Use `string_network_summary` for compact interaction, PPI-enrichment, and functional-enrichment \
  evidence; require at least two proteins so enrichment is not computed on an auto-expanded \
  one-protein neighborhood.\n\
- Use `string_network_image` only when the user needs visual preview. For pipeline calculations, \
  prefer the `source_string_*` DAG nodes.";
const PROMPT_KEGG: &str = "\n\
### KEGG (Academic Use)\n\
- Use `kegg_info`, `kegg_find`, and `kegg_entry_preview` to inspect pathways, genes, \
  orthologs, compounds, drugs, and diseases.\n\
- Use `kegg_link` and `kegg_convert` for biological relationships and identifier mapping; \
  prefer one database-level request or cached results over per-gene calls.\n\
- Keep outputs concise and remember that KEGG API access is limited to academic use.";

const PROMPT_BIOMEDICAL_RESOURCES: &str = "\n\
### Biomedical Reference Resources\n\
- Use `alphafold_lookup` for UniProt-linked predicted structure models, versions, quality, \
  and downloadable CIF/PDB/confidence URLs.\n\
- Use `interpro_lookup` for protein family/domain entry metadata, member databases, GO terms, \
  and representative structures.\n\
- Use `pubchem_compound_lookup` to resolve compounds by name, CID, or InChIKey and return \
  chemical identifiers and properties.\n\
- Use `clinicaltrials_study_lookup` for a study's status, sponsor, design, conditions, \
  interventions, and primary outcomes.";

const PROMPT_DAG_ENGINE: &str = "\n\
### Data Pipeline (DAG Engine)\n\
- Build and execute data processing pipelines: add data sources, apply SQL transforms, \
  connect nodes into a DAG, run the pipeline, and retrieve output.\n\
\n\
- **Prefer dedicated nodes**: before adding a source, processing, analysis, or external-tool \
  node, inspect the available node factories and choose the dedicated node kind that is most \
  appropriate for the operation. If no registered node supports the required semantics, \
  schema handling, or computation, tell the user that the operation is unsupported instead of \
  assembling an equivalent manually.\n\
\n\
- **Channel/dataflow control**: `add_node` is for registered typed nodes only. When a workflow \
  needs Channel operators (`of_items`, `map`, `filter`, `flatten`, `mix`, `collect`, `combine`, \
  `join`, `group_tuple`, `branch`) or logical strategies (`for_each`, `dynamic_for_each`, \
  `gather`), call `add_logical_graph` with the self-contained typed logical graph. Use the \
  returned physical job IDs to connect its compiled nodes to the rest of the DAG with `add_edge`. \
  Do not attempt to express these operators as SQL or an unregistered node kind.\n\
\n\
- **Visualization**: the `visualization` manifest plugin is a terminal sink for \
  plot-ready data. Perform filtering, aggregation, normalization, modeling, and all \
  other computation in upstream dedicated or SQL nodes. The node rejects arbitrary \
  computation in its R script and downstream edges from the rendered plot.\n\
\n\
- Use this when a task requires multi-step data processing or transformation.\n\
\n\
- **Build incrementally, layer by layer — never construct the full DAG in one shot.** \
  Start with just the data source node, run_dag, and inspect the output columns to \
  understand what you have. Then add the next processing node (a SQL transform, a filter, \
  an analysis), wire it, run again, and verify the output matches expectations before \
  extending further. Repeat until the pipeline reaches the final analysis. \
  This feedback loop catches schema mismatches, wrong column names, and type errors \
  early — a single-shot full-DAG construction fails silently and wastes time debugging.\n\
\n\
- **Inspect ports before wiring**: every node kind declares typed input/output ports. \
  `list_node_factories` returns lightweight metadata (kind + short description) only. \
  To see the full port layout (port count, variadic flag, per-port column schema), \
  call `get_node_ports` with the chosen `kind`. For dynamic-port kinds, pass the exact \
  `spec` so declared outputs are included. Read the downstream node's input \
  port schema BEFORE writing the transform that feeds it. The downstream port's \
  required columns and types are a contract, not a suggestion. \
  Similarly, call `get_node_spec` to fetch the JSON Schema a node expects for its \
  configuration parameters, and `get_node_doc` for detailed usage documentation.\n\
\n\
- **Transform to match the consuming port**: data flowing along an edge MUST conform to the \
  downstream node's input port schema. If the upstream output does not already match, insert \
  a dedicated SQL transform node between them that projects, casts, renames, or extracts \
  subfields so its output is exactly what the downstream port expects. Do not connect a \
  node's output to a downstream input hoping it will work — verify column names, types, \
  and struct shape first, and reshape explicitly. \
  Examples: an `ldsc` input port requires columns `z: Float64, n: Float64, rsid: Utf8` — \
  if upstream exposes `beta`, `se`, `n`, `rsid`, add a SQL node computing \
  \"z\" = beta / se and selecting exactly `rsid, \"z\", \"n\"`. A VCF emits an `info` Struct column; \
  extract subfields with `get_field(info, 'ES')` in the transform, never rely on a List \
  column where a Struct is required. Reserve exactly the required column names and types.";

const PROMPT_DAG_HISTORY: &str = "\n\
### DAG Version Control (History & Refs)\n\
\n\
Every `run_dag` call **automatically saves a snapshot** of the full pipeline (all nodes, \
edges, specs) plus the run report to a local history database. Snapshots are organized \
into **refs** (branches) — each ref is an independent lineage.\n\
\n\
- **Always provide a `commit_message`** when calling `run_dag`. A descriptive message \
  like \"LDSC h² with 200 blocks on BMI\" makes it easy to find past runs later via \
  `dag_history_log`.\n\
\n\
- **One analysis = one ref.** When starting a new, unrelated analysis pipeline, call \
  `new_dag_ref` with a descriptive name (e.g. \"gwas-bmi\", \"epi-charls\") instead of \
  building on top of the previous pipeline. This keeps histories cleanly separated. \
  The old pipeline's snapshots are preserved — switch back anytime with `switch_dag_ref`.\n\
\n\
- **Reviewing history.** Use `dag_history_log` to see past snapshots in the current ref, \
  and `list_dag_refs` to see all analysis lineages. The `*` marker shows the active ref.\n\
\n\
- **Recovering past work.** `checkout_dag` loads a historical snapshot's pipeline into \
  memory without changing the ref (like `git checkout`). `branch_from_snapshot` creates \
  a new ref from any historical snapshot (like `git checkout -b`), letting you explore \
  an alternative direction from that point.";

const PROMPT_SQL_CONVENTIONS: &str = "\n\
### SQL Conventions\n\
All SQL in this system runs on Apache DataFusion. The following rules apply to \
every SQL string you write — whether in `add_sql_node`, `add_source`, \
or any other tool that accepts SQL.\n\
\n\
- **Double-quote all column names**: DataFusion normalizes unquoted identifiers to \
  lowercase by default (`enable_ident_normalization = true`). Always wrap column \
  names (and any identifier whose case matters) with double quotes. \
  Wrong: `SELECT Z, N FROM port_0`  —  Z and N become `z`, `n` silently. \
  Right: `SELECT \"Z\", \"N\" FROM port_0`  —  case is preserved exactly.\n\
\n\
- **Table naming in SQL nodes**: in `add_sql_node`, upstream data is registered as tables \
  named `port_N` where N is the input port index (0-based). For single-input nodes the \
  table is `port_0`. Never use the upstream node's id — always use `port_N`. \
  Example: a filter node receiving one input → `SELECT * FROM port_0 WHERE x > 1`. \
  A two-input join node → `SELECT * FROM port_0 JOIN port_1 ON port_0.id = port_1.id`.\n\
\n\
- **Cast to double precision with `DOUBLE`, never `FLOAT64`**: DataFusion's SQL parser \
  uses SQL-standard type names. The 64-bit floating type is `DOUBLE`; `FLOAT64` is an \
  Arrow/Rust type name and is NOT valid SQL — `CAST(x AS FLOAT64)` will error with a \
  parse/type failure. Always write `CAST(x AS DOUBLE)` (or `TRY_CAST(x AS DOUBLE)` to \
  coerce non-numeric strings to NULL instead of failing). \
  Wrong: `CAST(\"Z\" AS FLOAT64)`  —  parser error. \
  Right: `CAST(\"Z\" AS DOUBLE)`. \
  The same applies to other types: prefer SQL-standard names (`INTEGER`, `BIGINT`, \
  `VARCHAR`, `DOUBLE`) over their Arrow equivalents (`INT32`, `INT64`, `UTF8`, `FLOAT64`).";

const PROMPT_GENERAL: &str = "\n\
### General\n\
- Read, write, and manage files on the local filesystem.\n\
- Break complex research questions into sequential tool calls; explain your reasoning.\n\
\n\
## Guidelines\n\
- Cite PMID(s) when referencing literature.\n\
- Report quantitative results with appropriate precision and confidence intervals when available.\n\
- If a tool call fails, diagnose the error and retry with corrected parameters before asking the user.";

/// Trait so [`build_system_prompt`] can accept either an [`AgentProfile`] or a
/// [`RuntimeConfig`] — both carry the same boolean tool-capability flags.
pub trait PromptCapabilities {
    fn enable_bibliography(&self) -> bool;
    fn enable_opengwas(&self) -> bool;
    fn enable_opentargets(&self) -> bool;
    fn enable_gwascatalog(&self) -> bool;
    fn enable_chembl(&self) -> bool;
    fn enable_rcsb(&self) -> bool;
    fn enable_string(&self) -> bool;
    fn enable_kegg(&self) -> bool;
    fn enable_dag_history(&self) -> bool;
}

/// Build the default system prompt dynamically, including only the sections for
/// tool groups that are actually enabled. This prevents the prompt from
/// advertising tools the agent cannot access.
pub fn build_system_prompt<C: PromptCapabilities>(caps: &C) -> String {
    let mut s = String::from(PROMPT_HEADER);

    if caps.enable_bibliography() {
        s.push_str(PROMPT_BIBLIOGRAPHY);
    }
    if caps.enable_opengwas() {
        s.push_str(PROMPT_OPENGWAS);
    }
    if caps.enable_opentargets() {
        s.push_str(PROMPT_OPENTARGETS);
    }
    if caps.enable_gwascatalog() {
        s.push_str(PROMPT_GWASCATALOG);
    }
    if caps.enable_chembl() {
        s.push_str(PROMPT_CHEMBL);
    }
    if caps.enable_rcsb() {
        s.push_str(PROMPT_RCSB);
    }
    if caps.enable_string() {
        s.push_str(PROMPT_STRING);
    }
    if caps.enable_kegg() {
        s.push_str(PROMPT_KEGG);
    }
    s.push_str(PROMPT_BIOMEDICAL_RESOURCES);

    // DAG engine, SQL conventions, and general sections are always included —
    // the data-engine tools and filesystem tools are always registered.
    s.push_str(PROMPT_DAG_ENGINE);

    if caps.enable_dag_history() {
        s.push_str(PROMPT_DAG_HISTORY);
    }

    s.push_str(PROMPT_SQL_CONVENTIONS);

    s.push_str(PROMPT_GENERAL);
    s
}

/// The full default system prompt with all sections enabled.
///
/// Prefer [`build_system_prompt`] when you have an [`AgentProfile`] or
/// [`RuntimeConfig`] — that function omits sections for disabled tool groups.
pub fn default_system_prompt() -> String {
    struct AllEnabled;
    impl PromptCapabilities for AllEnabled {
        fn enable_bibliography(&self) -> bool {
            true
        }
        fn enable_opengwas(&self) -> bool {
            true
        }
        fn enable_opentargets(&self) -> bool {
            true
        }
        fn enable_gwascatalog(&self) -> bool {
            true
        }
        fn enable_chembl(&self) -> bool {
            true
        }
        fn enable_rcsb(&self) -> bool {
            true
        }
        fn enable_string(&self) -> bool {
            true
        }
        fn enable_kegg(&self) -> bool {
            true
        }
        fn enable_dag_history(&self) -> bool {
            true
        }
    }
    build_system_prompt(&AllEnabled)
}

impl PromptCapabilities for RuntimeConfig {
    fn enable_bibliography(&self) -> bool {
        self.enable_bibliography
    }
    fn enable_opengwas(&self) -> bool {
        self.enable_opengwas
    }
    fn enable_opentargets(&self) -> bool {
        self.enable_opentargets
    }
    fn enable_gwascatalog(&self) -> bool {
        self.enable_gwascatalog
    }
    fn enable_chembl(&self) -> bool {
        self.enable_chembl
    }
    fn enable_rcsb(&self) -> bool {
        self.enable_rcsb
    }
    fn enable_string(&self) -> bool {
        self.enable_string
    }
    fn enable_kegg(&self) -> bool {
        self.enable_kegg
    }
    fn enable_dag_history(&self) -> bool {
        self.enable_dag_history
    }
}

impl RuntimeConfig {
    /// Return the system prompt, falling back to the built-in default.
    pub fn system_prompt_or_default(&self) -> String {
        self.system_prompt
            .clone()
            .unwrap_or_else(|| build_system_prompt(self))
    }

    /// Return a display-friendly summary for logging.
    pub fn summary(&self) -> String {
        format!(
            "RuntimeConfig {{ name: {:?}, data_dir: {}, state_dir: {}, \
             dag_history_db: {}, bib_db: {}, app_db: {}, \
             dag_history: {}, bib: {}, opengwas: {}, \
             opentargets: {}, gwascatalog: {}, chembl: {}, rcsb: {}, string: {}, kegg: {} }}",
            self.name,
            self.data_dir.display(),
            self.state_dir.display(),
            self.dag_history_db.display(),
            self.bib_db_path.display(),
            self.app_db_path.display(),
            self.enable_dag_history,
            self.enable_bibliography,
            self.enable_opengwas,
            self.enable_opentargets,
            self.enable_gwascatalog,
            self.enable_chembl,
            self.enable_rcsb,
            self.enable_string,
            self.enable_kegg,
        )
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder for [`RuntimeConfig`].
///
/// Every method is optional; unset fields fall through to environment
/// variables and then hard-coded defaults. Call [`build`](Self::build) to
/// produce a fully resolved [`RuntimeConfig`].
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RuntimeConfigBuilder {
    pub(crate) name: Option<String>,
    pub(crate) data_dir: Option<PathBuf>,
    pub(crate) state_dir: Option<PathBuf>,
    pub(crate) dag_history_db: Option<PathBuf>,
    pub(crate) bib_db_path: Option<PathBuf>,
    pub(crate) writing_db_path: Option<PathBuf>,
    pub(crate) app_db_path: Option<PathBuf>,
    pub(crate) agent_db: Option<PathBuf>,
    pub(crate) opengwas_token: Option<String>,
    pub(crate) opengwas_cache_dir: Option<PathBuf>,
    pub(crate) agent_identity: Option<String>,
    pub(crate) system_prompt: Option<String>,
    pub(crate) enable_dag_history: Option<bool>,
    pub(crate) enable_bibliography: Option<bool>,
    pub(crate) enable_writing: Option<bool>,
    pub(crate) enable_opengwas: Option<bool>,
    pub(crate) enable_opentargets: Option<bool>,
    pub(crate) enable_gwascatalog: Option<bool>,
    #[serde(default)]
    pub(crate) enable_chembl: Option<bool>,
    #[serde(default)]
    pub(crate) enable_rcsb: Option<bool>,
    pub(crate) enable_string: Option<bool>,
    #[serde(default)]
    pub(crate) enable_kegg: Option<bool>,
    pub(crate) use_memory: Option<bool>,
    pub(crate) generate_memory: Option<bool>,
    pub(crate) enable_kms: Option<bool>,
    pub(crate) enable_skill_evolution: Option<bool>,
    pub(crate) skill_evolution_auto_approve: Option<bool>,
    pub(crate) skill_evolution_interval_secs: Option<u64>,
    pub(crate) bib_http: Option<BibHttpOptions>,
}

impl RuntimeConfigBuilder {
    fn new() -> Self {
        Self::default()
    }

    /// Human-readable name for this runtime instance.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Root directory for the virtual file system exposed to agent tools.
    pub fn data_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.data_dir = Some(dir.into());
        self
    }

    /// Directory for agent-internal state (DAG history DB, etc.).
    pub fn state_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.state_dir = Some(dir.into());
        self
    }

    /// Explicit path to the DAG history SQLite database (overrides
    /// `state_dir/dag-history.db`).
    pub fn dag_history_db(mut self, path: impl Into<PathBuf>) -> Self {
        self.dag_history_db = Some(path.into());
        self
    }

    /// Path to the bibliography SQLite database.
    pub fn bib_db_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.bib_db_path = Some(path.into());
        self
    }

    /// Path to the writing-system SQLite database.
    pub fn writing_db_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.writing_db_path = Some(path.into());
        self
    }

    /// Path to the TUI / application SQLite database.
    pub fn app_db_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.app_db_path = Some(path.into());
        self
    }

    /// Path to the agent persistence database (Turso/SQLite).
    pub fn agent_db(mut self, path: impl Into<PathBuf>) -> Self {
        self.agent_db = Some(path.into());
        self
    }

    /// OpenGWAS API token. If not set, `OPENGWAS_TOKEN` env var is used.
    pub fn opengwas_token(mut self, token: impl Into<String>) -> Self {
        self.opengwas_token = Some(token.into());
        self
    }

    /// OpenGWAS on-disk cache directory override.
    pub fn opengwas_cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.opengwas_cache_dir = Some(dir.into());
        self
    }

    /// Short identity string prepended to the system prompt.
    pub fn agent_identity(mut self, identity: impl Into<String>) -> Self {
        self.agent_identity = Some(identity.into());
        self
    }

    /// Custom system prompt section. Pass `None` to use the built-in default.
    pub fn system_prompt(mut self, prompt: Option<String>) -> Self {
        self.system_prompt = prompt;
        self
    }

    /// Enable or disable DAG history persistence.
    pub fn enable_dag_history(mut self, enabled: bool) -> Self {
        self.enable_dag_history = Some(enabled);
        self
    }

    /// Enable or disable bibliography tools.
    pub fn enable_bibliography(mut self, enabled: bool) -> Self {
        self.enable_bibliography = Some(enabled);
        self
    }

    /// Enable or disable writing tools (doc_create, doc_edit, doc_compile, etc.).
    pub fn enable_writing(mut self, enabled: bool) -> Self {
        self.enable_writing = Some(enabled);
        self
    }

    /// Enable or disable OpenGWAS tools.
    pub fn enable_opengwas(mut self, enabled: bool) -> Self {
        self.enable_opengwas = Some(enabled);
        self
    }

    /// Enable or disable Open Targets Platform tools.
    pub fn enable_opentargets(mut self, enabled: bool) -> Self {
        self.enable_opentargets = Some(enabled);
        self
    }

    /// Enable or disable GWAS Catalog tools.
    pub fn enable_gwascatalog(mut self, enabled: bool) -> Self {
        self.enable_gwascatalog = Some(enabled);
        self
    }

    /// Enable or disable ChEMBL tools.
    pub fn enable_chembl(mut self, enabled: bool) -> Self {
        self.enable_chembl = Some(enabled);
        self
    }

    /// Enable or disable RCSB PDB tools.
    pub fn enable_rcsb(mut self, enabled: bool) -> Self {
        self.enable_rcsb = Some(enabled);
        self
    }

    /// Enable or disable STRING protein-association tools.
    pub fn enable_string(mut self, enabled: bool) -> Self {
        self.enable_string = Some(enabled);
        self
    }

    /// Enable or disable KEGG tools.
    pub fn enable_kegg(mut self, enabled: bool) -> Self {
        self.enable_kegg = Some(enabled);
        self
    }

    /// Enable or disable memory read/injection.
    pub fn use_memory(mut self, enabled: bool) -> Self {
        self.use_memory = Some(enabled);
        self
    }

    /// Enable or disable startup memory generation.
    pub fn generate_memory(mut self, enabled: bool) -> Self {
        self.generate_memory = Some(enabled);
        self
    }

    /// Enable KMS knowledge-graph tools.
    pub fn enable_kms(mut self, enabled: bool) -> Self {
        self.enable_kms = Some(enabled);
        self
    }

    pub fn enable_skill_evolution(mut self, enabled: bool) -> Self {
        self.enable_skill_evolution = Some(enabled);
        self
    }

    pub fn skill_evolution_auto_approve(mut self, enabled: bool) -> Self {
        self.skill_evolution_auto_approve = Some(enabled);
        self
    }

    pub fn skill_evolution_interval_secs(mut self, secs: u64) -> Self {
        self.skill_evolution_interval_secs = Some(secs);
        self
    }

    /// Override the bibliography HTTP client configuration. Merged on
    /// top of any env-var defaults already resolved by
    /// [`RuntimeConfig::resolve`].
    pub fn bib_http(mut self, opts: BibHttpOptions) -> Self {
        self.bib_http = Some(opts);
        self
    }

    /// Resolve into a fully-resolved [`RuntimeConfig`].
    pub fn build(self) -> RuntimeConfig {
        RuntimeConfig::resolve(Some(&self))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read a non-empty env var as a [`PathBuf`].
fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// Resolve [`BibHttpOptions`] from the builder overlay + environment
/// variables. Builder-explicit fields win; otherwise env vars fill in;
/// otherwise everything stays `None` (i.e. the default applied by
/// [`BibHttpOptions::apply_to`]).
fn resolve_bib_http(base: Option<&RuntimeConfigBuilder>) -> BibHttpOptions {
    let overlay = base.and_then(|b| b.bib_http.clone());
    let env_user_agent = std::env::var(ENV_HTTP_USER_AGENT).ok();
    let env_connect = std::env::var(ENV_HTTP_CONNECT_TIMEOUT_SECS)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(std::time::Duration::from_secs);
    let env_request = std::env::var(ENV_HTTP_REQUEST_TIMEOUT_SECS)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(std::time::Duration::from_secs);
    let env_proxy = std::env::var(ENV_HTTP_PROXY).ok();

    BibHttpOptions {
        user_agent: overlay
            .as_ref()
            .and_then(|o| o.user_agent.clone())
            .or(env_user_agent),
        connect_timeout: overlay
            .as_ref()
            .and_then(|o| o.connect_timeout)
            .or(env_connect),
        request_timeout: overlay
            .as_ref()
            .and_then(|o| o.request_timeout)
            .or(env_request),
        proxy_url: overlay
            .as_ref()
            .and_then(|o| o.proxy_url.clone())
            .or(env_proxy),
        accept_invalid_certs: overlay.as_ref().and_then(|o| o.accept_invalid_certs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static BIB_HTTP_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static DATA_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn defaults() {
        // Guard against env-var pollution from parallel tests like
        // `env_data_dir_override` that set AUTONOMICS_DATA_DIR.
        let _guard = DATA_DIR_ENV_LOCK.lock().unwrap();
        let saved_data_dir = std::env::var_os(ENV_DATA_DIR);
        let saved_state_dir = std::env::var_os(ENV_STATE_DIR);
        // SAFETY: single-threaded within this test fn.
        unsafe {
            std::env::remove_var(ENV_DATA_DIR);
            std::env::remove_var(ENV_STATE_DIR);
        }

        let cfg = RuntimeConfig::default();
        assert_eq!(cfg.name, "default");
        // data_dir resolves to $HOME/.autonomics/data when HOME is set.
        let expected_data_dir = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(DEFAULT_DATA_DIR))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR));
        assert_eq!(cfg.data_dir, expected_data_dir);

        // state_dir resolves to $HOME/.autonomics when HOME is set.
        let expected_state_dir = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(DEFAULT_STATE_DIR))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIR));
        assert_eq!(cfg.state_dir, expected_state_dir);
        assert_eq!(
            cfg.dag_history_db,
            expected_state_dir.join(DEFAULT_DAG_HISTORY_DB)
        );
        assert_eq!(cfg.bib_db_path, expected_state_dir.join(DEFAULT_BIB_DB));
        assert_eq!(cfg.app_db_path, expected_state_dir.join(DEFAULT_APP_DB));
        assert!(cfg.enable_dag_history);
        assert!(cfg.enable_bibliography);
        assert!(cfg.enable_opengwas);
        assert!(cfg.enable_opentargets);
        assert!(cfg.enable_gwascatalog);
        assert!(cfg.enable_chembl);
        assert!(cfg.enable_rcsb);
        assert!(cfg.enable_string);
        assert!(cfg.use_memory);
        assert!(cfg.generate_memory);
        assert!(!cfg.enable_kms);

        // Restore env vars.
        // SAFETY: single-threaded within this test fn.
        unsafe {
            if let Some(v) = saved_data_dir {
                std::env::set_var(ENV_DATA_DIR, v);
            }
            if let Some(v) = saved_state_dir {
                std::env::set_var(ENV_STATE_DIR, v);
            }
        }
    }

    #[test]
    fn builder_overrides() {
        let cfg = RuntimeConfig::builder()
            .name("test-agent")
            .data_dir("/tmp/test-data")
            .state_dir("/tmp/test-state")
            .dag_history_db("/tmp/custom-history.db")
            .bib_db_path("/tmp/custom-bib.db")
            .app_db_path("/tmp/custom-app.db")
            .opengwas_token("secret-token")
            .agent_identity("Custom agent")
            .system_prompt(Some("Custom prompt".to_string()))
            .enable_dag_history(false)
            .enable_opengwas(false)
            .enable_rcsb(false)
            .use_memory(false)
            .generate_memory(true)
            .enable_kms(true)
            .build();

        assert_eq!(cfg.name, "test-agent");
        assert_eq!(cfg.data_dir, PathBuf::from("/tmp/test-data"));
        assert_eq!(cfg.state_dir, PathBuf::from("/tmp/test-state"));
        assert_eq!(cfg.dag_history_db, PathBuf::from("/tmp/custom-history.db"));
        assert_eq!(cfg.bib_db_path, PathBuf::from("/tmp/custom-bib.db"));
        assert_eq!(cfg.app_db_path, PathBuf::from("/tmp/custom-app.db"));
        assert_eq!(cfg.opengwas_token.as_deref(), Some("secret-token"));
        assert_eq!(cfg.agent_identity, "Custom agent");
        assert_eq!(cfg.system_prompt.as_deref(), Some("Custom prompt"));
        assert!(!cfg.enable_dag_history);
        assert!(!cfg.enable_opengwas);
        // Flags not touched → still true
        assert!(cfg.enable_bibliography);
        assert!(cfg.enable_opentargets);
        assert!(cfg.enable_gwascatalog);
        assert!(cfg.enable_chembl);
        assert!(!cfg.enable_rcsb);
        assert!(cfg.enable_string);
        assert!(!cfg.use_memory);
        assert!(cfg.generate_memory);
        assert!(cfg.enable_kms);
    }

    #[test]
    fn system_prompt_fallback() {
        let cfg = RuntimeConfig::default();
        assert!(cfg.system_prompt.is_none());
        assert!(cfg.system_prompt_or_default().contains("Core Competencies"));

        let cfg = RuntimeConfig::builder()
            .system_prompt(Some("short".to_string()))
            .build();
        assert_eq!(cfg.system_prompt_or_default(), "short");

        let cfg = RuntimeConfig::builder().build();
        assert!(
            cfg.system_prompt_or_default()
                .contains("Protein Association (STRING)")
        );

        let cfg = RuntimeConfig::builder().enable_string(false).build();
        assert!(
            !cfg.system_prompt_or_default()
                .contains("Protein Association (STRING)")
        );
    }

    #[test]
    fn system_prompt_dynamic_sections() {
        // All sections enabled (default config).
        let cfg = RuntimeConfig::default();
        let prompt = build_system_prompt(&cfg);
        assert!(prompt.contains("Literature & Evidence"));
        assert!(prompt.contains("OpenGWAS API"));
        assert!(prompt.contains("Open Targets Platform"));
        assert!(prompt.contains("GWAS Catalog (EBI)"));
        assert!(prompt.contains("Structural Biology (RCSB PDB)"));
        assert!(prompt.contains("Protein Association (STRING)"));
        assert!(prompt.contains("DAG Version Control"));
        // Minimal config — no external service tools.
        let cfg = RuntimeConfig::builder()
            .enable_bibliography(false)
            .enable_opengwas(false)
            .enable_opentargets(false)
            .enable_gwascatalog(false)
            .enable_string(false)
            .enable_chembl(false)
            .enable_rcsb(false)
            .enable_dag_history(false)
            .build();
        let prompt = build_system_prompt(&cfg);
        assert!(!prompt.contains("Literature & Evidence"));
        assert!(!prompt.contains("OpenGWAS API"));
        assert!(!prompt.contains("Open Targets Platform"));
        assert!(!prompt.contains("GWAS Catalog (EBI)"));
        assert!(!prompt.contains("Drug & Bioactivity Data (ChEMBL)"));
        assert!(!prompt.contains("Structural Biology (RCSB PDB)"));
        assert!(!prompt.contains("Protein Association (STRING)"));
        assert!(!prompt.contains("DAG Version Control"));
        // These are always present:
        assert!(prompt.contains("Core Competencies"));
        assert!(prompt.contains("Data Pipeline (DAG Engine)"));
        assert!(prompt.contains("SQL Conventions"));
        assert!(prompt.contains("## Guidelines"));
    }

    #[test]
    fn system_prompt_requires_dedicated_node_priority() {
        let prompt = RuntimeConfig::default().system_prompt_or_default();

        assert!(prompt.contains("**Prefer dedicated nodes**"));
        // The visualization bullet must keep pinning computation
        // upstream: it is a terminal sink, never a compute node.
        assert!(prompt.contains("**Visualization**"));
        assert!(prompt.contains("terminal sink for plot-ready data"));
        assert!(!prompt.contains("python_script"));
        assert!(!prompt.contains("r_script"));
        assert!(!prompt.contains("container_command"));
    }

    #[test]
    fn skill_evolution_defaults_and_overrides() {
        // Defaults: service on, review gate on (propose-only), 6h sweep.
        let cfg = RuntimeConfig::default();
        assert!(cfg.enable_skill_evolution);
        assert!(!cfg.skill_evolution_auto_approve);
        assert_eq!(cfg.skill_evolution_interval_secs, 6 * 60 * 60);

        // Builder overrides flow through resolve.
        let cfg = RuntimeConfig::builder()
            .enable_skill_evolution(false)
            .skill_evolution_auto_approve(true)
            .skill_evolution_interval_secs(600)
            .build();
        assert!(!cfg.enable_skill_evolution);
        assert!(cfg.skill_evolution_auto_approve);
        assert_eq!(cfg.skill_evolution_interval_secs, 600);
    }

    #[test]
    fn env_data_dir_override() {
        let _guard = DATA_DIR_ENV_LOCK.lock().unwrap();
        // SAFETY: guarded by DATA_DIR_ENV_LOCK for tests that assert defaults.
        unsafe {
            std::env::set_var(ENV_DATA_DIR, "/tmp/env-data");
        }
        let cfg = RuntimeConfig::default();
        assert_eq!(cfg.data_dir, PathBuf::from("/tmp/env-data"));
        unsafe {
            std::env::remove_var(ENV_DATA_DIR);
        }
    }

    #[test]
    fn builder_takes_precedence_over_env() {
        let _guard = DATA_DIR_ENV_LOCK.lock().unwrap();
        // SAFETY: guarded by DATA_DIR_ENV_LOCK for tests that assert defaults.
        unsafe {
            std::env::set_var(ENV_DATA_DIR, "/tmp/env-data");
        }
        let cfg = RuntimeConfig::builder()
            .data_dir("/tmp/explicit-data")
            .build();
        assert_eq!(cfg.data_dir, PathBuf::from("/tmp/explicit-data"));
        unsafe {
            std::env::remove_var(ENV_DATA_DIR);
        }
    }

    #[test]
    fn bib_http_defaults_to_all_none() {
        let _guard = BIB_HTTP_ENV_LOCK.lock().unwrap();
        let cfg = RuntimeConfig::default();
        assert!(cfg.bib_http.user_agent.is_none());
        assert!(cfg.bib_http.connect_timeout.is_none());
        assert!(cfg.bib_http.request_timeout.is_none());
        assert!(cfg.bib_http.proxy_url.is_none());
        assert!(cfg.bib_http.accept_invalid_certs.is_none());
    }

    #[test]
    fn bib_http_builder_overrides() {
        let opts = BibHttpOptions {
            user_agent: Some("autonomics-test/0.1".into()),
            connect_timeout: Some(std::time::Duration::from_secs(3)),
            request_timeout: Some(std::time::Duration::from_secs(20)),
            proxy_url: Some("http://proxy.test:3128".into()),
            accept_invalid_certs: Some(false),
        };
        let cfg = RuntimeConfig::builder().bib_http(opts.clone()).build();
        assert_eq!(cfg.bib_http, opts);
    }

    #[test]
    fn bib_http_env_user_agent_override() {
        let _guard = BIB_HTTP_ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var(ENV_HTTP_USER_AGENT, "from-env");
        }
        let cfg = RuntimeConfig::default();
        assert_eq!(cfg.bib_http.user_agent.as_deref(), Some("from-env"));
        unsafe {
            std::env::remove_var(ENV_HTTP_USER_AGENT);
        }
    }
}
