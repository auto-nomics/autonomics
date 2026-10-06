use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::{ToolResult, ToolResultContent};
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "dag_shell",
    description = "Build or modify several DAG nodes, edges, and logical subgraphs in one \
                  transactional Rhai script. Use node(id, kind, spec), update_node, remove_node, \
                  edge, remove_edge, and add_logical_graph; all modifications must be followed by \
                  commit(). Queries see the DAG snapshot from script start. Dry-run validates the \
                  plan without applying it. The shell cannot read files, access the network or \
                  environment, inspect output data, or execute the DAG; call run_dag separately."
)]
pub struct DagShellInput {
    /// Rhai graph-orchestration script. Use #{} for JSON-like specs/maps.
    pub script: String,
    /// Validate and return the staged operation plan without applying it.
    pub dry_run: Option<bool>,
    /// Execution timeout in milliseconds (default 1000, maximum 5000).
    pub timeout_ms: Option<u64>,
}

pub struct DagShellTool {
    client: Arc<DataEngineClient>,
}

impl DagShellTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for DagShellTool {
    type Input = DagShellInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let outcome = self
            .client
            .run_dag_shell(
                input.script,
                input.dry_run.unwrap_or(false),
                input.timeout_ms,
            )
            .await
            .map_err(ExecError::from)?;
        let response = serde_json::to_value(&outcome).map_err(|error| {
            ExecError::Format(format!("cannot serialize dag_shell result: {error}"))
        })?;
        let mut result = ToolResult::success_json(response);
        if !outcome.ok {
            result.is_error = Some(true);
            result.content = ToolResultContent::Json(serde_json::json!(&outcome));
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(script: &str, dry_run: bool) -> DagShellInput {
        DagShellInput {
            script: script.to_string(),
            dry_run: Some(dry_run),
            timeout_ms: Some(1000),
        }
    }

    #[tokio::test]
    async fn applies_loop_and_returns_trace() {
        let engine = data_engine::data_engine::DataEngine::builder().build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let tool = DagShellTool::new(Arc::new(client.clone()));
        let result = tool
            .run(input(
                r#"
                for sample in ["a", "b"] {
                    let id = "echo_" + sample;
                    node(id, "echo", #{});
                }
                edge("echo_a", 0, "echo_b", 0);
                commit();
                "done"
                "#,
                false,
            ))
            .await
            .unwrap();
        let ToolResultContent::Json(report) = result.content else {
            panic!("dag_shell result should be JSON");
        };
        assert_eq!(report["ok"], true);
        assert_eq!(report["applied"], true);
        assert_eq!(report["graph"]["node_count"], 2);
        assert_eq!(report["operations"][0]["id"], "echo_a");

        let run = client
            .run_dag(Some("dag-shell-built".into()))
            .await
            .unwrap();
        assert!(run.ok, "run errors: {:?}", run.errors);
    }

    #[tokio::test]
    async fn failed_second_operation_leaves_first_behind() {
        let engine = data_engine::data_engine::DataEngine::builder().build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let tool = DagShellTool::new(Arc::new(client.clone()));
        let result = tool
            .run(input(
                r#"
                node("first", "echo", #{});
                node("second", "not_a_registered_kind", #{});
                commit();
                "#,
                false,
            ))
            .await
            .unwrap();
        let ToolResultContent::Json(report) = result.content else {
            panic!("dag_shell failure should be structured JSON");
        };
        assert_eq!(report["ok"], false);
        assert_eq!(report["applied"], false);
        assert_eq!(report["error"]["code"], "unknown_node_kind");
        assert_eq!(report["error"]["operation_index"], 1);
        assert_eq!(report["graph"]["node_count"], 0);
    }

    #[tokio::test]
    async fn dry_run_and_missing_commit_do_not_apply() {
        let engine = data_engine::data_engine::DataEngine::builder().build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let tool = DagShellTool::new(Arc::new(client.clone()));
        let dry = tool
            .run(input(r#"node("dry", "echo", #{}); commit(); "#, true))
            .await
            .unwrap();
        let ToolResultContent::Json(report) = dry.content else {
            panic!("dry-run result should be JSON");
        };
        assert_eq!(report["applied"], false);
        assert_eq!(report["graph"]["node_count"], 1);
        assert_eq!(client.node_exists("dry".into()).await.unwrap(), false);

        let uncommitted = tool
            .run(input(r#"node("not_committed", "echo", #{}); "#, false))
            .await
            .unwrap();
        let ToolResultContent::Json(report) = uncommitted.content else {
            panic!("uncommitted result should be JSON");
        };
        assert_eq!(report["committed"], false);
        assert_eq!(
            client.node_exists("not_committed".into()).await.unwrap(),
            false
        );
    }

    #[tokio::test]
    async fn installs_channel_logical_graph() {
        let engine = data_engine::data_engine::DataEngine::builder().build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let tool = DagShellTool::new(Arc::new(client));
        let result = tool
            .run(input(
                r#"
                add_logical_graph(#{
                    nodes: [
                        #{id: "items", definition: #{Channel: #{operator: "of_items", items: [#{id: "a"}]}}, strategy: "Once"},
                        #{id: "mapped", definition: #{Channel: #{operator: "map", template: #{name: "{{item.id}}"}}}, strategy: "Once"}
                    ],
                    edges: [#{from: "items", from_port: 0, to: "mapped", to_port: 0}]
                });
                commit();
                "#,
                false,
            ))
            .await
            .unwrap();
        let ToolResultContent::Json(report) = result.content else {
            panic!("logical graph result should be JSON");
        };
        assert_eq!(report["ok"], true, "{report}");
        assert_eq!(report["graph"]["logical_graph_count"], 1);
    }
}
