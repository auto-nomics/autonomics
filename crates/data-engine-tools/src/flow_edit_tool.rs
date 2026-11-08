use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;
use serde_json::Value;

use crate::ExecError;

/// Shared summary of an authored flow — small by construction (one entry
/// per process), so the whole graph fits any context comfortably.
fn flow_summary(flow: &dag_engine::FlowGraph) -> Value {
    serde_json::json!({
        "sources": flow
            .sources()
            .iter()
            .map(|source| {
                serde_json::json!({"id": source.id, "spec": source.spec})
            })
            .collect::<Vec<_>>(),
        "processes": flow
            .processes()
            .iter()
            .map(|process| {
                serde_json::json!({
                    "id": process.id,
                    "kind": process.kind,
                    "error_strategy": process.error_strategy,
                })
            })
            .collect::<Vec<_>>(),
        "wires": flow
            .wires()
            .iter()
            .map(|wire| {
                serde_json::json!({
                    "from": wire.from,
                    "from_port": wire.from_port,
                    "to": wire.to,
                    "to_port": wire.to_port,
                    "op": wire.op,
                })
            })
            .collect::<Vec<_>>(),
    })
}

fn parse_flow(value: Value) -> Result<dag_engine::FlowGraph, ToolError> {
    serde_json::from_value(value).map_err(|error| ToolError::ValidationFailed {
        message: format!("invalid flow graph: {error}"),
    })
}

#[tool(
    name = "set_flow",
    description = "Replace the session workflow run by run_flow with the given graph. \
                  Validation is atomic: an invalid submission leaves the previous flow untouched. \
                  For incremental transactional edits use dag_shell. During execution, sources \
                  emit items incrementally, queue inputs pair by arrival, value inputs broadcast, \
                  and collect emits a value when a nonempty input closes. \
                  No cycles; ids unique and free of `#`."
)]
pub struct SetFlowInput {
    /// The complete flow graph (processes + wires arrays).
    pub flow: Value,
}

pub struct SetFlowTool {
    client: Arc<DataEngineClient>,
}

impl SetFlowTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for SetFlowTool {
    type Input = SetFlowInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let flow = parse_flow(input.flow)?;
        self.client.set_flow(flow).await.map_err(ExecError::from)?;
        let stored = self.client.get_flow().await.map_err(ExecError::from)?;
        Ok(ToolResult::success_json(serde_json::json!({
            "stored": true,
            "flow": flow_summary(&stored),
        })))
    }
}

#[tool(
    name = "view_flow",
    description = "Read the session's current stage-model workflow: the process list \
                  (kind and error strategy), sources and wires, plus the full \
                  graph JSON under `graph` for exact re-editing. Pair with set_flow \
                  for edit cycles and run_flow (omit `flow`) to execute it."
)]
pub struct ViewFlowInput {}

pub struct ViewFlowTool {
    client: Arc<DataEngineClient>,
}

impl ViewFlowTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for ViewFlowTool {
    type Input = ViewFlowInput;

