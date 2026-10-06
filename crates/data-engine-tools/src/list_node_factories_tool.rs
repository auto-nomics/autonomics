use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "list_node_factories",
    description = "List all registered node kinds (name + short description). \
                  Returns lightweight metadata only — no JSON Schema or port layout. \
                  To configure a node, first discover kinds here, then call \
                  get_node_spec (for parameters), get_node_ports (for wiring), \
                  and get_node_doc (for usage) with the chosen `kind`. Pass kind \
                  to diagnose whether the live engine registry (including manifest \
                  plugins loaded at engine startup) contains that exact node kind."
)]
pub struct ListNodeFactoriesInput {
    /// Exact node kind to diagnose. When omitted, every live registered kind
    /// is returned as a list.
    pub kind: Option<String>,
}

pub struct ListNodeFactoriesTool {
    client: Arc<DataEngineClient>,
}

impl ListNodeFactoriesTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for ListNodeFactoriesTool {
    type Input = ListNodeFactoriesInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let nodes = self.client.list_node_factories().map_err(ExecError::from)?;

        let kind = input
            .kind
            .as_deref()
            .map(str::trim)
            .filter(|kind| !kind.is_empty());
        if let Some(kind) = kind {
            let registered = nodes.iter().find(|node| node.kind == kind);
            let is_registered = registered.is_some();
            return Ok(ToolResult::success_json(serde_json::json!({
                "kind": kind,
                "registered": is_registered,
                "node": registered,
                "diagnostic": if is_registered {
                    "The live engine registry contains this kind."
                } else {
                    "The live engine registry does not contain this kind. Manifest \
                     plugins are loaded only at engine startup; check the engine's \
                     plugins root/HOME, sync the plugin, and restart the engine."
                },
                "available_kinds": nodes.iter().map(|node| node.kind.clone()).collect::<Vec<_>>(),
            })));
        }

        let content = serde_json::to_value(&nodes).map_err(|e| ToolError::ExecutionFailed {
            source: Box::new(e),
        })?;

        Ok(ToolResult::success_json(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_sdk::types::ToolResultContent;

    #[tokio::test]
    async fn kind_query_diagnoses_a_missing_live_factory() {
        let engine = data_engine::data_engine::DataEngine::builder().build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let tool = ListNodeFactoriesTool::new(Arc::new(client));

        let result = tool
            .run(ListNodeFactoriesInput {
                kind: Some("__missing_scan_node__".into()),
            })
            .await
            .unwrap();
        let ToolResultContent::Json(report) = result.content else {
            panic!("diagnostic result should be JSON");
        };

        assert_eq!(report["kind"], "__missing_scan_node__");
        assert_eq!(report["registered"], false);
        assert_eq!(report["node"], serde_json::Value::Null);
        assert!(
            report["diagnostic"]
                .as_str()
                .unwrap_or_default()
                .contains("restart the engine"),
            "{report}"
        );
        assert!(
            report["available_kinds"]
                .as_array()
                .is_some_and(|kinds| { kinds.iter().any(|kind| kind == "echo") }),
            "{report}"
        );
    }
}
