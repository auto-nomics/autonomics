//! `wiki_links`:词条的出链/入链清单({id, title} 形式)。

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use serde_json::json;

use super::{id_title_pairs, resolve_entry, tool_err};
use crate::service::WikiHandle;

#[tool(
    name = "wiki_links",
    description = "List an entry's out-links (targets it points to via [[name]]) and \
                   in-links (entries pointing to it), each as {id, title}. \
                   Recycle-bin entries are excluded."
)]
pub struct WikiLinksInput {
    #[desc = "Entry id (UUID string)"]
    pub id: Option<String>,
    #[desc = "Exact entry title"]
    pub title: Option<String>,
}

pub struct WikiLinksTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiLinksTool {
    type Input = WikiLinksInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let e = resolve_entry(&self.handle, input.id, input.title).await?;
        let out = self
            .handle
            .get_out_links_batch(std::slice::from_ref(&e.id))
            .await
            .map_err(tool_err)?
            .into_iter()
            .next()
            .unwrap_or_default();
        let inn = self
            .handle
            .get_in_links_batch(std::slice::from_ref(&e.id))
            .await
            .map_err(tool_err)?
            .into_iter()
            .next()
            .unwrap_or_default();
        Ok(ToolResult::success_json(json!({
            "entry": { "id": e.id, "title": e.title },
            "out_links": id_title_pairs(&self.handle, out).await,
            "in_links": id_title_pairs(&self.handle, inn).await,
        })))
    }
}
