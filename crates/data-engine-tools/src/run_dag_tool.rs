use std::sync::Arc;

use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::dag::node_event::{NodeEvent, NodeEventKind};
use data_engine::dag::runtime::NodeReport;
use data_engine::dag::{LogicalRunSummary, RunReport};
use data_engine::runtime::DataEngineClient;
use serde_json::Value;

use agentik_core::tools::{ToolContext, ToolError, ToolFunction};
use agentik_proc::tool;

use crate::ExecError;

#[tool(
    name = "run_dag",
    description = "Execute the current DAG pipeline. All nodes are validated \
                  and run according to their dependency order. Returns a slim \
                  summary: top-level ok/warnings/snapshot_id/summary counts, \
                  and per node the id, status, node_type, output_type, \
                  elapsed_ms, sink paths (file_path, artifact_path), \
                  output_files[*].path, port_assignments[port].path, plus a \
                  truncated error or skipped_because where applicable. \
                  For file-producing nodes, use port_assignments (not \
                  output_files order) when wiring downstream ports. \
                  For full per-node detail — execution evidence (image \
                  digest, exit code, persisted logs), input bindings, output \
                  schema, row counts, fingerprints, and complete scatter \
                  item lists — call dag_runs_log with run_id=\"latest\". \
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
        NodeEventKind::ChannelItem { sequence, .. } => Some(
            ProgressRecord::new("channel_item")
                .label(label)
                .current(*sequence),
        ),
        NodeEventKind::ChannelClosed { item_count, .. } => Some(
            ProgressRecord::new("channel_closed")
                .label(label)
                .current(*item_count)
                .total(*item_count),
        ),
        // Done carries the heavy output map and is never emitted on the
        // external sink — nothing to record.
        NodeEventKind::Done(_) => None,
    }
}

/// Cap on the `error.message` preview in the slim per-node summary. The
/// untruncated message is always in the drill-down view.
const SLIM_ERROR_MESSAGE_CHARS: usize = 200;

/// Preview lengths for logical-node lists in the slim summary. Complete
/// lists live in the drill-down view.
const SLIM_ITEM_KEYS: usize = 5;
const SLIM_LOGICAL_ERRORS: usize = 3;

/// Truncate to at most `max_chars` characters (char-boundary safe), marking
/// the clip with a trailing ellipsis.
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let prefix: String = text.chars().take(keep).collect();
    format!("{prefix}…")
}

/// Slim per-node view of a [`NodeReport`] for the agent-facing run summary.
///
/// Keeps what the agent needs to react to the run: identity, status, output
/// kind, sink and port-wiring paths, timing, and failure/skip causes. The
/// heavy evidence — `execution` (image digest, exit code, persisted logs),
/// `inputs[]` bindings, `output_schema`, `output_rows`, `fingerprint`, and
/// the scatter bookkeeping fields — is omitted here and recoverable via
/// `dag_runs_log(run_id="latest")`, which decodes the persisted full report.
fn slim_node_report(nr: &NodeReport) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("id".into(), serde_json::json!(nr.id));
    obj.insert("status".into(), serde_json::json!(nr.status));
    obj.insert("node_type".into(), serde_json::json!(nr.node_type));
    if let Some(output_type) = &nr.output_type {
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
                (port.to_string(), to_value_without_field(file, "fingerprint"))
            })
            .collect();
        obj.insert("port_assignments".into(), serde_json::Value::Object(ports));
    }
    if let Some(ms) = nr.elapsed_ms {
        obj.insert("elapsed_ms".into(), serde_json::json!(ms));
    }
    if let Some(path) = &nr.file_path {
        obj.insert("file_path".into(), serde_json::json!(path));
    }
    if let Some(path) = &nr.artifact_path {
        obj.insert("artifact_path".into(), serde_json::json!(path));
    }
    if let Some(err) = &nr.error {
        // DagErrorReport is exactly {kind, message}; clamp the message so a
        // verbose node error cannot flood the summary.
        obj.insert(
            "error".into(),
            serde_json::json!({
                "kind": err.kind,
                "message": truncate_chars(&err.message, SLIM_ERROR_MESSAGE_CHARS),
            }),
        );
    }
    if let Some(cause) = &nr.skipped_because {
        obj.insert("skipped_because".into(), serde_json::json!(cause));
    }
    serde_json::Value::Object(obj)
}

