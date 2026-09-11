mod lookup;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

use crate::PubChemClient;

pub fn registrations(client: Arc<PubChemClient>) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(lookup::PubChemLookupTool { client })]
}
