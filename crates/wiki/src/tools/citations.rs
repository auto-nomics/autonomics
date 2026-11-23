//! `wiki_citations`:词条引用的文献键双向查询(pandoc `[@citekey]` 边表)。

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use serde_json::json;

use super::{id_title_pairs, resolve_entry, tool_err};
use crate::service::WikiHandle;

#[tool(
    name = "wiki_citations",
    description = "Query the literature-citation edges between wiki entries and the \
                   bibliography. Two directions, one per call: pass `entry` (id or \
                   title) to list the citekeys the entry's body cites \
                   ([@smith2024a-k3x9] syntax, deduplicated); pass `citekey` to \
                   list the entries citing that article. Dangling citekeys \
                   (article not yet imported) are kept in the index, so a \
                   reverse lookup works even before import. Resolve citekeys \
                   to article metadata with `bib_resolve_citekeys`."
)]
pub struct WikiCitationsInput {
    #[desc = "Entry id (UUID string) — list the citekeys this entry cites"]
    pub id: Option<String>,
    #[desc = "Exact entry title — alternative to id"]
    pub title: Option<String>,
    #[desc = "Citekey (e.g. smith2024a-k3x9) — list entries citing this article"]
    pub citekey: Option<String>,
}

pub struct WikiCitationsTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiCitationsTool {
    type Input = WikiCitationsInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        if let Some(citekey) = input.citekey.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
            if input.id.is_some() || input.title.is_some() {
                return Err(ToolError::ValidationFailed {
                    message: "pass either `citekey` (reverse lookup) or `id`/`title` \
                               (forward lookup), not both"
                        .into(),
                });
            }
            let entries = self
                .handle
                .get_citing_entries(citekey)
                .await
                .map_err(tool_err)?;
            return Ok(ToolResult::success_json(json!({
                "citekey": citekey,
                "citing_entries": id_title_pairs(&self.handle, entries).await,
            })));
        }

        let e = resolve_entry(&self.handle, input.id, input.title).await?;
        let citekeys = self
            .handle
            .get_citations(&e.id)
            .await
            .map_err(tool_err)?;
        Ok(ToolResult::success_json(json!({
            "entry": { "id": e.id, "title": e.title },
            "citekeys": citekeys,
        })))
    }
}
