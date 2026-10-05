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
                  For file-producing nodes, use port_assignments (not \
                  output_files order) when wiring downstream ports. \
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
        NodeEventKind::Resource {
            usage_bytes,
            limit_bytes,
            usage_ratio,
            threshold_ratio,
        } => Some(
            ProgressRecord::new("resource")
                .label("memory")
                .current(*usage_bytes)
                .total(*limit_bytes)
                .message(format!(
                    "memory {:.1}% (cancel threshold {:.1}%)",
                    usage_ratio * 100.0,
                    threshold_ratio * 100.0
                )),
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
///
/// File-level fingerprints are stripped via [`to_value_without_field`] so the
/// agent-facing report stays slim without changing `FileRef`'s serde contract.
fn build_report_json(report: RunReport) -> serde_json::Value {
    let warnings = report.warnings;
    let snapshot_id = report.snapshot_id;

    // Build per-node entries.
    let nodes: Vec<serde_json::Value> = report
        .nodes
        .into_iter()
        .map(|nr| {
            let mut obj = serde_json::Map::new();
            obj.insert("id".into(), serde_json::json!(nr.id));
            obj.insert("status".into(), serde_json::json!(nr.status));
            obj.insert("node_type".into(), serde_json::json!(nr.node_type));
            if let Some(logical_node) = nr.logical_node {
                obj.insert("logical_node".into(), serde_json::json!(logical_node));
            }
            if let Some(physical_job_id) = nr.physical_job_id {
                obj.insert("physical_job_id".into(), serde_json::json!(physical_job_id));
            }
            if let Some(scatter_axis) = nr.scatter_axis {
                obj.insert("scatter_axis".into(), serde_json::json!(scatter_axis));
            }
            if let Some(item_key) = nr.item_key {
                obj.insert("item_key".into(), serde_json::json!(item_key));
            }
            if let Some(output_type) = nr.output_type {
                obj.insert("output_type".into(), serde_json::json!(output_type));
            }
            if !nr.output_files.is_empty() {
                obj.insert(
                    "output_files".into(),
                    to_value_without_field(&nr.output_files, "fingerprint"),
                );
            }
            if !nr.port_assignments.is_empty() {
                // A map of port -> FileRef serializes as an object, so strip
                // each value individually rather than the map itself.
                let ports: serde_json::Map<String, _> = nr
                    .port_assignments
                    .iter()
                    .map(|(port, file)| {
                        (
                            port.to_string(),
                            to_value_without_field(file, "fingerprint"),
                        )
                    })
                    .collect();
                obj.insert("port_assignments".into(), serde_json::Value::Object(ports));
            }

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
            if let Some(execution) = nr.execution {
                obj.insert("execution".into(), serde_json::json!(execution));
            }
            if !nr.inputs.is_empty() {
                obj.insert(
                    "inputs".into(),
                    to_value_without_field(&nr.inputs, "fingerprint"),
                );
            }

            // Omit fingerprint in run report for agent
            // if let Some(fingerprint) = nr.fingerprint {
            //     obj.insert("fingerprint".into(), serde_json::json!(fingerprint));
            // }

            serde_json::Value::Object(obj)
        })
        .collect();

    // Summary counts.
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut cancelled = 0usize;
    for node in &nodes {
        let status = node.get("status").and_then(|v| v.as_str()).unwrap_or("");
        match status {
            "success" => succeeded += 1,
            "failed" => failed += 1,
            "skipped" => skipped += 1,
            "cancelled" => cancelled += 1,
            _ => {}
        }
    }
    let total = nodes.len();
    let logical_nodes =
        serde_json::to_value(&report.logical_nodes).unwrap_or(serde_json::Value::Array(Vec::new()));

    serde_json::json!({
        "ok": report.ok,
        "warnings": warnings,
        "snapshot_id": snapshot_id,
        "resource": report.resource,
        "summary": {
            "total": total,
            "succeeded": succeeded,
            "failed": failed,
            "skipped": skipped,
            "cancelled": cancelled,
        },
        "nodes": nodes,
        "logical_nodes": logical_nodes,
    })
}

/// Serialize a value, then remove `field` from the resulting object (or from
/// every object in a top-level array). Used to drop fingerprints from file
/// entries in the report without reconstructing the Rust types.
fn to_value_without_field<T: serde::Serialize>(value: &T, field: &str) -> serde_json::Value {
    let mut v = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
    match &mut v {
        serde_json::Value::Object(map) => {
            map.remove(field);
        }
        serde_json::Value::Array(items) => {
            for item in items {
                if let Some(map) = item.as_object_mut() {
                    map.remove(field);
                }
            }
        }
        _ => {}
    }
    v
}

#[async_trait]
impl ToolFunction for RunDagTool {
    type Input = RunDagInput;

    // Sync — the agent needs the DAG output to continue. DAG execution
    // can run long (full pipelines: LD scoring, fitting, heavy joins).
    fn timeout_seconds(&self) -> u64 {
        60 * 60 * 72
    }

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        // Non-streaming path (used when called directly, not via the toolset).
        let trigger = Some(format!("agent:{}", self.client.session_id()));
        let report = self
            .client
            .run_dag(trigger)
            .await
            .map_err(ExecError::from)?;
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

        let trigger = Some(format!("agent:{}", self.client.session_id()));
        let (mut event_rx, reply_rx) = self.client.run_dag_stream(parsed.commit_message, trigger);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_json_exposes_history_snapshot_result() {
        let report = RunReport {
            ok: true,
            warnings: vec!["no DAG history store attached; run snapshot was not persisted".into()],
            snapshot_id: None,
            resource: Default::default(),
            nodes: Vec::new(),
            logical_nodes: Vec::new(),
            statuses: Default::default(),
            errors: Default::default(),
        };

        let json = build_report_json(report);
        assert_eq!(
            json["warnings"][0],
            "no DAG history store attached; run snapshot was not persisted"
        );
        assert!(json["snapshot_id"].is_null());
    }

    #[test]
    fn report_json_strips_file_fingerprints() {
        use data_engine::dag::runtime::NodeReport;
        use data_engine::dag::{InputBinding, RuntimeStatus};
        use data_engine::value::{FileFingerprint, FileRef};

        let fingerprint = FileFingerprint {
            size: 1,
            mtime_ns: 2,
            content_hash: Some("abc".into()),
            immutable_remote: false,
        };
        let file = FileRef {
            path: "out.csv".into(),
            format: Some("csv".into()),
            fingerprint: Some(fingerprint.clone()),
        };
        let node = NodeReport {
            id: "sink".into(),
            status: RuntimeStatus::Success,
            node_type: "dataframe_to_file".into(),
            executor: Some("local"),
            logical_node: None,
            physical_job_id: None,
            scatter_axis: None,
            item_key: None,
            output_type: Some("File".into()),
            output_files: vec![file.clone()],
            port_assignments: [(0u8, file)].into_iter().collect(),
            output_schema: None,
            output_rows: None,
            elapsed_ms: Some(3),
            dispatch_seq: Some(0),
            artifact_path: None,
            file_path: Some("out.csv".into()),
            error: None,
            skipped_because: None,
            execution: None,
            inputs: vec![InputBinding {
                from: "src".into(),
                from_port: 0,
                to_port: 0,
                kind: "DataFrame".into(),
                path: Some("in.csv".into()),
                fingerprint: Some(fingerprint),
            }],
            fingerprint: None,
        };
        let report = RunReport {
            ok: true,
            warnings: Vec::new(),
            snapshot_id: None,
            resource: Default::default(),
            nodes: vec![node],
            logical_nodes: Vec::new(),
            statuses: Default::default(),
            errors: Default::default(),
        };

        let json = build_report_json(report);
        let node = &json["nodes"][0];

        assert!(node["output_files"][0].get("fingerprint").is_none());
        assert_eq!(node["output_files"][0]["path"], "out.csv");
        assert!(node["port_assignments"]["0"].get("fingerprint").is_none());
        assert!(node["inputs"][0].get("fingerprint").is_none());
        assert_eq!(node["inputs"][0]["path"], "in.csv");
    }
}
