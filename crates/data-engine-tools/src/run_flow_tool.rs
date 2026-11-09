use std::sync::Arc;

use agentik_core::tools::{ToolContext, ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;
use serde_json::Value;

use crate::ExecError;

/// Preview lengths for per-instance failure details. Complete detail is
/// available by re-running or extending the report; the index stays small.
const MAX_LISTED_FAILURES: usize = 10;
const MAX_ERROR_CHARS: usize = 300;

#[tool(
    name = "run_flow",
    description = "Execute a flow using the session graph or the supplied JSON graph. \
                  The graph contains sources, processes, and wires. Sources emit items \
                  incrementally and process instances start when their input tuples arrive. \
                  Queue inputs pair in arrival order, value inputs broadcast, and collect \
                  waits for upstream closure. \
                  Returns process summaries, audit persistence status, artifact path previews and failed instance details."
)]
pub struct RunFlowInput {
    /// The authored flow graph (processes + wires arrays). Omit to run the
    /// flow already stored for this session (see set_flow); passing a graph
    /// replaces the stored flow and runs it in one step.
    #[serde(default)]
    pub flow: Option<Value>,
    /// Maximum item instances executing concurrently. Omit for the
    /// CPU-derived default.
    #[serde(default)]
    pub max_concurrency: Option<usize>,
    /// Reuse eligible instances whose fingerprints and file outputs can be
    /// validated. Inputs or outputs that cannot be validated are re-executed.
    #[serde(default)]
    pub resume: Option<bool>,
}

pub struct RunFlowTool {
    client: Arc<DataEngineClient>,
}

impl RunFlowTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

/// Slim, process-grouped result index. Per-instance detail beyond failures
/// is intentionally not carried — the shape mirrors `run_dag`'s progressive
/// disclosure posture.
fn build_flow_report_json(report: &data_engine::workflow::FlowRunReport) -> Value {
    let mut processes = serde_json::Map::new();
    for (name, summary) in &report.processes {
        processes.insert(
            name.clone(),
            serde_json::json!({
                "total": summary.total,
                "succeeded": summary.succeeded,
                "failed": summary.failed,
                "skipped": summary.skipped,
                "cancelled": summary.cancelled,
                "reused": summary.reused,
                "summed_elapsed_ms": summary.summed_elapsed_ms,
            }),
        );
    }

    let failed: Vec<Value> = report
        .instances
        .iter()
        .filter(|record| {
            matches!(
                record.status,
                data_engine::dag::RuntimeStatus::Failed
                    | data_engine::dag::RuntimeStatus::Skipped
                    | data_engine::dag::RuntimeStatus::Cancelled
            )
        })
        .take(MAX_LISTED_FAILURES)
        .map(|record| {
            serde_json::json!({
                "instance": record.id.as_str(),
                "process": record.process,
                "item_key": record.item_key,
                "status": format!("{:?}", record.status).to_lowercase(),
                "ignored": record.ignored,
                "attempts": record.attempts,
                "error": record.error.as_deref().map(|text| {
                    text.chars().take(MAX_ERROR_CHARS).collect::<String>()
                }),
            })
        })
        .collect();

    serde_json::json!({
        "ok": report.ok,
        "run_id": report.run_id,
        "audit_persisted": report.audit_persisted,
        "cancelled": report.cancelled,
        "artifacts": report.instances.iter().filter(|record| !record.outputs.is_empty() || record.execution.is_some()).take(10).map(|record| {
            let audit = record.audit_json();
            let files = audit["output_files"].as_array().expect("audit file list");
            serde_json::json!({"instance": record.id.as_str(), "output_files": files.iter().take(10).collect::<Vec<_>>(), "output_files_total": files.len(), "workspace": record.execution.as_ref().and_then(|execution| execution.workspace.as_deref())})
        }).collect::<Vec<_>>(),
        "artifact_details_truncated": report.instances.iter().filter(|record| !record.outputs.is_empty() || record.execution.is_some()).count() > 10,
        "artifact_details": "dag_runs_log view=node_detail exposes persisted output_files, port_assignments and execution.workspace",
        "reuse_policy": "Only validated file outputs are reusable; unsupported or unverifiable inputs/outputs re-execute. History persistence is required across restarts, not within a session.",
        "processes": processes,
        "failed_instances": failed,
        "failed_instances_truncated":
            report.instances.iter().filter(|record| matches!(record.status, data_engine::dag::RuntimeStatus::Failed | data_engine::dag::RuntimeStatus::Skipped | data_engine::dag::RuntimeStatus::Cancelled)).count().saturating_sub(MAX_LISTED_FAILURES),
        "execution_binding": report.execution_binding,
    })
}

