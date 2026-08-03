//! Tool assembly functions for the default agent configuration.
//!
//! Each function returns a [`Vec<ToolRegistration>`]. Compose them
//! to build custom tool sets.

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;
use bib_base::{BibBase, LiteratureGateway};
use data_engine::runtime::DataEngineClient;
use datalake::Datalake;
use fs::OpendalFileStorage;
use gwascatalog_sdk::GwasCatalogClient;
use opengwas::{OpengwasClient, OpengwasError};
use opentargets::OpenTargetsClient;

/// OpenGWAS tools (GWAS catalog lookup).
///
/// Returns [`OpengwasError`] if the OpenGWAS client cannot be constructed
/// (typically because `OPENGWAS_TOKEN` is unset).
pub fn opengwas_tools(
    file_storage: Arc<OpendalFileStorage>,
) -> Result<Vec<ToolRegistration>, OpengwasError> {
    let opengwas = Arc::new(OpengwasClient::new(None)?);
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

/// Iceberg data-lake tools (query_iceberg).
pub fn datalake_tools(datalake: Arc<Datalake>) -> Vec<ToolRegistration> {
    datalake_tools::registrations(datalake)
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
pub async fn bib_tools(db_path: &str) -> Result<Vec<ToolRegistration>, bib_base::Error> {
    let bib = Arc::new(BibBase::open(db_path).await?);
    let gateway = Arc::new(LiteratureGateway::with_default_sources());
    Ok(bib_base::bib_all_registrations(bib, gateway))
}

/// Resolves the bibliography DB path: the `AUTONOMICS_BIB_DB` env var if set,
/// otherwise [`DEFAULT_BIB_DB`].
pub fn resolve_bib_db_path() -> String {
    std::env::var("AUTONOMICS_BIB_DB").unwrap_or_else(|_| DEFAULT_BIB_DB.to_string())
}

/// The complete default tool set: File + OpenGWAS + Open Targets
/// + GWAS Catalog + DataLake + DataEngine + Bibliography.
///
/// Pass a shared [`OpendalFileStorage`] used by both the fs tools
/// and the OpenGWAS download tool.
pub async fn default_tool_set(
    file_storage: Arc<OpendalFileStorage>,
    datalake: Arc<Datalake>,
    data_engine_client: Arc<DataEngineClient>,
) -> Result<Vec<ToolRegistration>, DefaultToolSetError> {
    let mut tools = fs::vbash_registrations(file_storage.clone());
    tools.extend(opengwas_tools(file_storage.clone())?);
    tools.extend(opentargets_tools());
    tools.extend(gwascatalog_tools(file_storage));
    tools.extend(datalake_tools(datalake.clone()));
    tools.extend(data_engine_tools::registrations(data_engine_client));
    tools.extend(bib_tools(&resolve_bib_db_path()).await?);
    Ok(tools)
}

/// Errors that can arise while assembling the default tool set.
#[derive(thiserror::Error, Debug)]
pub enum DefaultToolSetError {
    #[error(transparent)]
    Opengwas(#[from] OpengwasError),
    #[error(transparent)]
    Bib(#[from] bib_base::Error),
}
