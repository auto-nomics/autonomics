use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "dag_export_run",
    description = "Export a recorded DAG run as deliverable provenance evidence. \
                  `crate` (default) produces an RO-Crate 1.1 directory: manifest + \
                  the run's result files pulled from object storage with their \
                  sha256 verified against the audit record + the executed DAG \
                  definition; input data is referenced by uri + hash, not copied. \
                  `prov` produces a single W3C PROV-JSON document (entities/\
                  activities/agents, machine-queryable lineage). Use this when \
                  delivering evidence for a result (publication, audit, handoff) \
                  or packaging a run for an archive."
)]
pub struct DagExportRunInput {
    /// Run id (or unique prefix) from dag_runs_log.
    pub run_id: String,
    /// `crate` (RO-Crate directory) or `prov` (PROV-JSON). Default `crate`.
    pub format: Option<String>,
    /// Absolute output directory. For `crate` it holds the crate; for `prov`
    /// the document is written inside it as `prov-<run_id>.json`.
    pub out_dir: String,
}

pub struct DagExportRunTool {
    client: Arc<DataEngineClient>,
}

impl DagExportRunTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for DagExportRunTool {
    type Input = DagExportRunInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let out_dir = std::path::PathBuf::from(&input.out_dir);
        if !out_dir.is_absolute() {
            return Ok(ToolResult::error(format!(
                "out_dir must be an absolute path, got `{}`",
                input.out_dir
            )));
        }
        let format = input.format.unwrap_or_else(|| "crate".to_string());
        let summary = self
            .client
            .export_run(input.run_id.clone(), format, out_dir)
            .await
            .map_err(ExecError::from)?;
        let summary_json = serde_json::to_value(&summary).map_err(|e| {
            ToolError::ValidationFailed {
                message: format!("serialize export summary: {e}"),
            }
        })?;
        Ok(ToolResult::success_json(summary_json))
    }
}
