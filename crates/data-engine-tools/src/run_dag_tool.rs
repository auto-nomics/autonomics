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
                  row counts, timing, sink paths, and error/skip details. \
                  A snapshot of the DAG is automatically committed to the \
                  history store — provide a descriptive commit message for \
                  easy retrieval via dag_history_log."
)]
pub struct RunDagInput {
    /// A short, human-readable description of what this run does (e.g. \
    /// "initial LDSC h² on BMI", "added sex covariate"). Stored as the \
    /// snapshot message so dag_history_log shows meaningful entries. \
    /// If omitted, a generic default is used.
    #[serde(default)]
    pub commit_message: Option<String>,
}

pub struct RunDagTool {
    client: Arc<DataEngineClient>,
}

impl RunDagTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

/// Map a live [`NodeEvent`] to a structured [`ProgressRecord`] pushed onto the
/// task's live-output buffer (what `view_task_status` returns as JSON). Returns
/// `None` for events that should not be surfaced (the heavy internal `Done`).
fn node_event_to_record(ev: &NodeEvent) -> Option<agentik_core::tools::ProgressRecord> {
    use agentik_core::tools::ProgressRecord;
    let label = ev.node_id.as_str();
    match &ev.kind {
        NodeEventKind::Status { status } => Some(
            ProgressRecord::new("status")
                .label(label)
                .status(format!("{status:?}").to_lowercase()),
        ),
        NodeEventKind::Progress { current, total } => Some(
            ProgressRecord::new("progress")
                .label(label)
                .current(*current)
                .total(*total),
        ),
        NodeEventKind::Log { level, message } => Some(
            ProgressRecord::new("log")
                .label(label)
                .level(format!("{level:?}").to_lowercase())
                .message(message),
        ),
        NodeEventKind::Finished { status, elapsed_ms } => Some(
            ProgressRecord::new("finished")
                .label(label)
                .status(format!("{status:?}").to_lowercase())
                .elapsed_ms(*elapsed_ms),
        ),
        // Done carries the heavy output map and is never emitted on the
        // external sink — nothing to record.
        NodeEventKind::Done(_) => None,
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

    /// Phase 1 threshold. Tool execution will convert from synchronous into asynchronus.
    fn sync_seconds(&self) -> u64 {
        1
    }

    // DAG execution can run long (full pipelines: LD scoring, fitting,
    // heavy joins). Override the default 300s phase-2 timeout so that a
    // genuinely long run is not killed prematurely.
    fn timeout_seconds(&self) -> u64 {
        60 * 60
    }

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        // Non-streaming path (used when called directly, not via the toolset).
        let report = self.client.run_dag().await.map_err(ExecError::from)?;
        let _ = input; // commit_message only used in streaming path
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
        let parsed = serde_json::from_value::<Self::Input>(input)?;

        let (mut event_rx, reply_rx) = self.client.run_dag_stream(parsed.commit_message);
        // Pin the reply future so it can be polled across loop iterations.
        tokio::pin!(reply_rx);

        let report = loop {
            tokio::select! {
                ev = event_rx.recv() => {
                    if let Some(ev) = ev {
                        if let Some(record) = node_event_to_record(&ev) {
                            ctx.emit(record);
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
