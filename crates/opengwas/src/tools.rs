//! Agent tool layer wrapping the OpenGWAS SDK.
//!
//! Table-fetching endpoints (associations, phewas, gwasinfo,
//! gwasinfo_search, variants_rsid, variants_chrpos, ld_clump, tophits)
//! were migrated to DAG source nodes (`source_opengwas_*`) in
//! `nodes-opengwas`; the scalar and matrix operations (`gwasinfo_count`,
//! `ld_matrix`) followed them in the M3-② spike — scalar as a single-row
//! DataFrame, matrix as a long-format table — and their tool layers are
//! removed. The one tool left here has no node-side equivalent:
//!
//! - `download_files` — authenticated bulk summary-statistics file
//!   download to storage (the OpenGWAS API requires a token; plain
//!   `http_fetch` cannot substitute).

pub mod download;

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

/// Build [`ToolRegistration`]s for the remaining OpenGWAS tool.
///
/// Pass a shared [`OpengwasClient`] so the tool reuses the same HTTP
/// connection and SQLite cache, and a shared [`OpendalFileStorage`] for
/// file download operations.
pub fn opengwas_registrations(
    client: Arc<OpengwasClient>,
    storage: Arc<OpendalFileStorage>,
) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![R::from(download::DownloadFilesTool::new(client, storage))]
}

#[cfg(test)]
mod deregistration_tests {
    use super::*;

    /// The migration's deregistration contract: every superseded OpenGWAS
    /// tool is absent from the registration surface, and the one surviving
    /// tool is the file download with no node equivalent.
    #[test]
    fn only_download_files_remains_registered() {
        let storage = Arc::new(vfs::OpendalFileStorage::new_temp());
        let client = Arc::new(crate::OpengwasClient::new(None).unwrap());
        let names: Vec<_> = opengwas_registrations(client, storage)
            .into_iter()
            .map(|registration| registration.definition.name)
            .collect();
        assert_eq!(names, ["opengwas_download_files"]);
    }
}
