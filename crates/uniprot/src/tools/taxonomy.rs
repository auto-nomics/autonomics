use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::UniProtClient;
use crate::format::format_taxa;
use crate::types::SearchRequest;

#[tool(
    name = "uniprot_taxonomy",
    description = "Search the UniProt taxonomy (NCBI lineage) to resolve species names \
                  to taxon IDs and inspect ranks/lineages. \
                  \
                  Provide `taxon_id` for an exact lookup (e.g. 9606) and/or `search` \
                  for free text over scientific/common names (e.g. \"sapiens\"). \
                  Results include rank, parent, and full lineage. \
                  Use the returned taxon ID in uniprot_search's organism_id filter."
)]
pub struct UniprotTaxonomyInput {
    #[desc = "Exact NCBI taxon ID, e.g. 9606 (Homo sapiens), 2697049 (SARS-CoV-2)."]
    pub taxon_id: Option<u64>,

    #[desc = "Free-text search over scientific and common names, \
             e.g. \"homo sapiens\", \"mouse\"."]
    pub search: Option<String>,

    #[desc = "Maximum number of taxa to return (default 25, max 500)."]
    pub size: Option<u32>,

    #[desc = "Cursor from a previous response to fetch the next page."]
    pub cursor: Option<String>,
}

pub struct UniprotTaxonomyTool {
    pub(crate) client: Arc<UniProtClient>,
}

#[async_trait]
impl ToolFunction for UniprotTaxonomyTool {
    type Input = UniprotTaxonomyInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let mut req = match (input.taxon_id, input.search) {
            (Some(id), _) => SearchRequest::new(format!("id:{id}")),
            (None, Some(s)) if !s.trim().is_empty() => {
                SearchRequest::new(format!("\"{}\"", s.trim().replace('"', " ")))
            }
            (None, _) => {
                return Err(ToolError::ValidationFailed {
                    message: "uniprot_taxonomy requires `taxon_id` or `search`".into(),
                });
            }
        };

        if let Some(size) = input.size {
            req = req.size(size);
        }
        if let Some(cursor) = input.cursor {
            req = req.cursor(cursor);
        }

        let page = self
            .client
            .search_taxonomy(&req)
            .await
            .map_err(super::json_err)?;
        Ok(AgentToolResult::success(format_taxa(&page)))
    }
}
