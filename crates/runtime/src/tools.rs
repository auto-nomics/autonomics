//! Tool assembly functions for the default agent configuration.
//!
//! Each function returns a [`Vec<ToolRegistration>`]. Compose them
//! to build custom tool sets, or use [`tool_set_from_config`] to let a
//! [`RuntimeConfig`](crate::config::RuntimeConfig) decide which tools to
//! enable.

use std::sync::Arc;

use writing_base::LatexEngine;

use agentik_core::tools::ToolRegistration;
use bib_base::{BibBase, LiteratureGateway};
use data_engine::runtime::DataEngineClient;
use fs::OpendalFileStorage;
use gwascatalog_sdk::GwasCatalogClient;
use opengwas::{OpengwasClient, OpengwasError};
use opentargets::OpenTargetsClient;

use crate::config::RuntimeConfig;

/// OpenGWAS tools (GWAS catalog lookup).
///
/// Returns [`OpengwasError`] if the OpenGWAS client cannot be constructed
/// (typically because `OPENGWAS_TOKEN` is unset).
pub fn opengwas_tools(
    file_storage: Arc<OpendalFileStorage>,
) -> Result<Vec<ToolRegistration>, OpengwasError> {
    opengwas_tools_with_token(file_storage, None)
}

/// OpenGWAS tools with an explicit API token. Pass `None` to read from
/// the `OPENGWAS_TOKEN` env var (same as [`opengwas_tools`]).
pub fn opengwas_tools_with_token(
    file_storage: Arc<OpendalFileStorage>,
    token: Option<&str>,
) -> Result<Vec<ToolRegistration>, OpengwasError> {
    let opengwas = Arc::new(OpengwasClient::new(token)?);
    Ok(opengwas::opengwas_registrations(opengwas, file_storage))
}

/// Open Targets Platform tools (target/disease/drug associations, search).
pub fn opentargets_tools() -> Vec<ToolRegistration> {
    let opentargets = Arc::new(OpenTargetsClient::new());
    opentargets::opentargets_registrations(opentargets)
}

/// GWAS Catalog tools (curated studies, associations, EFO traits, SNPs,
/// unpublished submissions, summary statistics, full summary-stats file
/// download, and Solr full-text search).
pub fn gwascatalog_tools(storage: Arc<OpendalFileStorage>) -> Vec<ToolRegistration> {
    let client = Arc::new(GwasCatalogClient::new());
    gwascatalog_sdk::gwascatalog_registrations(client, storage)
}

/// Default on-disk location for the bibliography database, mirroring the
/// TUI's `bib` subcommand default (`--db bib.db`). Overridable via the
/// `AUTONOMICS_BIB_DB` environment variable.
pub const DEFAULT_BIB_DB: &str = "bib.db";

/// Bibliography tools: literature search/fetch (`lit_search`, `lit_fetch`)
/// plus library management (`bib_save`, `bib_create_collection`, …, `bib_export`).
///
/// `db_path` selects the libSQL file backing [`BibBase`]; pass
/// [`DEFAULT_BIB_DB`] for the conventional location.
///
/// **Note**: this is the single-agent helper. In a multi-agent host,
/// open a [`bib_base::BibShared`] once and reuse its handles — see
/// [`crate::RuntimeHost`]. Each call here builds a fresh
/// `EutilsClient`, `ArxivClient`, `reqwest::Client` and
/// `EuropePmcClient`, which is fine for one agent but wasteful for
/// many.
pub async fn bib_tools(db_path: &str) -> Result<Vec<ToolRegistration>, bib_base::Error> {
    let bib = Arc::new(BibBase::open(db_path).await?);
    let gateway = Arc::new(LiteratureGateway::with_default_sources());
    Ok(bib_base::bib_all_registrations(bib, gateway, None))
}

/// Resolves the bibliography DB path: the `AUTONOMICS_BIB_DB` env var if set,
/// otherwise [`DEFAULT_BIB_DB`].
pub fn resolve_bib_db_path() -> String {
    std::env::var("AUTONOMICS_BIB_DB").unwrap_or_else(|_| DEFAULT_BIB_DB.to_string())
}

