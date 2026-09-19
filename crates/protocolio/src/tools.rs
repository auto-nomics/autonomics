pub mod download;
pub mod materials;
pub mod protocol;
pub mod reagents;
pub mod search;
pub mod steps;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;
use vfs::OpendalFileStorage;

use crate::ProtocolioClient;

pub fn registrations(
    client: Arc<ProtocolioClient>,
    storage: Arc<OpendalFileStorage>,
) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(search::ProtocolioSearchTool {
            client: Arc::clone(&client),
        }),
        ToolRegistration::from(protocol::ProtocolioGetTool {
            client: Arc::clone(&client),
        }),
        ToolRegistration::from(steps::ProtocolioStepsTool {
            client: Arc::clone(&client),
        }),
        ToolRegistration::from(materials::ProtocolioMaterialsTool {
            client: Arc::clone(&client),
        }),
        ToolRegistration::from(reagents::ProtocolioReagentsTool {
            client: Arc::clone(&client),
        }),
        ToolRegistration::from(download::ProtocolioPdfTool { client, storage }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registrations_expose_six_readonly_tools() {
        let client = Arc::new(ProtocolioClient::from_token("test-token").unwrap());
        let storage = Arc::new(OpendalFileStorage::new_temp());
        let names: Vec<_> = registrations(client, storage)
            .into_iter()
            .map(|tool| tool.definition.name)
            .collect();
        for name in [
            "protocolio_search_protocols",
            "protocolio_get_protocol",
            "protocolio_get_steps",
            "protocolio_get_materials",
            "protocolio_search_reagents",
            "protocolio_download_pdf",
        ] {
            assert!(names.contains(&name.to_string()), "missing tool: {name}");
        }
        assert_eq!(names.len(), 6);
    }
}
