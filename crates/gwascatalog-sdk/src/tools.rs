//! Agent tool layer wrapping the GWAS Catalog SDK.
//!
//! The table-fetching endpoints (Solr search, curated REST studies /
//! associations / SNPs / EFO traits / unpublished submissions, and the
//! Summary Statistics API) have been migrated to DAG source nodes
//! (`source_gwascatalog_*`, registered by the io bundle) and their tool
//! layers removed. The one tool left here covers the operation with no
//! node-side equivalent in the paginated-JSON sense:
//!
//! - `download` — full per-study summary-statistics **files** from the
//!   HTTPS FTP mirror (hundreds of MiB). The `source_gwascatalog_download`
//!   node wraps the same fetch as a FileSet output; this tool remains for
//!   interactive, progress-streamed downloads outside a DAG.
//!
//! Wire into an agent's toolset via [`gwascatalog_registrations`].

pub mod download;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;
use vfs::OpendalFileStorage;

pub(crate) use self::helpers::json_err;
use crate::client::GwasCatalogClient;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

mod helpers {
    use agentik_core::tools::ToolError;

    use crate::error::GwasCatalogError;

    pub(crate) fn json_err(e: GwasCatalogError) -> ToolError {
        ToolError::ExecutionFailed {
            source: Box::new(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Build [`ToolRegistration`]s for the remaining GWAS Catalog tool.
///
/// Pass a shared [`GwasCatalogClient`] so the tool reuses the same HTTP
/// connection pool, and a shared [`OpendalFileStorage`] for the
/// summary-statistics file download.
pub fn gwascatalog_registrations(
    client: Arc<GwasCatalogClient>,
    storage: Arc<OpendalFileStorage>,
) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![R::from(download::DownloadSummaryStatsTool {
        client,
        storage,
    })]
}
