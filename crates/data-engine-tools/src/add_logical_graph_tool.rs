use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::dag::LogicalGraph;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "add_logical_graph",
    description = "Install a self-contained logical DAG subgraph with typed Channel operators and \
                  logical execution strategies. This is the supported way for an agent to control \
                  dataflow operators such as of_items, map, filter, flatten, mix, collect, \
                  combine, join, group_tuple, branch, gather, for_each, and dynamic_for_each. \
                  The graph is compiled to physical jobs and becomes visible to view_dag/run_dag. \
                  \
                  A minimal map example: \
                  {\"nodes\":[\
                    {\"id\":\"items\",\"definition\":{\"Channel\":{\"operator\":\"of_items\",\"items\":[{\"id\":\"a\"},{\"id\":\"b\"}]}},\"strategy\":\"Once\"},\
                    {\"id\":\"mapped\",\"definition\":{\"Channel\":{\"operator\":\"map\",\"template\":{\"name\":\"{{item.id}}\"}}},\"strategy\":\"Once\"}\
                  ],\"edges\":[{\"from\":\"items\",\"from_port\":0,\"to\":\"mapped\",\"to_port\":0}]} \
                  \
                  For dynamic fanout, use a Registry node with strategy \
                  {\"DynamicForEach\":{\"axis\":\"sample\"}} and follow it with channel.collect or \
                  a gather node. Registry node specs use the same kinds/schemas exposed by \
                  list_node_factories/get_node_spec."
)]
pub struct AddLogicalGraphInput {
    /// Self-contained typed logical graph. All referenced node ids must be declared in this graph.
    pub graph: LogicalGraph,
}

pub struct AddLogicalGraphTool {
    client: Arc<DataEngineClient>,
}

impl AddLogicalGraphTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for AddLogicalGraphTool {
    type Input = AddLogicalGraphInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let report = self
            .client
            .add_logical_graph(input.graph)
            .await
            .map_err(ExecError::from)?;

        let response = serde_json::to_value(&report).map_err(|error| {
            ExecError::Format(format!("cannot serialize logical graph report: {error}"))
        })?;
        Ok(ToolResult::success_json(response))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agentik_core::tools::ToolFunction;
    use agentik_sdk::types::ToolInput;

    use super::*;

    fn test_graph() -> serde_json::Value {
        serde_json::json!({
            "nodes": [
                {
                    "id": "items",
                    "definition": {
                        "Channel": {
                            "operator": "of_items",
                            "items": [{"id": "one"}, {"id": "two"}]
                        }
                    },
                    "strategy": "Once"
                },
                {
                    "id": "mapped",
                    "definition": {
                        "Channel": {
                            "operator": "map",
                            "template": {"name": "{{item.id}}"}
                        }
                    },
                    "strategy": "Once"
                }
            ],
            "edges": [
                {"from": "items", "from_port": 0, "to": "mapped", "to_port": 0}
            ]
        })
    }

    #[test]
    fn generated_schema_accepts_channel_graph() {
        let raw = test_graph();
        let input: super::AddLogicalGraphInput = serde_json::from_value(serde_json::json!({
            "graph": raw
        }))
        .unwrap();
        assert_eq!(input.graph.nodes().len(), 2);
        super::AddLogicalGraphInput::definition()
            .validate_input(&serde_json::json!({"graph": test_graph()}))
            .unwrap();
    }

    #[tokio::test]
    async fn installs_and_runs_channel_graph_for_agent() {
        let engine = data_engine::data_engine::DataEngine::builder().build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let tool = super::AddLogicalGraphTool::new(Arc::new(client));
        let input: super::AddLogicalGraphInput = serde_json::from_value(serde_json::json!({
            "graph": test_graph()
        }))
        .unwrap();

        let result = tool.run(input).await.unwrap();
        let agentik_sdk::types::ToolResultContent::Json(report) = result.content else {
            panic!("logical graph result should be JSON");
        };
        assert_eq!(report["logical_node_count"], 2);
        assert!(report["jobs"].as_object().unwrap().contains_key("items#0"));

        let report = tool
            .client
            .run_dag(Some("agent-channel-graph".to_string()))
            .await
            .unwrap();
        assert!(report.ok, "report errors: {:?}", report.errors);
        assert!(report.statuses.contains_key("items#0"));
        assert!(report.statuses.contains_key("mapped#0"));
    }

    #[tokio::test]
    async fn registry_nodes_cannot_bypass_disabled_kinds() {
        let engine = data_engine::data_engine::DataEngine::builder().build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let tool = super::AddLogicalGraphTool::new(Arc::new(client));
        let graph = data_engine::dag::LogicalGraph::builder()
            .add_node(data_engine::dag::LogicalNode::registry(
                "generic_container",
                "container_command",
                serde_json::json!({}),
            ))
            .build();

        let error = tool
            .run(super::AddLogicalGraphInput { graph })
            .await
            .expect_err("disabled node kinds must not become reachable through logical graphs");

        assert!(
            error.to_string().contains("container_command"),
            "unexpected error: {error}"
        );
    }
}
