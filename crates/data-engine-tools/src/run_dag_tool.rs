use std::sync::Arc;

use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::dag::RunReport;
use data_engine::dag::node_event::{NodeEvent, NodeEventKind};
use data_engine::runtime::DataEngineClient;
use serde_json::Value;

use agentik_core::tools::{ToolContext, ToolError, ToolFunction};
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

/// Render a live [`NodeEvent`] as a single human-readable line for the task's
/// accumulated output (what `view_task_status` surfaces while `run_dag` runs).
/// Returns an empty string for events that should not be surfaced.
fn format_node_event(ev: &NodeEvent) -> String {
    let id = &ev.node_id;
    match &ev.kind {
        NodeEventKind::Status { status } => {
            format!("node {id}: {}", format!("{status:?}").to_lowercase())
        }
        NodeEventKind::Progress { current, total } => {
            format!("node {id}: progress {current}/{total}")
        }
        NodeEventKind::Log { level, message } => {
            format!("node {id} [{level:?}]: {message}")
        }
        NodeEventKind::Finished { status, elapsed_ms } => {
            format!(
                "node {id}: {} ({elapsed_ms}ms)",
                format!("{status:?}").to_lowercase()
            )
        }
        // Done carries the heavy output map and is never emitted on the
        // external sink — nothing to render.
        NodeEventKind::Done(_) => String::new(),
    }
}

/// Convert a finished [`RunReport`] into the structured JSON returned to the
/// agent. Shared by both the streaming and non-streaming execution paths so
/// their results are identical.
fn build_report_json(report: RunReport) -> serde_json::Value {
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

    serde_json::json!({
        "ok": report.ok,
        "summary": {
            "total": total,
            "succeeded": succeeded,
            "failed": failed,
            "skipped": skipped,
        },
        "nodes": nodes,
    })
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
        // Non-streaming path (used when called directly, not via the toolset).
        let report = self.client.run_dag().await.map_err(ExecError::from)?;
        Ok(ToolResult::success_json(build_report_json(report)))
    }

    /// Streaming entry point invoked by the toolset. Drains live per-node
    /// events into the task's output channel (so `view_task_status` can show
    /// node status/elapsed/progress while a long DAG runs) while awaiting the
    /// final [`RunReport`]. The returned JSON is identical to [`run`].
    async fn execute_with_context(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        // `RunDagInput` carries no parameters; validate it parses (mirrors the
        // default `execute` deserialize contract we bypass by overriding here).
        let _ = serde_json::from_value::<Self::Input>(input)?;

        let (mut event_rx, reply_rx) = self.client.run_dag_stream();
        // Pin the reply future so it can be polled across loop iterations.
        tokio::pin!(reply_rx);

        let report = loop {
            tokio::select! {
                ev = event_rx.recv() => {
                    if let Some(ev) = ev {
                        let line = format_node_event(&ev);
                        if !line.is_empty() {
                            ctx.emit_line(line);
                        }
                    }
                }
                res = &mut reply_rx => {
                    break res
                        .map_err(|_| ExecError::from("data engine closed the reply channel".to_string()))?
                        .map_err(data_engine::runtime::error::ClientError::from)
                        .map_err(ExecError::from)?;
                }
            }
        };

        Ok(ToolResult::success_json(build_report_json(report)))
    }
}