/// Slim view of a [`LogicalRunSummary`]: aggregate scatter health without
/// the full per-item enumerations. `item_keys` and `errors` are previewed
/// with explicit totals; `physical_job_ids` is drill-down only.
fn slim_logical_summary(ls: &LogicalRunSummary) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("logical_node".into(), serde_json::json!(ls.logical_node));
    if let Some(strategy) = ls.execution_strategy {
        obj.insert("execution_strategy".into(), serde_json::json!(strategy));
    }
    if let Some(node_type) = &ls.logical_node_type {
        obj.insert("logical_node_type".into(), serde_json::json!(node_type));
    }
    obj.insert("status".into(), serde_json::json!(ls.status));
    obj.insert(
        "physical_job_count".into(),
        serde_json::json!(ls.physical_job_count),
    );
    obj.insert("status_counts".into(), serde_json::json!(ls.status_counts));
    if let Some(axis) = &ls.scatter_axis {
        obj.insert("scatter_axis".into(), serde_json::json!(axis));
    }
    // Preview the leading item keys; the complete list is drill-down only.
    obj.insert(
        "item_keys".into(),
        serde_json::json!(ls.item_keys[..ls.item_keys.len().min(SLIM_ITEM_KEYS)]),
    );
    obj.insert("item_keys_total".into(), serde_json::json!(ls.item_keys.len()));
    obj.insert(
        "failed_item_keys".into(),
        serde_json::json!(ls.failed_item_keys),
    );
    obj.insert(
        "skipped_item_keys".into(),
        serde_json::json!(ls.skipped_item_keys),
    );
    obj.insert(
        "summed_elapsed_ms".into(),
        serde_json::json!(ls.summed_elapsed_ms),
    );
    if let Some(ms) = ls.max_elapsed_ms {
        obj.insert("max_elapsed_ms".into(), serde_json::json!(ms));
    }
    // Preview the leading errors; the complete list is drill-down only.
    obj.insert(
        "errors".into(),
        serde_json::json!(ls.errors[..ls.errors.len().min(SLIM_LOGICAL_ERRORS)]),
    );
    let hidden = ls.errors.len().saturating_sub(SLIM_LOGICAL_ERRORS);
    if hidden > 0 {
        obj.insert("errors_truncated".into(), serde_json::json!(hidden));
    }
    serde_json::Value::Object(obj)
}

