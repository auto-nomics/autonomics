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
                  expecting a list to be rejected). With run_id: show that \
                  single run including its full per-node report; `latest` \
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
                if run_id.as_deref().is_some_and(|id| id.eq_ignore_ascii_case("latest")) {
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

/// Full detail view for a single run: the record's metadata plus a compact
/// per-node summary decoded from the embedded run report (schemaless reads,
/// mirroring how `show_snapshot_tool` treats snapshot reports).
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
        let nodes = report
            .get("nodes")
            .and_then(|nodes| nodes.as_array())
            .map(|nodes| {
                nodes
                    .iter()
                    .map(|node| {
                        serde_json::json!({
                            "id": node.get("id"),
                            "status": node.get("status"),
                            "node_type": node.get("node_type"),
                            "elapsed_ms": node.get("elapsed_ms"),
                            "fingerprint": node.get("fingerprint"),
                            "execution": node.get("execution"),
                            "inputs": node.get("inputs"),
                            "error": node.get("error"),
                            "skipped_because": node.get("skipped_because"),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        value["nodes"] = serde_json::json!(nodes);
        value["report_ok"] = report.get("ok").cloned().unwrap_or(serde_json::json!(null));
        value["warnings"] = report.get("warnings").cloned().unwrap_or(serde_json::json!(null));
    }

    value
}
