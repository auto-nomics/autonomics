//! The fully resolved [`RuntimeConfig`](super::RuntimeConfig) and its
//! env/default resolution logic.

use std::path::PathBuf;

use bib_base::BibHttpOptions;

use super::builder::RuntimeConfigBuilder;
use super::defaults::*;
use super::prompt::build_system_prompt;
use super::rsi::PluginRsiConfig;

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

    /// Path to the persistent-memory knowledge database (Turso/SQLite).
    #[serde(default = "default_knowledge_db_path")]
    pub knowledge_db_path: PathBuf,

    /// Path to the KMS knowledge database (Turso/SQLite). Legacy rows are
    /// migrated out of `agent.db` into this file once on startup.
    #[serde(default = "default_kms_db_path")]
    pub kms_db_path: PathBuf,

    /// Path to the wiki knowledge database (Turso/SQLite).
    #[serde(default = "default_wiki_db_path")]
    pub wiki_db_path: PathBuf,

    /// Path to the slide-deck engine database (Turso/SQLite).
    #[serde(default = "default_deck_db_path")]
    pub deck_db_path: PathBuf,
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

    /// Whether browser-kind agents should run their Chromium process with
    /// `--no-sandbox` (needed inside most containers). Has no effect on
    /// non-browser agents.
    #[serde(default)]
    pub browser_no_sandbox: bool,

    /// Whether browser-kind agents run headed (visible window) or headless.
    /// Defaults to `true`; pass `false` to `RuntimeConfig::builder` for
    /// local debugging sessions.
    #[serde(default = "default_true")]
    pub browser_headless: bool,

    /// Directory used by browser-kind agents for Chromium downloads.
    ///
    /// `None` resolves to `<state_dir>/browser-downloads`; the directory is
    /// mounted in the agent VFS at `/browser-downloads`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_download_dir: Option<PathBuf>,

    /// Inject persistent memory and expose memory read/search tools.
    #[serde(default = "default_true")]
    pub use_memory: bool,
    /// Generate and consolidate persistent memory for root-level agents.
    #[serde(default = "default_true")]
    pub generate_memory: bool,
    /// Expose KMS entity/knowledge/index tools backed by Turso.
    #[serde(default)]
    pub enable_kms: bool,
    /// Default text-embedding provider configuration, persisted via
    /// the gateway's embedding-config endpoints; per-feature consumers
    /// (bibliography semantic search) construct their own embedders
    /// from it.
    #[serde(default)]
    pub embedding: text_embedding::EmbeddingConfig,

    // ── Agent instance lifecycle (virtual actor) ─────────────────────
    /// Idle duration after which the host passivates a live agent
    /// instance (graceful persist + registry eviction). The identity and
    /// history stay in storage; the next message re-activates the agent.
    /// `None` disables passivation — instances live until explicitly shut
    /// down. Passivation only fires on idle-compatible statuses with no
    /// in-flight background work.
    #[serde(default = "default_agent_idle_ttl")]
    pub agent_idle_ttl: Option<std::time::Duration>,

    // ── HTTP client (shared via `BibShared`) ─────────────────────────
    /// Configuration for the process-wide `reqwest::Client` used by
    /// every literature source (PubMed / arXiv / bioRxiv) and by the
    /// Europe PMC full-text fetcher. Touched fields flow through env
    /// vars [`ENV_HTTP_USER_AGENT`], [`ENV_HTTP_CONNECT_TIMEOUT_SECS`],
    /// [`ENV_HTTP_REQUEST_TIMEOUT_SECS`], and [`ENV_HTTP_PROXY`].
    #[serde(default)]
    pub bib_http: BibHttpOptions,
    /// Plugin-based recursive self-improvement services.
    #[serde(default)]
    pub plugin_rsi: PluginRsiConfig,
    /// Central resource downloader policy, transport limits, and object-store
    /// profiles. Defaults to deny-all network sources.
    #[serde(default)]
    pub downloader: resource_downloader::DownloaderConfig,
    /// Slurm (HPC) integration settings. Defaults to disabled.
    #[serde(default)]
    pub slurm: super::slurm::SlurmConfig,
    /// Reuse authenticated SSH master connections across every SSH channel
    /// (execution, file access, scheduler tools). Each invocation stays an
    /// independent exec session; only the TCP/KEX/auth handshake is
    /// shared, keyed per destination endpoint. Defaults to off (stateless
    /// one-shot connections).
    #[serde(default)]
    pub ssh_multiplex: bool,
    /// OpenSSH duration bounding how long an idle multiplexing master
    /// survives (for example `10m` or `yes`). Only meaningful with
    /// [`Self::ssh_multiplex`].
    #[serde(default = "default_ssh_control_persist")]
    pub ssh_control_persist: String,
    /// Path to the sealed SSH credential vault (passwords and TOTP
    /// secrets for password/OTP-only clusters). The daemon unlocks it at
    /// startup with the `AUTONOMICS_SSH_PASSPHRASE` environment variable
    /// and serves answers to SSH prompts through the askpass helper.
    #[serde(default)]
    pub ssh_credential_vault: Option<std::path::PathBuf>,
}

