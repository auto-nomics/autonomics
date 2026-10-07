use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

/// Default page size when the caller omits `limit`. Keeps tool output compact
/// — most sessions only have a handful of analysis lineages, so 10 is plenty
/// for an at-a-glance overview.
const DEFAULT_LIMIT: usize = 10;

/// Hard ceiling on refs rendered per call. Prevents a pathologically large
/// request from dumping every lineage into context at once; use `offset` to
/// page instead.
const MAX_LIMIT: usize = 50;

#[tool(
    name = "list_dag_refs",
    description = "List all history refs (branches/tags) and the snapshot each \
                  currently points to. The current active ref is marked with `*`. \
                  Output is paginated (default 10, max 50 refs per call) — pass \
                  `offset` to page through additional refs."
)]
pub struct ListDagRefsInput {
    /// Number of refs to skip from the beginning. Defaults to 0.
    pub offset: Option<usize>,
    /// Maximum number of refs to return. Defaults to 10 (`DEFAULT_LIMIT`). \
    /// Any request above `MAX_LIMIT` is hard-clamped — pass `offset` to page.
    pub limit: Option<usize>,
}

pub struct ListDagRefsTool {
    client: Arc<DataEngineClient>,
}

impl ListDagRefsTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for ListDagRefsTool {
    type Input = ListDagRefsInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let offset = input.offset.unwrap_or(0);
        let requested = input.limit.unwrap_or(DEFAULT_LIMIT);
        let limit = requested.min(MAX_LIMIT).max(1);

        let refs = self.client.list_dag_refs().await.map_err(ExecError::from)?;
        let total = refs.len();

        if total == 0 {
            return Ok(ToolResult::success(
                "No history refs found (no history store attached).",
            ));
        }

        let current_ref = self.client.get_dag_ref().await.map_err(ExecError::from)?;

        let page: Vec<(String, String, bool)> = refs
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect();

        if page.is_empty() {
            return Ok(ToolResult::success(format!(
                "No refs at offset {offset} (total: {total})."
            )));
        }

        let mut out = String::new();
        for (name, snap_id, pinned) in &page {
            let short_id = if snap_id.is_empty() {
                "(empty)"
            } else {
                &snap_id[..12.min(snap_id.len())]
            };
            let marker = if name == &current_ref { " * " } else { "   " };
            let ty = if *pinned { "tag" } else { "branch" };
            out.push_str(&format!("{marker}{name:<24} {short_id}  ({ty})\n"));
        }

        let shown_end = offset + page.len();
        let truncated = shown_end < total;
        match (truncated, requested > MAX_LIMIT) {
            (true, true) => out.push_str(&format!(
                "\n* = current active ref\nshowing {offset}..{shown_end} of {total} \
                 (limit clamped to {MAX_LIMIT}; page with offset)"
            )),
            (true, false) => out.push_str(&format!(
                "\n* = current active ref\nshowing {offset}..{shown_end} of {total} \
                 (page with offset)"
            )),
            (false, true) => out.push_str(&format!(
                "\n* = current active ref\n{shown_end} refs shown (limit clamped to {MAX_LIMIT})"
            )),
            (false, false) => out.push_str(&format!(
                "\n* = current active ref\n{total} refs shown"
            )),
        }

        Ok(ToolResult::success(out))
    }
}
