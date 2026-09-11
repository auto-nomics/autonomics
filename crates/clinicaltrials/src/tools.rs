mod lookup;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

use crate::ClinicalTrialsClient;

pub fn registrations(client: Arc<ClinicalTrialsClient>) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(lookup::ClinicalTrialsLookupTool {
        client,
    })]
}
