use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "dag_runs_log",
    description = "Query the execution audit trail of the data engine. \
                  Every DAG execution leaves one append-only run record: \
                  who triggered it, when, which snapshot (DAG definition) it \
                  ran, the engine version + git revision, and per-node \
                  execution evidence (container image digest, exit code, \
                  input bindings, persisted stdout/stderr logs). \
                  Without run_id: list recent runs newest-first (leave the \
                  field absent or empty — do not pass an empty string \
                  expecting a list to be rejected). With run_id: that single \
                  run's full per-node report — the drill-down into a run_dag \
                  summary, recovering everything it omits: execution \
                  evidence, input bindings, output schema, row counts, \
                  fingerprints, and complete scatter item lists. `latest` \
                  selects the most recent run, a unique id prefix also \
                  works. Run ids shown here are what dag_export_run accepts. \
                  Use this to answer audit questions such as 'which run \
                  produced this output, from which inputs, with which image'."
)]
pub struct DagRunsLogInput {
    /// Ref name to filter by (e.g. "main"). If omitted, spans all refs.
    pub ref_name: Option<String>,
    /// Maximum number of runs to return when listing (default 20).
    pub limit: Option<usize>,
    /// Fetch a single run by its id (`latest` or a unique prefix also
    /// works) instead of listing, returning the full per-node run report.
    pub run_id: Option<String>,
}

pub struct DagRunsLogTool {
    client: Arc<DataEngineClient>,
}

impl DagRunsLogTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for DagRunsLogTool {
    type Input = DagRunsLogInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let limit = input.limit.unwrap_or(20);
        // An empty or whitespace run_id means "list" — normalize so callers
        // passing "" get the listing they meant, not a not-found error.
        let run_id = input
            .run_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string);
        let runs = self
            .client
            .dag_runs_log(input.ref_name.clone(), limit, run_id.clone())
            .await
            .map_err(ExecError::from)?;

        if runs.is_empty() {
            return Ok(ToolResult::success(if run_id.is_some() {
                if run_id
                    .as_deref()
                    .is_some_and(|id| id.eq_ignore_ascii_case("latest"))
                {
                    "No runs recorded yet."
                } else {
                    "No run found with that id (pass no run_id to list recent runs)."
                }
            } else {
                "No runs recorded yet."
            }));
        }

        if run_id.is_some()
            && let Some(run) = runs.first()
        {
            return Ok(ToolResult::success_json(run_detail_json(run)));
        }

        let mut out = String::new();
        for run in &runs {
            let short_id = &run.id[..12.min(run.id.len())];
            let snapshot = run
                .snapshot_id
                .as_ref()
                .map(|id| &id[..12.min(id.len())])
                .unwrap_or("(none)");
            let trigger = run.trigger.as_deref().unwrap_or("(unattributed)");
            let outcome = if run.cancelled {
                "cancelled"
            } else if run.ok {
                "ok"
            } else {
                "failed"
            };
            out.push_str(&format!(
                "{short_id}  snapshot={snapshot}  {outcome}  {}  trigger={trigger}\n",
                &run.started_at[..19.min(run.started_at.len())],
            ));
        }
        out.push_str(&format!(
            "\n{} runs (limit {limit}). Pass run_id for the full per-node report.",
            runs.len()
        ));

        Ok(ToolResult::success(out))
    }
}

/// Full detail view for a single run: the record's metadata plus the
/// per-node report decoded from the embedded run report (schemaless reads,
/// mirroring how `show_snapshot_tool` treats snapshot reports).
///
/// This is the drill-down counterpart of `run_dag`'s slim summary: every
/// field the summary omits — execution evidence, input bindings, output
/// schema, row counts, fingerprints, scatter bookkeeping — is surfaced here
/// from the persisted blob, which always carries the complete report.
fn run_detail_json(run: &data_engine::dag::RunRecord) -> serde_json::Value {
    let mut value = serde_json::json!({
        "id": run.id,
        "ref_name": run.ref_name,
        "snapshot_id": run.snapshot_id,
        "manifest_hash": run.manifest_hash,
        "trigger": run.trigger,
        "started_at": run.started_at,
        "finished_at": run.finished_at,
        "ok": run.ok,
        "cancelled": run.cancelled,
        "error": run.error,
        "message": run.message,
        "engine_version": run.engine_version,
        "source_revision": run.source_revision,
    });

    if let Some(report_json) = &run.run_report_json
        && let Ok(report) = serde_json::from_str::<serde_json::Value>(report_json)
    {
        let nodes: Vec<serde_json::Value> = report
            .get("nodes")
            .and_then(|nodes| nodes.as_array())
            .map(|nodes| nodes.iter().map(node_detail_from_persisted).collect())
            .unwrap_or_default();
        value["nodes"] = serde_json::json!(nodes);
        let logical_nodes: Vec<serde_json::Value> = report
            .get("logical_nodes")
            .and_then(|nodes| nodes.as_array())
            .map(|nodes| {
                nodes
                    .iter()
                    .map(logical_node_detail_from_persisted)
                    .collect()
            })
            .unwrap_or_default();
        if !logical_nodes.is_empty() {
            value["logical_nodes"] = serde_json::json!(logical_nodes);
        }
        value["report_ok"] = report.get("ok").cloned().unwrap_or(serde_json::json!(null));
        value["warnings"] = report
            .get("warnings")
            .cloned()
            .unwrap_or(serde_json::json!(null));
    }

    value
}

