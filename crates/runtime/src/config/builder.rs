//! Builder overlay for [`RuntimeConfig`](super::RuntimeConfig).

use std::path::PathBuf;

use bib_base::BibHttpOptions;

use super::rsi::PluginRsiConfig;
use super::runtime::RuntimeConfig;

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
    pub(crate) knowledge_db_path: Option<PathBuf>,
    pub(crate) kms_db_path: Option<PathBuf>,
    pub(crate) wiki_db_path: Option<PathBuf>,
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
    pub(crate) browser_no_sandbox: Option<bool>,
    pub(crate) browser_headless: Option<bool>,
    pub(crate) browser_download_dir: Option<PathBuf>,
    pub(crate) use_memory: Option<bool>,
    pub(crate) generate_memory: Option<bool>,
    pub(crate) enable_kms: Option<bool>,
    pub(crate) embedding: Option<text_embedding::EmbeddingConfig>,
    pub(crate) agent_idle_ttl: Option<std::time::Duration>,
    pub(crate) bib_http: Option<BibHttpOptions>,
    pub(crate) plugin_rsi: Option<PluginRsiConfig>,
    pub(crate) slurm: Option<super::slurm::SlurmConfig>,
    pub(crate) downloader: Option<resource_downloader::DownloaderConfig>,
}

impl RuntimeConfigBuilder {
    pub(crate) fn new() -> Self {
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

    /// Path to the persistent-memory knowledge database.
    pub fn knowledge_db_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.knowledge_db_path = Some(path.into());
        self
    }

    /// Path to the KMS knowledge database (Turso/SQLite).
    pub fn kms_db_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.kms_db_path = Some(path.into());
        self
    }

    /// Override the wiki knowledge DB path.
    pub fn wiki_db_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.wiki_db_path = Some(path.into());
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

    /// Pass `--no-sandbox` to the Chromium process that browser-kind
    /// agents spawn. Has no effect on other agent kinds.
    pub fn browser_no_sandbox(mut self, enabled: bool) -> Self {
        self.browser_no_sandbox = Some(enabled);
        self
    }

    /// Run browser-kind agents with a visible window (default off). Use
    /// for local debugging only; server / daemon agents should stay
    /// headless.
    pub fn browser_headless(mut self, headless: bool) -> Self {
        self.browser_headless = Some(headless);
        self
    }

    /// Use a custom Chromium download directory.
    pub fn browser_download_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.browser_download_dir = Some(dir.into());
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

    pub fn embedding(mut self, config: text_embedding::EmbeddingConfig) -> Self {
        self.embedding = Some(config);
        self
    }

    /// Override the virtual-actor idle passivation TTL. Agents idle
    /// longer than this are passivated by the host (persist + evict);
    /// the next message re-activates them from storage.
    pub fn agent_idle_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.agent_idle_ttl = Some(ttl);
        self
    }

    /// Override the bibliography HTTP client configuration. Merged on
    /// top of any env-var defaults already resolved by
    /// [`RuntimeConfig::resolve`].
    pub fn bib_http(mut self, opts: BibHttpOptions) -> Self {
        self.bib_http = Some(opts);
        self
    }

    /// Configure the RSI plugin subsystem.
    pub fn plugin_rsi(mut self, config: PluginRsiConfig) -> Self {
        self.plugin_rsi = Some(config);
        self
    }

    /// Configure the Slurm (HPC) integration.
    pub fn slurm(mut self, config: super::slurm::SlurmConfig) -> Self {
        self.slurm = Some(config);
        self
    }

    /// Configure the central resource downloader.
    pub fn downloader(mut self, config: resource_downloader::DownloaderConfig) -> Self {
        self.downloader = Some(config);
        self
    }

    /// Resolve into a fully-resolved [`RuntimeConfig`].
    pub fn build(self) -> RuntimeConfig {
        RuntimeConfig::resolve(Some(&self))
    }
}
