//! Tool assembly functions for the default agent configuration.
//!
//! Each function returns a [`Vec<ToolRegistration>`]. Compose them
//! to build custom tool sets.

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;
use data_engine::runtime::DataEngineClient;
use datalake::Datalake;
use eutils::EutilsClient;
use fs::OpendalFileStorage;
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

/// NCBI E-utilities tools (PubMed, Entrez).
pub fn eutils_tools() -> Vec<ToolRegistration> {
    let eutils = Arc::new(EutilsClient::from_env());
    eutils::eutils_registrations(eutils)
}

/// Open Targets Platform tools (target/disease/drug associations, search).
pub fn opentargets_tools() -> Vec<ToolRegistration> {
    let opentargets = Arc::new(OpenTargetsClient::new());
    opentargets::opentargets_registrations(opentargets)
}

/// Iceberg data-lake tools (query_iceberg).
pub fn datalake_tools(datalake: Arc<Datalake>) -> Vec<ToolRegistration> {
    datalake_tools::registrations(datalake)
}

/// The complete default tool set: File + OpenGWAS + E-utilities + Open Targets
/// + DataLake + DataEngine.
///
/// Pass a shared [`OpendalFileStorage`] used by both the fs tools
/// and the OpenGWAS download tool.
pub fn default_tool_set(
    file_storage: Arc<OpendalFileStorage>,
    datalake: Arc<Datalake>,
    data_engine_client: Arc<DataEngineClient>,
) -> Result<Vec<ToolRegistration>, OpengwasError> {
    let mut tools = fs::file_base_registrations(file_storage.clone());
    tools.extend(opengwas_tools(file_storage)?);
    tools.extend(eutils_tools());
    tools.extend(opentargets_tools());
    tools.extend(datalake_tools(datalake.clone()));
    tools.extend(data_engine_tools::registrations(data_engine_client));
    Ok(tools)
}