/// Default on-disk location for the writing-system database.
pub const DEFAULT_WRITING_DB: &str = "writing.db";

/// Writing tools: document management (doc_create, doc_list, …), editing
/// (doc_insert_section, doc_insert_block, …), citation management
/// (doc_add_citation, doc_check_citations, …), and compilation (doc_compile).
pub async fn writing_tools(db_path: &str) -> Result<Vec<ToolRegistration>, writing_base::Error> {
    let store = Arc::new(writing_base::WritingStore::open(db_path).await?);
    let engine: Arc<dyn writing_base::LatexEngine> = {
        let x = writing_base::XelatexEngine::new();
        if writing_base::LatexEngine::is_available(&x) {
            Arc::new(x)
        } else {
            Arc::new(writing_base::NullEngine)
        }
    };
    Ok(writing_base::writing_all_registrations(
        store,
        None,
        Some(engine),
    ))
}

/// Resolves the writing DB path: the `AUTONOMICS_WRITING_DB` env var if set,
/// otherwise [`DEFAULT_WRITING_DB`].
pub fn resolve_writing_db_path() -> String {
    std::env::var("AUTONOMICS_WRITING_DB").unwrap_or_else(|_| DEFAULT_WRITING_DB.to_string())
}

/// The complete default tool set: File + OpenGWAS + Open Targets
/// + GWAS Catalog + DataLake + DataEngine + Bibliography.
///
/// Pass a shared [`OpendalFileStorage`] used by both the fs tools
/// and the OpenGWAS download tool.
///
/// **Note**: prefer [`tool_set_from_config`] for new code — it respects
/// per-agent feature flags and token overrides.
pub async fn default_tool_set(
    file_storage: Arc<OpendalFileStorage>,
    data_engine_client: Arc<DataEngineClient>,
) -> Result<Vec<ToolRegistration>, DefaultToolSetError> {
    let cfg = RuntimeConfig::default();
    tool_set_from_config(file_storage, data_engine_client, &cfg).await
}

/// Build a tool set from a [`RuntimeConfig`], enabling/disabling each
/// tool group according to the config's feature flags and using the
/// config's token overrides.
pub async fn tool_set_from_config(
    file_storage: Arc<OpendalFileStorage>,
    data_engine_client: Arc<DataEngineClient>,
    config: &RuntimeConfig,
) -> Result<Vec<ToolRegistration>, DefaultToolSetError> {
    // Filesystem / shell tools — always enabled.
    let mut tools = fs::vbash_registrations(file_storage.clone());

    if config.enable_opengwas {
        match opengwas_tools_with_token(file_storage.clone(), config.opengwas_token.as_deref()) {
            Ok(opengwas) => tools.extend(opengwas),
            Err(e) => {
                eprintln!("[runtime] WARNING: OpenGWAS tools disabled (token error): {e}");
            }
        }
    }

    if config.enable_opentargets {
        tools.extend(opentargets_tools());
    }

    if config.enable_gwascatalog {
        tools.extend(gwascatalog_tools(file_storage));
    }

    tools.extend(data_engine_tools::registrations(data_engine_client));

    // Resource catalog tools — always enabled (read-only, no side effects).
    tools.extend(crate::resource_tools::registrations());

    if config.enable_bibliography {
        match bib_tools(&config.bib_db_path.to_string_lossy()).await {
            Ok(bib) => tools.extend(bib),
            Err(e) => {
                eprintln!("[runtime] WARNING: bibliography tools disabled: {e}");
            }
        }
    }

    if config.enable_writing {
        match writing_tools(&config.writing_db_path.to_string_lossy()).await {
            Ok(wt) => tools.extend(wt),
            Err(e) => {
                eprintln!("[runtime] WARNING: writing tools disabled: {e}");
            }
        }
    }

    Ok(tools)
}

/// Errors that can arise while assembling the default tool set.
#[derive(thiserror::Error, Debug)]
pub enum DefaultToolSetError {
    #[error(transparent)]
    Opengwas(#[from] OpengwasError),
    #[error(transparent)]
    Bib(#[from] bib_base::Error),
    #[error(transparent)]
    Writing(#[from] writing_base::Error),
}
