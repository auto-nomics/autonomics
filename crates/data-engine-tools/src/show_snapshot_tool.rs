use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "show_snapshot",
    description = "Show details of a specific history snapshot: metadata, manifest (nodes + edges), \
                  and run report if available. Use dag_history_log to find snapshot ids. \
                  Short-hash prefixes are accepted."
)]
pub struct ShowSnapshotInput {
    /// Snapshot id or short-hash prefix.
    pub snapshot_id: String,
}

pub struct ShowSnapshotTool {
    client: Arc<DataEngineClient>,
}

impl ShowSnapshotTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for ShowSnapshotTool {
    type Input = ShowSnapshotInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let snap = self
            .client
            .get_snapshot(input.snapshot_id.clone())
            .await
            .map_err(ExecError::from)?
            .ok_or_else(|| ToolError::ExecutionFailed {
                source: Box::new(ExecError::from(format!(
                    "snapshot '{}' not found",
                    input.snapshot_id
                ))),
            })?;

        let mut out = format!(
            "Snapshot:   {}\n\
             Parent:     {}\n\
             Timestamp:  {}\n\
             Message:    {}\n\
             Engine:     {}\n\
             Manifest:   {}\n",
            snap.id,
            snap.parent_id.as_deref().unwrap_or("(root)"),
            snap.timestamp,
            snap.message,
            snap.engine_version,
            snap.manifest_hash,
        );

        // Manifest summary.
        if let Ok(manifest) = snap.manifest() {
            out.push_str(&format!(
                "\nManifest ({} nodes, {} edges):\n",
                manifest.nodes.len(),
                manifest.edges.len()
            ));
            for n in &manifest.nodes {
                out.push_str(&format!("  • {} ({})\n", n.id, n.kind));
            }
            for e in &manifest.edges {
                out.push_str(&format!(
                    "  • {}.[{}] → {}.[{}]\n",
                    e.from, e.from_port, e.to, e.to_port
                ));
            }
        }

        // Run report summary.
        if let Some(rr) = &snap.run_report_json {
            if let Ok(rr) = serde_json::from_str::<serde_json::Value>(rr) {
                let ok = rr.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                out.push_str(&format!("\nRun report: ok={ok}\n"));
                if let Some(nodes) = rr.get("nodes").and_then(|v| v.as_array()) {
                    for node in nodes {
                        let id = node.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                        let status = node.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                        let ty = node
                            .get("node_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?");
                        out.push_str(&format!("  {id:<20} {status:<10} ({ty})\n"));
                    }
                }
            }
        }

        Ok(ToolResult::success(out))
    }
}
