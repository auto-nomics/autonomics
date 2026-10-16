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
                  port_assignments[port].path (the deterministic port \
                  contract), plus a truncated error or skipped_because where \
                  applicable. For file-producing nodes, use port_assignments \
                  when wiring downstream ports. \
                  For full per-node detail — execution evidence (image \
                  digest, exit code, persisted logs), input bindings, output \
                  schema, row counts, fingerprints, and complete scatter \
                  item lists — call dag_runs_log with run_id=\"latest\". \
                  A snapshot of the DAG is automatically committed to the \
                  history store — provide a descriptive commit message for \
                  easy retrieval via dag_history_log. Set resume=true to reuse \
                  successful nodes whose fingerprints and cached outputs are \
                  unchanged; input_hashing=content additionally hashes unhashed \
                  file-like inputs before reuse decisions; wave_size dispatches \
                  assay jobs in fixed barrier waves and groups failures by wave."
)]
pub struct RunDagInput {
    /// A short, human-readable description of what this run does (e.g. \
    /// "initial LDSC h² on BMI", "added sex covariate"). Stored as the \
    /// snapshot message so dag_history_log shows meaningful entries. \
    /// If omitted, a generic default is used.
    #[serde(default)]
    pub commit_message: Option<String>,
    /// Reuse successful nodes when their execution fingerprint and cached
    /// outputs are unchanged.
    #[serde(default)]
    pub resume: Option<bool>,
    /// Input identity depth: `metadata` (default) or `content`.
    #[serde(default)]
    pub input_hashing: Option<String>,
    /// Number of physical jobs in one fixed barrier wave.
    #[serde(default)]
    pub wave_size: Option<usize>,
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

/// Preview lengths for logical-node lists in the run_dag index. Complete
/// lists live in the drill-down view.
const SLIM_ITEM_KEYS: usize = 5;
const SLIM_LOGICAL_ERRORS: usize = 3;

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
    // Compact failure count for fast triage; the full list of failed
    // item keys still rides below (and is recoverable in full via
    // `dag_runs_log(view="diagnostic")`).
    obj.insert(
        "failed_count".into(),
        serde_json::json!(ls.failed_item_keys.len()),
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
    obj.insert(
        "item_keys_total".into(),
        serde_json::json!(ls.item_keys.len()),
    );
    obj.insert(
        "failed_item_keys".into(),
        serde_json::json!(ls.failed_item_keys),
    );
    obj.insert(
        "skipped_item_keys".into(),
        serde_json::json!(ls.skipped_item_keys),
    );
    obj.insert(
        "reused_item_keys".into(),
        serde_json::json!(ls.reused_item_keys[..ls.reused_item_keys.len().min(SLIM_ITEM_KEYS)]),
    );
    obj.insert(
        "reused_item_keys_total".into(),
        serde_json::json!(ls.reused_item_keys.len()),
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

/// Convert a finished [`RunReport`] into the minimal index JSON returned to
/// the agent. Shared by both the streaming and non-streaming execution
/// paths so their results are identical.
///
/// Progressive disclosure: this payload is just the index — a top-level
/// pass/fail summary, status counts, a logical-node aggregate per scatter
/// (with previews of failed item keys), and a `failure_manifest` for fast
/// failure triage. Per-node details (status, sink paths, timing, error
/// text, input bindings, output schema, fingerprints, execution evidence)
/// live in the persisted run record and are recovered via
/// `dag_runs_log(run_id="latest", view=...)` — see that tool's `view`
/// enum for the available drill-down shapes.
///
/// The full [`RunReport`] is still serialized into the `dag-history.db`
/// `runs.run_report_json` column at commit time, so any field the index
/// drops remains recoverable by run id.
fn build_report_json(report: RunReport) -> serde_json::Value {
    let warnings = report.warnings;
    let snapshot_id = report.snapshot_id;
    let run_id = report.run_id;

    // Per-status counts derived from the physical-job stream.
    let mut succeeded = 0usize;
    let mut reused = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut cancelled = 0usize;
    for node in &report.nodes {
        match node.status {
            data_engine::dag::RuntimeStatus::Success => {
                succeeded += 1;
                if node.reused {
                    reused += 1;
                }
            }
            data_engine::dag::RuntimeStatus::Failed => failed += 1,
            data_engine::dag::RuntimeStatus::Skipped => skipped += 1,
            data_engine::dag::RuntimeStatus::Cancelled => cancelled += 1,
            // Other variants (Pending, Running) do not appear on a finished
            // report; fall through so the count is exact for completed runs.
            _ => {}
        }
    }
    let total = report.nodes.len();

    // Logical-node aggregates — see `slim_logical_summary`.
    let logical_nodes: Vec<serde_json::Value> = report
        .logical_nodes
        .iter()
        .map(slim_logical_summary)
        .collect();
    let failure_manifest = build_failure_manifest(&report.logical_nodes, &report.waves);

    serde_json::json!({
        "ok": report.ok,
        "warnings": warnings,
        "snapshot_id": snapshot_id,
        "run_id": run_id,
        "resource": report.resource,
        "summary": {
            "total": total,
            "succeeded": succeeded,
            "reused": reused,
            "failed": failed,
            "skipped": skipped,
            "cancelled": cancelled,
        },
        "logical_nodes": logical_nodes,
        "failure_manifest": failure_manifest,
    })
}

fn build_failure_manifest(
    logical_nodes: &[data_engine::dag::LogicalRunSummary],
    waves: &[data_engine::dag::WaveRunSummary],
) -> serde_json::Value {
    let mut failed_items = Vec::new();
    for logical in logical_nodes {
        for item_key in &logical.failed_item_keys {
            failed_items.push(serde_json::json!({
                "logical_node": logical.logical_node,
                "item_key": item_key,
            }));
        }
    }

    let wave_failures = waves
        .iter()
        .filter(|wave| wave.failed > 0)
        .map(|wave| {
            serde_json::json!({
                "wave": wave.wave,
                "failed": wave.failed,
                "failed_item_keys": wave.failed_item_keys,
                "node_ids": wave.node_ids,
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "failed_item_count": failed_items.len(),
        "failed_items": failed_items,
        "wave_failures": wave_failures,
    })
}

fn run_options(
    input: &RunDagInput,
) -> Result<data_engine::runtime::types::RunDagOptions, ToolError> {
    let input_hashing = match input.input_hashing.as_deref().map(str::trim) {
        None | Some("") => None,
        Some("metadata") => Some(data_engine::dag::InputHashing::Metadata),
        Some("content") => Some(data_engine::dag::InputHashing::Content),
        Some(other) => {
            return Err(ToolError::ValidationFailed {
                message: format!("input_hashing must be `metadata` or `content`, got `{other}`"),
            });
        }
    };
    if input.wave_size == Some(0) {
        return Err(ToolError::ValidationFailed {
            message: "wave_size must be at least 1".to_string(),
        });
    }
    Ok(data_engine::runtime::types::RunDagOptions {
        resume: input.resume,
        input_hashing,
        wave_size: input.wave_size,
    })
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
        let options = run_options(&input)?;
        let report = self
            .client
            .run_dag_with_options(trigger, options)
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
        let options = run_options(&parsed)?;
        let (mut event_rx, reply_rx) =
            self.client
                .run_dag_stream_with_options(parsed.commit_message, trigger, options);
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
            run_id: None,
            resource: Default::default(),
            nodes: Vec::new(),
            logical_nodes: Vec::new(),
            waves: Vec::new(),
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

    #[tokio::test]
    async fn resume_reuses_unchanged_successful_nodes() {
        let engine = data_engine::data_engine::DataEngine::builder().build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        client
            .add_node("stable".into(), "echo".into(), serde_json::json!({}))
            .await
            .unwrap();
        let tool = RunDagTool::new(Arc::new(client));
        let input = |resume| RunDagInput {
            commit_message: Some("resume test".into()),
            resume: Some(resume),
            input_hashing: Some("metadata".into()),
            wave_size: Some(1),
        };

        let first = tool.run(input(true)).await.unwrap();
        let agentik_sdk::types::ToolResultContent::Json(first) = first.content else {
            panic!("run report should be JSON");
        };
        assert_eq!(first["ok"], true, "{first}");
        // Minimal index: per-node `reused` is gone; only the summary count
        // remains. To verify reuse actually happened, hit the drill-down
        // view for this run instead.
        assert_eq!(first["summary"]["reused"], 0, "{first}");

        let second = tool.run(input(true)).await.unwrap();
        let agentik_sdk::types::ToolResultContent::Json(second) = second.content else {
            panic!("run report should be JSON");
        };
        assert_eq!(second["ok"], true, "{second}");
        assert_eq!(second["summary"]["reused"], 1, "{second}");
    }

    #[test]
    fn run_options_reject_invalid_recovery_controls() {
        let input = RunDagInput {
            commit_message: None,
            resume: Some(true),
            input_hashing: Some("sha256".into()),
            wave_size: Some(1),
        };
        let error = run_options(&input).unwrap_err();
        assert!(error.to_string().contains("input_hashing must be"));

        let input = RunDagInput {
            input_hashing: Some("metadata".into()),
            wave_size: Some(0),
            ..input
        };
        let error = run_options(&input).unwrap_err();
        assert!(error.to_string().contains("wave_size must be at least 1"));
    }

    #[test]
    fn failure_manifest_groups_failed_items_by_wave() {
        use data_engine::dag::{LogicalRunSummary, RuntimeStatus, WaveRunSummary};

        let logical = LogicalRunSummary {
            logical_node: "assay_scan".into(),
            execution_strategy: Some("dynamic_for_each"),
            logical_node_type: Some("twosamplemr".into()),
            status: RuntimeStatus::Failed,
            physical_job_count: 2,
            status_counts: [("success".to_string(), 1), ("failed".to_string(), 1)]
                .into_iter()
                .collect(),
            scatter_axis: Some("assay".into()),
            item_keys: vec!["ok".into(), "bad".into()],
            failed_item_keys: vec!["bad".into()],
            skipped_item_keys: Vec::new(),
            reused_item_keys: Vec::new(),
            physical_job_ids: vec!["ok-job".into(), "bad-job".into()],
            summed_elapsed_ms: 12,
            max_elapsed_ms: Some(10),
            errors: Vec::new(),
        };
        let wave = WaveRunSummary {
            wave: 3,
            total: 2,
            succeeded: 1,
            failed: 1,
            skipped: 0,
            cancelled: 0,
            reused: 1,
            node_ids: vec!["ok-job".into(), "bad-job".into()],
            item_keys: vec!["ok".into(), "bad".into()],
            failed_item_keys: vec!["bad".into()],
            reused_item_keys: vec!["ok".into()],
        };

        let manifest = build_failure_manifest(&[logical], &[wave]);
        assert_eq!(manifest["failed_item_count"], 1);
        assert_eq!(manifest["failed_items"][0]["item_key"], "bad");
        assert_eq!(manifest["wave_failures"][0]["wave"], 3);
        assert_eq!(manifest["wave_failures"][0]["failed_item_keys"][0], "bad");
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
            port_assignments: [(0u8, file)].into_iter().collect(),
            output_schema: None,
            output_rows: None,
            elapsed_ms: Some(3),
            dispatch_seq: Some(0),
            reused: false,
            wave: None,
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
            run_id: None,
            resource: Default::default(),
            nodes: vec![node],
            logical_nodes: Vec::new(),
            waves: Vec::new(),
            statuses: Default::default(),
            errors: Default::default(),
        };

        let json = build_report_json(report);
        // The minimal index dropped per-node fields entirely — everything
        // that used to live in `nodes[0]` (status, port_assignments,
        // file_path, fingerprint, inputs, …) is now behind
        // `dag_runs_log(view=...)`. Verify the index shape instead.
        assert!(json.get("nodes").is_none(), "nodes[] is drill-down only");
        assert!(json.get("waves").is_none(), "waves[] is drill-down only");
        assert!(json["logical_nodes"].as_array().is_some());
        assert!(json["summary"].is_object());
        assert!(json["summary"]["total"].as_u64().is_some());
        assert!(json["run_id"].is_null() || json["run_id"].is_string());
        assert!(json["resource"].is_object());
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
            port_assignments: [(0u8, file)].into_iter().collect(),
            output_schema: Some(SchemaReport::from_fields(&Fields::from(vec![Field::new(
                "c0",
                DataType::Int32,
                true,
            )]))),
            output_rows: Some(42),
            elapsed_ms: Some(3),
            dispatch_seq: Some(0),
            reused: false,
            wave: None,
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
            port_assignments: BTreeMap::new(),
            output_schema: None,
            output_rows: None,
            elapsed_ms: None,
            dispatch_seq: None,
            reused: false,
            wave: None,
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
            run_id: None,
            resource: Default::default(),
            nodes: vec![node, skipped],
            logical_nodes: Vec::new(),
            waves: Vec::new(),
            statuses: Default::default(),
            errors: Default::default(),
        };

        let json = build_report_json(report);

        // Minimal index: no `nodes[]` field at all. The exact per-node
        // data (including the failure cause, the sink paths, and the
        // upstream skip reason) is recoverable only via
        // `dag_runs_log(view=...)`.
        assert!(json.get("nodes").is_none(), "index must not carry nodes[]");
        assert!(json.get("waves").is_none(), "index must not carry waves[]");
        // Status counts still roll up per-node status at the index level,
        // so the agent can tell failed from skipped without a drill-down.
        let summary = &json["summary"];
        assert_eq!(summary["total"].as_u64(), Some(2));
        assert_eq!(summary["failed"].as_u64(), Some(1));
        assert_eq!(summary["skipped"].as_u64(), Some(1));
        // The failure manifest only knows there is at least one failure —
        // it does not currently tag which logical node owns it, so an
        // empty scatter axis manifests as `failed_item_count: 0`. The
        // `failure_manifest.first_failure` slot is reserved for the
        // scatter-aware path added in dag_runs_log(view="diagnostic").
        let manifest = &json["failure_manifest"];
        assert!(manifest["failed_item_count"].as_u64().is_some());
    }

    #[test]
    fn logical_nodes_truncate_item_keys() {
        use data_engine::dag::LogicalJobError;
        use data_engine::dag::RuntimeStatus;
        use data_engine::dag::runtime::DagErrorReport;
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
            status_counts: BTreeMap::from([("failed".to_string(), 5), ("success".to_string(), 15)]),
            scatter_axis: Some("sample".into()),
            item_keys,
            failed_item_keys: (0..5).map(|i| format!("item_{i}")).collect(),
            skipped_item_keys: Vec::new(),
            reused_item_keys: Vec::new(),
            physical_job_ids: (0..20).map(|i| format!("job_{i}")).collect(),
            summed_elapsed_ms: 1234,
            max_elapsed_ms: Some(300),
            errors,
        };
        let report = RunReport {
            ok: false,
            warnings: Vec::new(),
            snapshot_id: None,
            run_id: None,
            resource: Default::default(),
            nodes: Vec::new(),
            logical_nodes: vec![summary],
            waves: Vec::new(),
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
