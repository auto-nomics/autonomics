//! Agent tool layer for concise Ensembl previews and summaries.

mod info;
mod lookup;
mod overlap;
mod sequence;
mod variation;
mod vep;
mod xrefs;

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolRegistration};

use crate::EnsemblClient;

pub(crate) fn json_err(error: crate::EnsemblError) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}

/// Build registrations for all Ensembl tools.
///
/// All tools share one client and connection pool.
pub fn ensembl_registrations(client: Arc<EnsemblClient>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as Registration;
    vec![
        Registration::from(info::EnsemblSpeciesTool {
            client: client.clone(),
        }),
        Registration::from(info::EnsemblAssemblyTool {
            client: client.clone(),
        }),
        Registration::from(lookup::EnsemblLookupTool {
            client: client.clone(),
        }),
        Registration::from(sequence::EnsemblSequenceTool {
            client: client.clone(),
        }),
        Registration::from(xrefs::EnsemblXrefsTool {
            client: client.clone(),
        }),
        Registration::from(overlap::EnsemblOverlapTool {
            client: client.clone(),
        }),
        Registration::from(vep::EnsemblVepTool {
            client: client.clone(),
        }),
        Registration::from(variation::EnsemblVariationTool { client }),
    ]
}