/// Full per-node view decoded from one entry of the persisted run report.
///
/// Copies every recorded field as-is (fingerprints included — this is the
/// audit view), skipping keys the serialized `NodeReport` omitted.
fn node_detail_from_persisted(node: &serde_json::Value) -> serde_json::Value {
    copy_present_fields(
        node,
        &[
            "id",
            "status",
            "node_type",
            "executor",
            "logical_node",
            "physical_job_id",
            "scatter_axis",
            "item_key",
            "output_type",
            "output_files",
            "port_assignments",
            "output_schema",
            "output_rows",
            "elapsed_ms",
            "dispatch_seq",
            "artifact_path",
            "file_path",
            "error",
            "skipped_because",
            "execution",
            "inputs",
            "fingerprint",
        ],
    )
}

/// Full logical-node view decoded from the persisted run report: the
/// untruncated `item_keys`, the complete `errors` list, and
/// `physical_job_ids` — everything the slim summary previews or drops.
fn logical_node_detail_from_persisted(ls: &serde_json::Value) -> serde_json::Value {
    copy_present_fields(
        ls,
        &[
            "logical_node",
            "execution_strategy",
            "logical_node_type",
            "status",
            "physical_job_count",
            "status_counts",
            "scatter_axis",
            "item_keys",
            "failed_item_keys",
            "skipped_item_keys",
            "physical_job_ids",
            "summed_elapsed_ms",
            "max_elapsed_ms",
            "errors",
        ],
    )
}

/// Project `source` down to the listed keys, keeping only keys that exist
/// and are non-null. The persisted `NodeReport` serialization uses
/// `skip_serializing_if` for its optional fields, so presence == recorded.
fn copy_present_fields(source: &serde_json::Value, keys: &[&str]) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for key in keys {
        if let Some(value) = source.get(key)
            && !value.is_null()
        {
            obj.insert((*key).into(), value.clone());
        }
    }
    serde_json::Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_run(report_json: &str) -> data_engine::dag::RunRecord {
        data_engine::dag::RunRecord {
            id: "run-1".into(),
            ref_name: "main".into(),
            snapshot_id: Some("snap-1".into()),
            manifest_hash: "hash".into(),
            trigger: None,
            started_at: "2026-10-06T10:00:00Z".into(),
            finished_at: "2026-10-06T10:01:00Z".into(),
            ok: true,
            cancelled: false,
            error: None,
            message: None,
            engine_version: "0.1.0".into(),
            source_revision: "abc1234".into(),
            run_report_json: Some(report_json.to_string()),
        }
    }

    /// The drill-down view must surface every field the run_dag slim summary
    /// drops, straight from the persisted full report.
    #[test]
    fn run_detail_recovers_fields_the_summary_drops() {
        let report = serde_json::json!({
            "ok": true,
            "warnings": [],
            "nodes": [{
                "id": "sink",
                "status": "success",
                "node_type": "dataframe_to_file",
                "executor": "local",
                "logical_node": "scatter",
                "physical_job_id": "job-1",
                "scatter_axis": "sample",
                "item_key": "s1",
                "output_type": "File",
                "output_files": [{
                    "path": "out.csv",
                    "format": "csv",
                    "fingerprint": {"size": 1, "mtime_ns": 2}
                }],
                "port_assignments": {"0": {"path": "out.csv"}},
                "output_schema": {"column_count": 1, "columns": {}, "type_distribution": null},
                "output_rows": 42,
                "elapsed_ms": 7,
                "dispatch_seq": 3,
                "artifact_path": "a.png",
                "file_path": "out.csv",
                "execution": {"exit_code": 0, "image": "img"},
                "inputs": [{
                    "from": "src",
                    "from_port": 0,
                    "to_port": 0,
                    "kind": "File",
                    "path": "in.csv"
                }],
                "fingerprint": "fp"
            }],
            "logical_nodes": [{
                "logical_node": "scatter",
                "status": "failed",
                "physical_job_count": 3,
                "status_counts": {"failed": 1},
                "scatter_axis": "sample",
                "item_keys": ["a", "b", "c", "d", "e", "f"],
                "physical_job_ids": ["j1", "j2", "j3"],
                "summed_elapsed_ms": 9,
                "max_elapsed_ms": 5,
                "errors": [{"physical_job_id": "j1", "error": {"kind": "k", "message": "m"}}]
            }]
        });
        let run = sample_run(&report.to_string());

        let detail = run_detail_json(&run);
        let node = &detail["nodes"][0];
        for key in [
            "executor",
            "logical_node",
            "physical_job_id",
            "scatter_axis",
            "item_key",
            "output_type",
            "output_files",
            "port_assignments",
            "output_schema",
            "output_rows",
            "dispatch_seq",
            "artifact_path",
            "file_path",
            "execution",
            "inputs",
            "fingerprint",
        ] {
            assert!(node.get(key).is_some(), "drill-down must surface {key}");
        }
        // Fingerprints ride along from the persisted blob (audit view).
        assert!(node["output_files"][0].get("fingerprint").is_some());

        let ls = &detail["logical_nodes"][0];
        // Untruncated item keys and the full job-id list, unlike the summary.
        assert_eq!(ls["item_keys"].as_array().map(Vec::len), Some(6));
        assert_eq!(ls["physical_job_ids"].as_array().map(Vec::len), Some(3));
        assert_eq!(ls["errors"].as_array().map(Vec::len), Some(1));
    }

    /// Absent optional fields stay absent (not null) and run-level metadata
    /// is preserved alongside the decoded report.
    #[test]
    fn run_detail_handles_sparse_report() {
        let report = serde_json::json!({"ok": false, "warnings": ["w"], "nodes": []});
        let run = sample_run(&report.to_string());

        let detail = run_detail_json(&run);
        assert_eq!(detail["id"], "run-1");
        assert_eq!(detail["report_ok"], false);
        assert_eq!(detail["warnings"], serde_json::json!(["w"]));
        assert_eq!(detail["nodes"].as_array().map(Vec::len), Some(0));
        assert!(detail.get("logical_nodes").is_none());
    }
}
