mod lookup;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

use crate::AlphaFoldClient;

pub fn registrations(client: Arc<AlphaFoldClient>) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(lookup::AlphaFoldLookupTool {
        client,
    })]
}
