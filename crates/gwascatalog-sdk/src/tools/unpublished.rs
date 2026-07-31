use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{
    client::GwasCatalogClient,
    format,
    rest::{EmbeddedUnpublishedStudies, RestPage, UnpublishedFilter},
};

#[tool(
    name = "gwascatalog_unpublished_studies",
    description = "Search pre-publication GWAS submissions in the GWAS Catalog \
                  (REST API `/unpublished-studies`). \
                  Filter by first author, accession, title, or trait. \
                  These are studies submitted before journal publication."
)]
pub struct UnpublishedInput {
    #[desc = "First author name."]
    pub first_author: Option<String>,
    #[desc = "Study accession ID."]
    pub accession: Option<String>,
    #[desc = "Title (partial match)."]
    pub title: Option<String>,
    #[desc = "Trait name."]
    pub trait_name: Option<String>,
    #[desc = "Page number (0-indexed)."]
    pub page: Option<u32>,
    #[desc = "Page size (default 20)."]
    pub size: Option<u32>,
}

pub struct UnpublishedTool {
    pub(crate) client: Arc<GwasCatalogClient>,
}

#[async_trait]
impl ToolFunction for UnpublishedTool {
    type Input = UnpublishedInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let filter = UnpublishedFilter {
            first_author: input.first_author,
            accession: input.accession,
            title: input.title,
            trait_: input.trait_name,
        };
        let resp: RestPage<EmbeddedUnpublishedStudies> = self
            .client
            .rest_unpublished_studies(&filter, input.page, input.size)
            .await
            .map_err(super::json_err)?;

        let embedded = resp._embedded.unwrap_or_default();
        let mut md = format::format_unpublished(&embedded);
        md.push_str(&format!(
            "\n*Page {} of {} ({} total)*\n",
            resp.page.number, resp.page.total_pages, resp.page.total_elements
        ));
        Ok(AgentToolResult::success(md))
    }
}
