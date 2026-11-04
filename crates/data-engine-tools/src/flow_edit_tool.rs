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
        "processes": flow
            .processes()
            .iter()
            .map(|process| {
                let cardinality = match &process.cardinality {
                    dag_engine::Cardinality::Singleton => {
                        serde_json::json!({"type": "singleton"})
                    }
                    dag_engine::Cardinality::Fanout { source, axis } => serde_json::json!({
                        "type": "fanout",
                        "axis": axis,
                        "source": source,
                    }),
                };
                serde_json::json!({
                    "id": process.id,
                    "kind": process.kind,
                    "cardinality": cardinality,
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
    description = "Replace the session's stage-model workflow (the flow run by run_flow) \
                  with the given graph. Editing is whole-graph replacement — authored \
                  flows are process-granular and stay small (a 100-file pipeline is \
                  ~3 processes), so resending the complete graph is the intended \
                  editing model, unlike node-granular DAGs. The graph is validated \
                  atomically: an invalid submission is rejected and leaves the \
                  previous flow untouched. Rules: process ids unique and free of `#`; \
                  direct fanout->singleton is invalid — use op collect; no cycles."
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
                  (kind, cardinality, error strategy) and the wires, plus the full \
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
        let graph = serde_json::to_value(&flow)
            .map_err(|error| ToolError::ValidationFailed {
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
                        {"id": "hello", "kind": "echo", "spec": {},
                         "cardinality": {"type": "singleton"}}
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
                        {"id": "ok", "kind": "echo", "spec": {},
                         "cardinality": {"type": "singleton"}}
                    ],
                    "wires": []
                }),
            })
            .await
            .unwrap();
        // A direct fanout->singleton wire is invalid.
        let error = SetFlowTool::new(Arc::clone(&client))
            .run(SetFlowInput {
                flow: serde_json::json!({
                    "processes": [
                        {"id": "a", "kind": "echo", "spec": {},
                         "cardinality": {"type": "fanout", "axis": "x",
                                          "source": {"type": "items", "values": [1]}}},
                        {"id": "b", "kind": "echo", "spec": {},
                         "cardinality": {"type": "singleton"}}
                    ],
                    "wires": [
                        {"from": "a", "from_port": 0, "to": "b", "to_port": 0,
                         "op": "direct"}
                    ]
                }),
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("use a Collect wire"), "{error}");
        let view = ViewFlowTool::new(client).run(ViewFlowInput {}).await.unwrap();
        let agentik_sdk::types::ToolResultContent::Json(view) = view.content else {
            panic!("view_flow should be JSON");
        };
        assert_eq!(view["flow"]["processes"][0]["id"], "ok");
    }
}
