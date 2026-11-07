//! Hard-coded default paths and environment-variable names.

use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Default file-storage root — files downloaded by agent tools (OpenGWAS
/// summary stats, GWAS Catalog data, etc.) land here.
///
/// Override via builder `.data_dir(…)` or env `AUTONOMICS_DATA_DIR`.
/// Defaults to `$HOME/.autonomics/data` (absolute, independent of CWD).
pub(crate) const DEFAULT_DATA_DIR: &str = ".autonomics/data";

/// Default directory for agent-internal databases (DAG history).
///
/// Override via builder `.state_dir(…)` or env `AUTONOMICS_STATE_DIR`.
/// Defaults to `~/.autonomics` (absolute, independent of CWD).
pub(crate) const DEFAULT_STATE_DIR: &str = ".autonomics";

/// Default DAG history SQLite filename (relative to `state_dir`).
pub(crate) const DEFAULT_DAG_HISTORY_DB: &str = "dag-history.db";

/// Default agent persistence database filename (relative to `state_dir`).
pub(crate) const DEFAULT_AGENT_DB: &str = "agent.db";

/// Default persistent-memory knowledge database filename.
///
/// Override via builder `.knowledge_db_path(…)` or env `AUTONOMICS_KNOWLEDGE_DB`.
pub(crate) const DEFAULT_KNOWLEDGE_DB: &str = "knowledge.db";

/// Default KMS database filename (relative to `state_dir`).
///
/// Override via builder `.kms_db_path(…)` or env `AUTONOMICS_KMS_DB`.
pub(crate) const DEFAULT_KMS_DB: &str = "kms.db";
/// Default file name of the wiki knowledge db under the state dir.
pub(crate) const DEFAULT_WIKI_DB: &str = "wiki.db";
pub(crate) const DEFAULT_DECK_DB: &str = "deck.db";

/// Default bibliography database path.
///
/// Override via builder `.bib_db_path(…)` or env `AUTONOMICS_BIB_DB`.
pub(crate) const DEFAULT_BIB_DB: &str = "bib.db";

/// Default writing-system database path.
///
/// Override via builder `.writing_db_path(…)` or env `AUTONOMICS_WRITING_DB`.
pub(crate) const DEFAULT_WRITING_DB: &str = "writing.db";

/// Default TUI application database (model config, settings, etc.).
///
/// Override via builder `.app_db_path(…)` or env `AUTONOMICS_APP_DB`.
pub(crate) const DEFAULT_APP_DB: &str = "config.db";

/// Default agent identity string.
pub(crate) const DEFAULT_AGENT_IDENTITY: &str = "You are a biomedical research assistant \
     specializing in genomics, GWAS analysis, and literature mining.";

// ---------------------------------------------------------------------------
// Environment variable names
// ---------------------------------------------------------------------------

/// Env var overriding the file-storage root.
pub const ENV_DATA_DIR: &str = "AUTONOMICS_DATA_DIR";

/// Env var overriding the internal-state directory.
pub const ENV_STATE_DIR: &str = "AUTONOMICS_STATE_DIR";

/// Env var overriding the browser download directory.
pub const ENV_BROWSER_DOWNLOAD_DIR: &str = "AUTONOMICS_BROWSER_DOWNLOAD_DIR";

/// Env var overriding the bibliography DB path.
pub const ENV_BIB_DB: &str = "AUTONOMICS_BIB_DB";

/// Env var overriding the KMS knowledge DB path.
pub const ENV_KMS_DB: &str = "AUTONOMICS_KMS_DB";

/// Env var overriding the wiki knowledge DB path.
pub const ENV_WIKI_DB: &str = "AUTONOMICS_WIKI_DB";
pub const ENV_DECK_DB: &str = "AUTONOMICS_DECK_DB";

/// Env var overriding the persistent-memory knowledge DB path.
pub const ENV_KNOWLEDGE_DB: &str = "AUTONOMICS_KNOWLEDGE_DB";

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
/// Read a non-empty env var as a [`PathBuf`].
pub(crate) fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}
