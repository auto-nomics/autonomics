use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "dag_history_log",
    description = "Show the snapshot history for a ref (default: the current ref). \
                  Each snapshot records the full DAG manifest and optional run report. \
                  Snapshots are listed newest-first. Use this to review past analysis \
                  iterations, find a snapshot to restore, or compare two snapshots."
)]
pub struct DagHistoryLogInput {
    /// Ref name to inspect. If omitted, uses the current active ref.
    pub ref_name: Option<String>,
    /// Maximum number of snapshots to return (default 20).
    pub limit: Option<usize>,
}

pub struct DagHistoryLogTool {
    client: Arc<DataEngineClient>,
}

impl DagHistoryLogTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for DagHistoryLogTool {
    type Input = DagHistoryLogInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let limit = input.limit.unwrap_or(20);
        let snapshots = self
            .client
            .dag_log(input.ref_name.clone(), limit)
            .await
            .map_err(ExecError::from)?;

        if snapshots.is_empty() {
            return Ok(ToolResult::success("No snapshots found for this ref."));
        }

        let mut out = String::new();
        for snap in &snapshots {
            let short_id = &snap.id[..12.min(snap.id.len())];
            let parent = snap
                .parent_id
                .as_ref()
                .map(|p| &p[..12.min(p.len())])
                .unwrap_or("(root)");
            out.push_str(&format!(
                "{short_id}  parent={parent}  {}  {msg}\n",
                &snap.timestamp[..19.min(snap.timestamp.len())],
                msg = snap.message
            ));
        }
        out.push_str(&format!("\n{} snapshots (limit {limit})", snapshots.len()));

        Ok(ToolResult::success(out))
    }
}