fn default_ssh_control_persist() -> String {
    "10m".into()
}

/// Enable SSH connection multiplexing (mandatory for OTP clusters: a
/// one-time password authenticates exactly one connection, so every
/// operation must reuse a single master).
pub const ENV_SSH_MULTIPLEX: &str = "AUTONOMICS_SSH_MULTIPLEX";

fn default_ssh_credential_vault() -> Option<std::path::PathBuf> {
    std::env::var_os("AUTONOMICS_SSH_VAULT").map(std::path::PathBuf::from)
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::resolve(None)
    }
}

fn default_true() -> bool {
    true
}

/// Serde default for [`RuntimeConfig::agent_idle_ttl`]: 15 minutes.
fn default_agent_idle_ttl() -> Option<std::time::Duration> {
    Some(std::time::Duration::from_secs(15 * 60))
}

/// Serde default for [`RuntimeConfig::kms_db_path`]. Defensive only —
/// whole-`RuntimeConfig` deserialization does not occur in production
/// (agent persistence stores `AgentProfileConfig`). A serde default fn
/// cannot see sibling fields, so a legacy value would resolve against
/// `$HOME` instead of a custom `state_dir`.
pub(crate) fn default_kms_db_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| {
            PathBuf::from(h)
                .join(DEFAULT_STATE_DIR)
                .join(DEFAULT_KMS_DB)
        })
        .unwrap_or_else(|| PathBuf::from(DEFAULT_KMS_DB))
}

fn default_wiki_db_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| {
            PathBuf::from(h)
                .join(DEFAULT_STATE_DIR)
                .join(DEFAULT_WIKI_DB)
        })
        .unwrap_or_else(|| PathBuf::from(DEFAULT_WIKI_DB))
}

fn default_deck_db_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| {
            PathBuf::from(h)
                .join(DEFAULT_STATE_DIR)
                .join(DEFAULT_DECK_DB)
        })
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DECK_DB))
}

fn default_knowledge_db_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| {
            PathBuf::from(h)
                .join(DEFAULT_STATE_DIR)
                .join(DEFAULT_KNOWLEDGE_DB)
        })
        .unwrap_or_else(|| PathBuf::from(DEFAULT_KNOWLEDGE_DB))
}

impl RuntimeConfig {
    /// Whether this deployment admits local containers. A validated ordinary
    /// remote-only placement profile uses a rejecting local runtime instead of
    /// requiring a local Podman installation. No remote readiness is inferred.
    pub fn local_container_execution_required(&self) -> crate::Result<bool> {
        let mode = std::env::var("AUTONOMICS_EXECUTION_MODE").unwrap_or_default();
        let resolved = if mode.trim() == "formal" {
            self.clone()
        } else {
            crate::compute::startup_config(self).map_err(crate::error::Error::Other)?
        };
        crate::host::hpc_placement::requires_local_containers(&resolved, &mode, false)
    }

    /// Create a new [`RuntimeConfigBuilder`].
    pub fn builder() -> RuntimeConfigBuilder {
        RuntimeConfigBuilder::new()
    }

