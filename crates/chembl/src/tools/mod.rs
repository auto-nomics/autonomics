pub mod helpers;
pub mod molecule;
pub mod search;
pub mod target;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

use crate::ChEMBLClient;

pub(crate) use self::helpers::json_err;

pub fn chembl_registrations(client: Arc<ChEMBLClient>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as Registration;
    vec![
        Registration::from(search::SearchTool {
            client: client.clone(),
        }),
        Registration::from(molecule::MoleculeTool {
            client: client.clone(),
        }),
        Registration::from(target::TargetTool { client }),
    ]
}
