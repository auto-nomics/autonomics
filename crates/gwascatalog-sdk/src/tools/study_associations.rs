use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{
    client::GwasCatalogClient,
    format,
    rest::{EmbeddedRestAssociations, RestPage},
};

#[tool(
    name = "gwascatalog_study_associations",
    description = "Get curated SNP-trait associations for a GWAS Catalog study \
                  (by accession ID, e.g. GCST005038). \
                  Returns rsID, p-value, odds ratio / beta, risk alleles, and \
                  author-reported genes for each association."
)]
pub struct StudyAssociationsInput {
    #[desc = "Study accession ID (e.g. `GCST005038`)."]
    pub accession_id: String,
    #[desc = "Page number (0-indexed)."]
    pub page: Option<u32>,
    #[desc = "Page size (default 20)."]
    pub size: Option<u32>,
}

pub struct StudyAssociationsTool {
    pub(crate) client: Arc<GwasCatalogClient>,
}

#[async_trait]
impl ToolFunction for StudyAssociationsTool {
    type Input = StudyAssociationsInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let resp: RestPage<EmbeddedRestAssociations> = self
            .client
            .rest_study_associations(&input.accession_id, input.page, input.size)
            .await
            .map_err(super::json_err)?;

        let embedded = resp._embedded.unwrap_or_default();
        let mut md = format::format_rest_associations(&embedded);
        md.push_str(&format!(
            "\n*Page {} of {} ({} total)*\n",
            resp.page.number, resp.page.total_pages, resp.page.total_elements
        ));
        Ok(AgentToolResult::success(md))
    }
}
