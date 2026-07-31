use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{
    client::GwasCatalogClient,
    format,
    rest::{AssociationQueryKind, EmbeddedRestAssociations, RestPage},
};

#[tool(
    name = "gwascatalog_associations",
    description = "Find curated GWAS Catalog associations (REST API). \
                  Search by rsID, rsID + study accession, PubMed ID, or EFO trait. \
                  Returns p-value, odds ratio / beta, risk alleles, and author-reported genes. \
                  \
                  Provide exactly one search criterion. If both `rs_id` and `accession_id` \
                  are given, uses the combined rsID+accession search."
)]
pub struct AssociationsInput {
    #[desc = "Variant rsID, e.g. `rs7329174`."]
    pub rs_id: Option<String>,
    #[desc = "Study accession ID (e.g. `GCST005038`). Combined with rs_id if both given."]
    pub accession_id: Option<String>,
    #[desc = "PubMed ID to find associations."]
    pub pubmed_id: Option<String>,
    #[desc = "EFO trait label."]
    pub efo_trait: Option<String>,
    #[desc = "Page number (0-indexed)."]
    pub page: Option<u32>,
    #[desc = "Page size (default 20)."]
    pub size: Option<u32>,
}

pub struct AssociationsTool {
    pub(crate) client: Arc<GwasCatalogClient>,
}

#[async_trait]
impl ToolFunction for AssociationsTool {
    type Input = AssociationsInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let query = match (&input.rs_id, &input.accession_id) {
            (Some(rs), Some(acc)) => AssociationQueryKind::RsIdAndAccession {
                rs_id: rs.clone(),
                accession: acc.clone(),
            },
            (Some(rs), None) => AssociationQueryKind::RsId(rs.clone()),
            (None, Some(acc)) => AssociationQueryKind::StudyAccession(acc.clone()),
            _ => {
                if let Some(pm) = input.pubmed_id {
                    AssociationQueryKind::Pmid(pm)
                } else if let Some(t) = input.efo_trait {
                    AssociationQueryKind::EfoTrait(t)
                } else {
                    return Ok(AgentToolResult::error(
                        "Provide at least one of: rs_id, accession_id, pubmed_id, efo_trait.",
                    ));
                }
            }
        };

        let resp: RestPage<EmbeddedRestAssociations> = self
            .client
            .rest_find_associations(&query, input.page, input.size)
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