/// Convert a finished [`RunReport`] into the slim summary JSON returned to
/// the agent. Shared by both the streaming and non-streaming execution
/// paths so their results are identical.
///
/// Progressive disclosure: per-node entries carry identity, status, output
/// kind, sink/port paths, timing, and failure/skip causes
/// ([`slim_node_report`]); logical nodes carry aggregate health
/// ([`slim_logical_summary`]). The heavy evidence — `execution` details,
/// `inputs[]` bindings, `output_schema`, `output_rows`, fingerprints, and
/// full scatter enumerations — stays in the persisted run record and is
/// recovered via `dag_runs_log(run_id="latest")`.
///
/// File-level fingerprints are stripped via [`to_value_without_field`] so the
/// agent-facing report stays slim without changing `FileRef`'s serde contract.
fn build_report_json(report: RunReport) -> serde_json::Value {
    let warnings = report.warnings;
    let snapshot_id = report.snapshot_id;

    // Slim per-node entries — see `slim_node_report` for the kept subset.
    let nodes: Vec<serde_json::Value> = report.nodes.iter().map(slim_node_report).collect();

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
    // Slim logical-node aggregates — see `slim_logical_summary`.
    let logical_nodes: Vec<serde_json::Value> = report
        .logical_nodes
        .iter()
        .map(slim_logical_summary)
        .collect();

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
            resources: Default::default(),
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
        assert_eq!(node["port_assignments"]["0"]["path"], "out.csv");
        // The slim summary omits input bindings entirely — drill down via
        // dag_runs_log(run_id="latest") for the full inputs list.
        assert!(node.get("inputs").is_none());
    }

    #[test]
    fn report_json_slim_omits_drill_down_fields() {
        use arrow::datatypes::{DataType, Field, Fields};
        use data_engine::dag::runtime::{DagErrorReport, SchemaReport};
        use data_engine::dag::{InputBinding, NodeRunDetails, RuntimeStatus};
        use data_engine::value::FileRef;
        use std::collections::BTreeMap;

        let file = FileRef {
            path: "out.csv".into(),
            format: Some("csv".into()),
            fingerprint: None,
        };
        let node = NodeReport {
            id: "sink".into(),
            status: RuntimeStatus::Failed,
            node_type: "container_command".into(),
            executor: Some("local"),
            resources: Default::default(),
            logical_node: Some("scatter_src".into()),
            physical_job_id: Some("job-1".into()),
            scatter_axis: Some("sample".into()),
            item_key: Some("s1".into()),
            output_type: Some("File".into()),
            output_files: vec![file.clone()],
            port_assignments: [(0u8, file)].into_iter().collect(),
            output_schema: Some(SchemaReport::from_fields(&Fields::from(vec![Field::new(
                "c0",
                DataType::Int32,
                true,
            )]))),
            output_rows: Some(42),
            elapsed_ms: Some(3),
            dispatch_seq: Some(0),
            artifact_path: Some("art.png".into()),
            file_path: Some("out.csv".into()),
            error: Some(DagErrorReport {
                kind: "node_error".into(),
                message: "x".repeat(500),
            }),
            skipped_because: None,
            execution: Some(NodeRunDetails {
                workspace: Some("/tmp/ws".into()),
                exit_code: Some(1),
                ..Default::default()
            }),
            inputs: vec![InputBinding {
                from: "src".into(),
                from_port: 0,
                to_port: 0,
                kind: "DataFrame".into(),
                path: Some("in.csv".into()),
                fingerprint: None,
            }],
            fingerprint: Some("fp".into()),
        };
        let skipped = NodeReport {
            id: "downstream".into(),
            status: RuntimeStatus::Skipped,
            node_type: "sql".into(),
            executor: None,
            resources: Default::default(),
            logical_node: None,
            physical_job_id: None,
            scatter_axis: None,
            item_key: None,
            output_type: None,
            output_files: Vec::new(),
            port_assignments: BTreeMap::new(),
            output_schema: None,
            output_rows: None,
            elapsed_ms: None,
            dispatch_seq: None,
            artifact_path: None,
            file_path: None,
            error: None,
            skipped_because: Some("sink".into()),
            execution: None,
            inputs: Vec::new(),
            fingerprint: None,
        };
        let report = RunReport {
            ok: false,
            warnings: Vec::new(),
            snapshot_id: None,
            resource: Default::default(),
            nodes: vec![node, skipped],
            logical_nodes: Vec::new(),
            statuses: Default::default(),
            errors: Default::default(),
        };

        let json = build_report_json(report);
        let failed = &json["nodes"][0];
        for key in [
            "executor",
            "resources",
            "logical_node",
            "physical_job_id",
            "scatter_axis",
            "item_key",
            "output_schema",
            "output_rows",
            "dispatch_seq",
            "execution",
            "inputs",
            "fingerprint",
        ] {
            assert!(failed.get(key).is_none(), "slim summary must omit {key}");
        }
        // Failure cause is kept, with the message clamped.
        let message = failed["error"]["message"].as_str().unwrap();
        assert!(message.chars().count() <= 200, "message not clamped");
        assert_eq!(failed["error"]["kind"], "node_error");
        // Sink paths the agent acts on are kept.
        assert_eq!(failed["file_path"], "out.csv");
        assert_eq!(failed["artifact_path"], "art.png");

        let skipped_json = &json["nodes"][1];
        assert_eq!(skipped_json["skipped_because"], "sink");
    }

    #[test]
    fn logical_nodes_truncate_item_keys() {
        use data_engine::dag::LogicalJobError;
        use data_engine::dag::runtime::DagErrorReport;
        use data_engine::dag::RuntimeStatus;
        use std::collections::BTreeMap;

        let item_keys: Vec<String> = (0..20).map(|i| format!("item_{i}")).collect();
        let errors: Vec<LogicalJobError> = (0..5)
            .map(|i| LogicalJobError {
                physical_job_id: format!("job_{i}"),
                item_key: Some(format!("item_{i}")),
                error: DagErrorReport {
                    kind: "node_error".into(),
                    message: format!("boom {i}"),
                },
            })
            .collect();
        let summary = LogicalRunSummary {
            logical_node: "scatter".into(),
            execution_strategy: Some("fan_out"),
            logical_node_type: Some("map".into()),
            status: RuntimeStatus::Failed,
            physical_job_count: 20,
            status_counts: BTreeMap::from([
                ("failed".to_string(), 5),
                ("success".to_string(), 15),
            ]),
            scatter_axis: Some("sample".into()),
            item_keys,
            failed_item_keys: (0..5).map(|i| format!("item_{i}")).collect(),
            skipped_item_keys: Vec::new(),
            physical_job_ids: (0..20).map(|i| format!("job_{i}")).collect(),
            summed_elapsed_ms: 1234,
            max_elapsed_ms: Some(300),
            errors,
        };
        let report = RunReport {
            ok: false,
            warnings: Vec::new(),
            snapshot_id: None,
            resource: Default::default(),
            nodes: Vec::new(),
            logical_nodes: vec![summary],
            statuses: Default::default(),
            errors: Default::default(),
        };

        let json = build_report_json(report);
        let ls = &json["logical_nodes"][0];
        assert_eq!(ls["item_keys"].as_array().map(Vec::len), Some(5));
        assert_eq!(ls["item_keys_total"], 20);
        assert_eq!(ls["errors"].as_array().map(Vec::len), Some(3));
        assert_eq!(ls["errors_truncated"], 2);
        assert!(ls.get("physical_job_ids").is_none());
        assert_eq!(ls["physical_job_count"], 20);
        assert_eq!(ls["summed_elapsed_ms"], 1234);
    }
}
