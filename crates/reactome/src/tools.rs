//! Agent tool layer wrapping the Reactome SDK.
//!
//! Each tool maps to one client method. Wire them into an agent's toolset
//! via [`reactome_registrations`].

mod analysis;
mod database;
mod mapping;
mod participants;
mod pathways;
mod search;
mod species;

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolRegistration};

use crate::ReactomeClient;

pub(crate) fn json_err(error: crate::ReactomeError) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}

/// Build registrations for all Reactome tools.
///
/// Pass a shared [`ReactomeClient`] so every tool reuses the same HTTP
/// connection pool.
pub fn reactome_registrations(client: Arc<ReactomeClient>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(database::ReactomeDatabaseTool {
            client: client.clone(),
        }),
        R::from(species::ReactomeSpeciesTool {
            client: client.clone(),
        }),
        R::from(pathways::ReactomeTopPathwaysTool {
            client: client.clone(),
        }),
        R::from(pathways::ReactomePathwayDetailTool {
            client: client.clone(),
        }),
        R::from(mapping::ReactomeMappingTool {
            client: client.clone(),
        }),
        R::from(participants::ReactomeParticipantsTool {
            client: client.clone(),
        }),
        R::from(search::ReactomeSearchTool {
            client: client.clone(),
        }),
        R::from(analysis::ReactomeAnalysisTool {
            client: client.clone(),
        }),
    ]
}
