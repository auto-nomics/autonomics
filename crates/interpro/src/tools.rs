mod lookup;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

use crate::InterProClient;

pub fn registrations(client: Arc<InterProClient>) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(lookup::InterProLookupTool {
        client,
    })]
}
