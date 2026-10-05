//! Agent tool layer wrapping the OpenGWAS SDK.
//!
//! Table-fetching endpoints (associations, phewas, gwasinfo, gwasinfo_search,
//! variants_rsid, variants_chrpos, ld_clump, tophits) have been migrated to
//! DAG source nodes in `data-engine::nodes::source_opengwas`. The remaining
//! tools cover non-tabular operations:
//!
//! - `gwasinfo_count` — scalar dataset count
//! - `ld_matrix` — N×N LD matrix
//! - `download_files` — bulk file download to storage

pub mod download;
mod gwasinfo_count;
mod ld_matrix;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;
use vfs::OpendalFileStorage;

pub(crate) use self::helpers::json_err;
use crate::OpengwasClient;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

mod helpers {
    use agentik_core::tools::ToolError;

    use crate::error::OpengwasError;

    pub(crate) fn json_err(e: OpengwasError) -> ToolError {
        ToolError::ExecutionFailed {
            source: Box::new(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Build [`ToolRegistration`]s for the remaining OpenGWAS tools.
///
/// Table-fetching endpoints have been converted to DAG source nodes
/// (`source_opengwas_*`). The tools left here are:
/// `gwasinfo_count`, `ld_matrix`, and `download_files`.
///
/// Pass a shared [`OpengwasClient`] so every tool reuses the same HTTP
/// connection and SQLite cache, and a shared [`OpendalFileStorage`] for
/// file download operations.
pub fn opengwas_registrations(
    client: Arc<OpengwasClient>,
    storage: Arc<OpendalFileStorage>,
) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(gwasinfo_count::GwasinfoCountTool {
            client: client.clone(),
        }),
        R::from(ld_matrix::LdMatrixTool {
            client: client.clone(),
        }),
        R::from(download::DownloadFilesTool::new(client, storage)),
    ]
}

#[cfg(test)]
mod deprecation_schema_tests {
    use super::*;

    /// Survey T3: deprecated tool Inputs advertise `"deprecated": true` in
    /// the derived JSON Schema so tool-facing agents can route pipelines to
    /// the DAG nodes.
    #[test]
    fn table_fetcher_tools_advertise_deprecated_in_schema() {
        let storage = Arc::new(vfs::OpendalFileStorage::new_temp());
        let client = Arc::new(crate::OpengwasClient::new(None).unwrap());
        for registration in opengwas_registrations(client, storage) {
            let deprecated = registration
                .definition
                .input_schema
                .additional
                .get("deprecated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            match registration.definition.name.as_str() {
                "opengwas_gwasinfo_count" | "opengwas_ld_matrix" => {
                    assert!(
                        deprecated,
                        "{} must advertise deprecated in its schema",
                        registration.definition.name
                    );
                    assert!(
                        registration
                            .definition
                            .description
                            .contains("prefer the DAG node"),
                        "{} description must point at the node",
                        registration.definition.name
                    );
                }
                "opengwas_download_files" => {
                    assert!(!deprecated, "download_files has no node equivalent");
                }
                _ => {}
            }
        }
    }
}
