//! `wiki_list`:无查询浏览——按标签(AND)与 mtime 时间窗列词条摘要。

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use serde_json::json;

use super::tool_err;
use crate::service::WikiHandle;
use crate::storage::types::ListFilter;

/// 浏览默认与上限。
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;

#[tool(
    name = "wiki_list",
    description = "Browse wiki entries WITHOUT a text query: newest first, filterable by \
                   tags (AND) and/or an mtime window, with offset/limit paging. Returns \
                   metadata only (no content) — use wiki_get for bodies, wiki_search for \
                   relevance-ranked text search."
)]
pub struct WikiListInput {
    #[desc = "Require ALL of these tags (AND); exact match after trim"]
    pub tags: Option<Vec<String>>,
    #[desc = "Only entries modified at/after this unix timestamp (seconds)"]
    pub mtime_since: Option<i64>,
    #[desc = "Only entries modified at/before this unix timestamp (seconds)"]
    pub mtime_until: Option<i64>,
    #[desc = "Page size (default 50, max 200)"]
    pub limit: Option<usize>,
    #[desc = "Entries to skip before the first result (default 0)"]
    pub offset: Option<usize>,
}

pub struct WikiListTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiListTool {
    type Input = WikiListInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let filter = ListFilter {
            tags: input.tags.unwrap_or_default(),
            mtime_since: input.mtime_since,
            mtime_until: input.mtime_until,
            limit: input.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
            offset: input.offset.unwrap_or(0),
        };
        let entries = self.handle.list_entries(&filter).await.map_err(tool_err)?;
        let count = entries.len();
        Ok(ToolResult::success_json(json!({
            "count": count,
            "offset": filter.offset,
            "entries": entries.iter().map(|e| json!({
                "id": e.id,
                "title": e.title,
                "alias": e.alias,
                "tags": e.tags,
                "ctime": e.ctime,
                "mtime": e.mtime,
            })).collect::<Vec<_>>(),
        })))
    }
}