#[async_trait]
impl ToolFunction for RunFlowTool {
    type Input = RunFlowInput;

    // Batch flows run long (100-file pipelines); match run_dag's window.
    fn timeout_seconds(&self) -> u64 {
        60 * 60 * 72
    }

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let flow = match input.flow {
            Some(value) => Some(
                serde_json::from_value::<data_engine::workflow::FlowGraph>(value).map_err(|error| {
                    ToolError::ValidationFailed {
                        message: format!("invalid flow graph: {error}"),
                    }
                })?,
            ),
            None => None,
        };
        let options = data_engine::flow::FlowOptions {
            max_concurrency: input.max_concurrency,
            resume: input.resume,
        };
        let report = self
            .client
            .run_flow(flow, options)
            .await
            .map_err(ExecError::from)?;
        Ok(ToolResult::success_json(build_flow_report_json(&report)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fanout_flow_runs_and_groups_by_process() {
        let engine = data_engine::data_engine::DataEngine::builder().build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let directory = tempfile::tempdir().unwrap();
        let items: Vec<serde_json::Value> = ["a", "b", "c"]
            .iter()
            .map(|name| {
                let path = directory.path().join(format!("{name}.txt"));
                std::fs::write(&path, name).unwrap();
                serde_json::json!(path.display().to_string())
            })
            .collect();
        let flow = serde_json::json!({
            "sources": [
                {"id": "reads", "spec": {"type": "items", "values": items}}
            ],
            "processes": [
                {"id": "batch", "kind": "file_reference", "spec": {"path": "{{item}}"}}
            ],
            "wires": [
                {"from": "reads", "from_port": 0, "to": "batch", "to_port": 0, "op": "direct"}
            ]
        });
        let tool = RunFlowTool::new(Arc::new(client));
        let result = tool
            .run(RunFlowInput {
                flow: Some(flow),
                max_concurrency: None,
                resume: None,
            })
            .await
            .unwrap();
        let agentik_sdk::types::ToolResultContent::Json(json) = result.content else {
            panic!("flow report should be JSON");
        };
        assert_eq!(json["ok"], true, "{json}");
        assert_eq!(json["processes"]["batch"]["succeeded"], 3, "{json}");
        assert_eq!(json["processes"]["batch"]["total"], 3, "{json}");
        assert!(json["failed_instances"].as_array().unwrap().is_empty());
        assert_eq!(json["audit_persisted"], false);
        assert!(json["run_id"].is_null());
        assert_eq!(json["artifacts"].as_array().unwrap().len(), 3);
        assert!(json["artifacts"][0]["output_files"][0]["path"].is_string());
        assert!(json["artifacts"][0]["workspace"].is_string());
    }

    #[tokio::test]
    async fn invalid_flow_json_is_a_validation_error() {
        let engine = data_engine::data_engine::DataEngine::builder().build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let tool = RunFlowTool::new(Arc::new(client));
        let error = tool
            .run(RunFlowInput {
                flow: Some(serde_json::json!({"processes": "not-a-list"})),
                max_concurrency: None,
                resume: None,
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("invalid flow graph"), "{error}");
    }
}
