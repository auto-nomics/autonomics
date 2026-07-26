use std::sync::Arc;

use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;

use crate::ExecError;

#[tool(
    name = "run_dag",
    description = "Execute the current DAG pipeline. All nodes are validated \
                  and run according to their dependency order. Returns a \
                  detailed report with per-node status, type, output schema, \
                  row counts, timing, sink paths, and error/skip details."
)]
pub struct RunDagInput {}

pub struct RunDagTool {
    client: Arc<DataEngineClient>,
}

impl RunDagTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for RunDagTool {
    type Input = RunDagInput;

    // DAG execution can run long (full pipelines: LD scoring, fitting,
    // heavy joins). Override the default 300s phase-2 timeout so that a
    // genuinely long run is not killed prematurely.
    fn timeout_seconds(&self) -> u64 {
        3600
    }

    async fn run(&self, _input: Self::Input) -> Result<ToolResult, ToolError> {
        let report = self.client.run_dag().await.map_err(ExecError::from)?;

        // Build per-node entries.
        let nodes: Vec<serde_json::Value> = report
            .nodes
            .into_iter()
            .map(|nr| {
                let mut obj = serde_json::Map::new();
                obj.insert("id".into(), serde_json::json!(nr.id));
                obj.insert("status".into(), serde_json::json!(nr.status));
                obj.insert("node_type".into(), serde_json::json!(nr.node_type));

                if let Some(schema) = nr.output_schema {
                    obj.insert("output_schema".into(), serde_json::json!(schema));
                }
                if let Some(rows) = nr.output_rows {
                    obj.insert("output_rows".into(), serde_json::json!(rows));
                }
                if let Some(ms) = nr.elapsed_ms {
                    obj.insert("elapsed_ms".into(), serde_json::json!(ms));
                }
                if let Some(path) = nr.file_path {
                    obj.insert("file_path".into(), serde_json::json!(path));
                }
                if let Some(path) = nr.artifact_path {
                    obj.insert("artifact_path".into(), serde_json::json!(path));
                }
                if let Some(err) = nr.error {
                    obj.insert("error".into(), serde_json::json!(err));
                }
                if let Some(cause) = nr.skipped_because {
                    obj.insert("skipped_because".into(), serde_json::json!(cause));
                }

                serde_json::Value::Object(obj)
            })
            .collect();

        // Summary counts.
        let mut succeeded = 0usize;
        let mut failed = 0usize;
        let mut skipped = 0usize;
        for node in &nodes {
            let status = node.get("status").and_then(|v| v.as_str()).unwrap_or("");
            match status {
                "success" => succeeded += 1,
                "failed" => failed += 1,
                "skipped" => skipped += 1,
                _ => {}
            }
        }
        let total = nodes.len();

        let content = serde_json::json!({
            "ok": report.ok,
            "summary": {
                "total": total,
                "succeeded": succeeded,
                "failed": failed,
                "skipped": skipped,
            },
            "nodes": nodes,
        });

        // Always return structured JSON — the `ok` flag tells the agent the outcome.
        // Using success_json (not ToolResult::error) ensures agents always get
        // parseable JSON even on partial failure.
        Ok(ToolResult::success_json(content))
    }
}