    /// Resolve all fields from environment variables + defaults.
    ///
    /// Pass `base` to overlay explicit values from a partially-filled
    /// builder; pass `None` for the pure env+default resolution.
    pub(crate) fn resolve(base: Option<&RuntimeConfigBuilder>) -> Self {
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

        let knowledge_db_path = base
            .and_then(|b| b.knowledge_db_path.clone())
            .or_else(|| env_path(ENV_KNOWLEDGE_DB))
            .unwrap_or_else(|| state_dir.join(DEFAULT_KNOWLEDGE_DB));

        let kms_db_path = base
            .and_then(|b| b.kms_db_path.clone())
            .or_else(|| env_path(ENV_KMS_DB))
            .unwrap_or_else(|| state_dir.join(DEFAULT_KMS_DB));
        let wiki_db_path = base
            .and_then(|b| b.wiki_db_path.clone())
            .or_else(|| env_path(ENV_WIKI_DB))
            .unwrap_or_else(|| state_dir.join(DEFAULT_WIKI_DB));

        let deck_db_path = base
            .and_then(|b| b.deck_db_path.clone())
            .or_else(|| env_path(ENV_DECK_DB))
            .unwrap_or_else(|| state_dir.join(DEFAULT_DECK_DB));
        let browser_download_dir = base
            .and_then(|b| b.browser_download_dir.clone())
            .or_else(|| env_path(ENV_BROWSER_DOWNLOAD_DIR));
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
            knowledge_db_path,
            kms_db_path,
            wiki_db_path,
            deck_db_path,
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
            browser_no_sandbox: resolve_flag(base, |b| b.browser_no_sandbox, false),
            browser_headless: resolve_flag(base, |b| b.browser_headless, true),
            browser_download_dir,
            use_memory: resolve_env_flag(base.and_then(|b| b.use_memory), ENV_USE_MEMORY)
                .unwrap_or(true),
            generate_memory: resolve_env_flag(
                base.and_then(|b| b.generate_memory),
                ENV_GENERATE_MEMORY,
            )
            .unwrap_or(true),
            enable_kms: resolve_flag(base, |b| b.enable_kms, false),
            embedding: base.and_then(|b| b.embedding.clone()).unwrap_or_default(),
            agent_idle_ttl: base
                .and_then(|b| b.agent_idle_ttl)
                .or_else(default_agent_idle_ttl),
            bib_http: resolve_bib_http(base),
            plugin_rsi: base.and_then(|b| b.plugin_rsi.clone()).unwrap_or_default(),
            downloader: base.and_then(|b| b.downloader.clone()).unwrap_or_default(),
            slurm: base.and_then(|b| b.slurm.clone()).unwrap_or_default(),
            ssh_multiplex: resolve_flag(base, |b| b.ssh_multiplex, false)
                || std::env::var(ENV_SSH_MULTIPLEX)
                    .is_ok_and(|value| value != "0" && value != "false"),
            ssh_control_persist: base
                .and_then(|b| b.ssh_control_persist.clone())
                .unwrap_or_else(default_ssh_control_persist),
            ssh_credential_vault: base
                .and_then(|b| b.ssh_credential_vault.clone())
                .or_else(default_ssh_credential_vault),
        }
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
             dag_history_db: {}, bib_db: {}, app_db: {}, knowledge_db: {}, kms_db: {}, \
             dag_history: {}, bib: {}, opengwas: {}, \
             opentargets: {}, gwascatalog: {}, chembl: {}, rcsb: {}, string: {}, kegg: {}, \
             browser_headless: {}, browser_no_sandbox: {}, \
             browser_download_dir: {}, \
             rsi_publisher: {} }}",
            self.name,
            self.data_dir.display(),
            self.state_dir.display(),
            self.dag_history_db.display(),
            self.bib_db_path.display(),
            self.app_db_path.display(),
            self.knowledge_db_path.display(),
            self.kms_db_path.display(),
            self.enable_dag_history,
            self.enable_bibliography,
            self.enable_opengwas,
            self.enable_opentargets,
            self.enable_gwascatalog,
            self.enable_chembl,
            self.enable_rcsb,
            self.enable_string,
            self.enable_kegg,
            self.browser_headless,
            self.browser_no_sandbox,
            self.browser_download_dir
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<state_dir>/browser-downloads".to_string()),
            self.plugin_rsi.publisher.enabled,
        )
    }
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
