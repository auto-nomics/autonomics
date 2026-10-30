//! `wiki_search`:BM25 全文检索(可选标签 AND 过滤),结果带正文摘要。

use std::collections::HashMap;
use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use serde_json::json;

use super::tool_err;
use crate::service::WikiHandle;
use crate::storage::types::{KnowledgeEntryData, SearchFilter};

/// snippet 截断长度(字符)。
const SNIPPET_CHARS: usize = 160;
/// 检索默认与上限。
const DEFAULT_LIMIT: usize = 8;
const MAX_LIMIT: usize = 50;

/// 正文摘要:前 `SNIPPET_CHARS` 个字符,截断时补省略号。
fn snippet(content: &str) -> String {
    if content.chars().count() <= SNIPPET_CHARS {
        content.to_string()
    } else {
        let head: String = content.chars().take(SNIPPET_CHARS).collect();
        format!("{head}…")
    }
}

#[tool(
    name = "wiki_search",
    description = "Full-text search over wiki entries (BM25, Chinese + English). \
                   Returns id/title/score/tags/content-snippet ranked by relevance; \
                   use wiki_get for the full body. Optional tag filter is AND semantics."
)]
pub struct WikiSearchInput {
    #[desc = "Search query — Chinese or English, mixed terms allowed"]
    pub query: String,
    #[desc = "Max hits to return (default 8, max 50)"]
    pub limit: Option<usize>,
    #[desc = "Require ALL of these tags on hits (AND); exact match after trim"]
    pub tags: Option<Vec<String>>,
}

pub struct WikiSearchTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiSearchTool {
    type Input = WikiSearchInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        if input.query.trim().is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "查询为空".to_string(),
            });
        }
        let limit = input.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let hits = match input.tags {
            Some(tags) if !tags.is_empty() => {
                let filter = SearchFilter {
                    tags,
                    ..Default::default()
                };
                self.handle
                    .search_filtered(&input.query, limit, &filter)
                    .await
                    .map_err(tool_err)?
            }
            _ => self
                .handle
                .search(&input.query, limit)
                .await
                .map_err(tool_err)?,
        };
        let ids: Vec<String> = hits.iter().map(|h| h.entry_id.clone()).collect();
        let entries = self
            .handle
            .get_entry_batch(
                ids.iter()
                    .filter_map(|s| uuid::Uuid::parse_str(s).ok())
                    .collect(),
            )
            .await
            .map_err(tool_err)?;
        let by_id: HashMap<String, &KnowledgeEntryData> =
            entries.iter().map(|e| (e.id.clone(), e)).collect();
        let results: Vec<serde_json::Value> = hits
            .iter()
            .filter_map(|hit| {
                let e = by_id.get(&hit.entry_id)?;
                Some(json!({
                    "id": e.id,
                    "title": e.title,
                    "score": hit.score,
                    "tags": e.tags,
                    "snippet": snippet(&e.content),
                }))
            })
            .collect();
        Ok(ToolResult::success_json(json!({ "results": results })))
    }
}
