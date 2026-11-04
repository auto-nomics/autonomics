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
    description = "Execute the stage-model workflow (Nextflow-style) for batch analysis. \
                  The flow is authored at PROCESS granularity: one process per pipeline \
                  stage, regardless of how many items it fans out to — a folder of 100 \
                  files into the same pipeline is ONE fanout process, not 100 nodes. \
                  The `flow` parameter carries the authored graph as JSON: \
                  processes: [{id, kind (a registered node kind, e.g. container_command), \
                  spec (may reference {{item}} / {{item.field}} from the fanout source), \
                  cardinality: {type: \"singleton\"} or {type: \"fanout\", axis, source: \
                  {type: \"glob\", pattern: \"data/*.fastq\"} (enumerated through the VFS \
                  at run start) or {type: \"items\", values: [...]}}], and wires: \
                  [{from, from_port, to, to_port, op: \"direct\" (key-zip between fanouts, \
                  broadcast from a singleton) or \"collect\" (fanout -> singleton, \
                  Nextflow's collect)}. Rules: direct fanout->singleton is invalid — use \
                  collect; connected fanouts must resolve to identical item key sets. \
                  Per-process error_strategy (Nextflow's errorStrategy): default is \
                  fail-closed; mode=ignore tolerates that process's item failures \
                  (key-matched dependents skip, collect consumers still run on survivors, \
                  the run stays ok); mode=retry with max_retries=N and optional \
                  retry_delay_ms=M re-dispatches failed instances before degrading \
                  to fail-closed. \
                  Returns a process-grouped summary (succeeded/failed/skipped/cancelled \
                  per process) plus failed instances with item keys for triage. Item \
                  instances are run-scoped: the authored flow you pass in is the complete \
                  persistent state; nothing is added to the engine's DAG."
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
    /// Reuse instances whose inputs are unchanged since a previous run of
    /// the same flow (fingerprint match + output files still valid): only
    /// new or changed items execute. Nextflow's `-resume`.
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
fn build_flow_report_json(report: &dag_engine::FlowRunReport) -> Value {
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
                    &text[..text.len().min(MAX_ERROR_CHARS)]
                }),
            })
        })
        .collect();

    serde_json::json!({
        "ok": report.ok,
        "processes": processes,
        "failed_instances": failed,
        "failed_instances_truncated":
            report.instances.len().saturating_sub(MAX_LISTED_FAILURES),
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
                serde_json::from_value::<dag_engine::FlowGraph>(value).map_err(
                    |error| ToolError::ValidationFailed {
                        message: format!("invalid flow graph: {error}"),
                    },
                )?,
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
        let flow = serde_json::json!({
            "processes": [
                {
                    "id": "batch",
                    "kind": "echo",
                    "spec": {},
                    "cardinality": {
                        "type": "fanout",
                        "axis": "item",
                        "source": {"type": "items", "values": [1, 2, 3]}
                    }
                }
            ],
            "wires": []
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
        assert!(
            error.to_string().contains("invalid flow graph"),
            "{error}"
        );
    }
}