    async fn run(&self, _input: Self::Input) -> Result<ToolResult, ToolError> {
        let flow = self.client.get_flow().await.map_err(ExecError::from)?;
        let empty = flow.is_empty();
        let graph = serde_json::to_value(&flow).map_err(|error| ToolError::ValidationFailed {
            message: format!("cannot serialize flow: {error}"),
        })?;
        Ok(ToolResult::success_json(serde_json::json!({
            "empty": empty,
            "flow": flow_summary(&flow),
            "graph": graph,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn set_view_run_cycle_uses_the_session_flow() {
        let engine = data_engine::data_engine::DataEngine::builder().build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let client = Arc::new(client);

        // Initially empty.
        let view = ViewFlowTool::new(Arc::clone(&client))
            .run(ViewFlowInput {})
            .await
            .unwrap();
        let agentik_sdk::types::ToolResultContent::Json(view) = view.content else {
            panic!("view_flow should be JSON");
        };
        assert_eq!(view["empty"], true);

        // Set, then read back.
        SetFlowTool::new(Arc::clone(&client))
            .run(SetFlowInput {
                flow: serde_json::json!({
                    "processes": [
                        {"id": "hello", "kind": "echo", "spec": {}}
                    ],
                    "wires": []
                }),
            })
            .await
            .unwrap();
        let view = ViewFlowTool::new(Arc::clone(&client))
            .run(ViewFlowInput {})
            .await
            .unwrap();
        let agentik_sdk::types::ToolResultContent::Json(view) = view.content else {
            panic!("view_flow should be JSON");
        };
        assert_eq!(view["empty"], false);
        assert_eq!(view["flow"]["processes"][0]["id"], "hello");
        assert_eq!(view["flow"]["processes"][0]["kind"], "echo");

        // run_flow with no graph runs the stored flow.
        let run = crate::run_flow_tool::RunFlowTool::new(Arc::clone(&client))
            .run(crate::run_flow_tool::RunFlowInput {
                flow: None,
                max_concurrency: None,
                resume: None,
            })
            .await
            .unwrap();
        let agentik_sdk::types::ToolResultContent::Json(run) = run.content else {
            panic!("run_flow should be JSON");
        };
        assert_eq!(run["ok"], true, "{run}");
        assert_eq!(run["processes"]["hello"]["succeeded"], 1, "{run}");
    }

    #[tokio::test]
    async fn invalid_submission_leaves_the_previous_flow_intact() {
        let engine = data_engine::data_engine::DataEngine::builder().build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let client = Arc::new(client);
        SetFlowTool::new(Arc::clone(&client))
            .run(SetFlowInput {
                flow: serde_json::json!({
                    "processes": [
                        {"id": "ok", "kind": "echo", "spec": {}}
                    ],
                    "wires": []
                }),
            })
            .await
            .unwrap();
        // A wire cannot target a source.
        let error = SetFlowTool::new(Arc::clone(&client))
            .run(SetFlowInput {
                flow: serde_json::json!({
                    "sources": [
                        {"id": "reads", "spec": {"type": "items", "values": ["x"]}}
                    ],
                    "processes": [
                        {"id": "a", "kind": "echo", "spec": {}}
                    ],
                    "wires": [
                        // Invalid: a wire may not target a source (sources
                        // only produce).
                        {"from": "a", "from_port": 0, "to": "reads", "to_port": 0,
                         "op": "direct"}
                    ]
                }),
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("must be a process"), "{error}");
        let view = ViewFlowTool::new(client)
            .run(ViewFlowInput {})
            .await
            .unwrap();
        let agentik_sdk::types::ToolResultContent::Json(view) = view.content else {
            panic!("view_flow should be JSON");
        };
        assert_eq!(view["flow"]["processes"][0]["id"], "ok");
    }
    #[tokio::test]
    async fn all_flow_entrypoints_reject_invalid_channel_graph_without_replacing_session() {
        let (client, _handle) = data_engine::runtime::spawn_with_engine(
            data_engine::data_engine::DataEngine::builder().build(),
        );
        let valid: dag_engine::FlowGraph = serde_json::from_value(serde_json::json!({
            "processes":[{"id":"saved","kind":"echo","spec":{}}], "wires":[]
        }))
        .unwrap();
        client.set_flow(valid).await.unwrap();
        let invalid: dag_engine::FlowGraph = serde_json::from_value(serde_json::json!({
            "sources":[{"id":"a","spec":{"type":"items","values":[1]}}, {"id":"b","spec":{"type":"items","values":[2]}}],
            "processes":[{"id":"join","kind":"channel","spec":{"operator":"join"}}],
            "wires":[{"from":"a","from_port":0,"to":"join","to_port":0,"op":"direct"}, {"from":"b","from_port":0,"to":"join","to_port":0,"op":"direct"}]
        })).unwrap();
        assert!(client.set_flow(invalid.clone()).await.is_err());
        assert!(
            client
                .run_flow(Some(invalid), data_engine::flow::FlowOptions::default())
                .await
                .is_err()
        );
        assert_eq!(client.get_flow().await.unwrap().processes()[0].id, "saved");
    }
}
