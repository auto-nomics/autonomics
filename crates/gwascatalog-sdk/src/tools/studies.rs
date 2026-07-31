use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{
    client::GwasCatalogClient,
    format,
    rest::{EmbeddedRestStudies, RestPage, RestStudy, StudyQuery},
};

#[tool(
    name = "gwascatalog_studies",
    description = "Find curated GWAS Catalog studies (REST API). Search by accession ID \
                  (GCST...), EFO trait, EFO URI, disease trait, PubMed ID, or the boolean \
                  flags userRequested / fullPvalueSet. \
                  \
                  If no filter is provided, lists all studies (paginated). \
                  Returns accession ID, trait, publication metadata (PMID, title, date, author), \
                  sample size, and ancestry info."
)]
pub struct StudiesInput {
    #[desc = "Study accession ID, e.g. `GCST005038`."]
    pub accession_id: Option<String>,
    #[desc = "EFO trait label to search by, e.g. `body mass index`."]
    pub efo_trait: Option<String>,
    #[desc = "EFO ontology URI, e.g. `http://www.ebi.ac.uk/efo/EFO_0004343`."]
    pub efo_uri: Option<String>,
    #[desc = "Reported disease trait name."]
    pub disease_trait: Option<String>,
    #[desc = "PubMed ID to find associated studies."]
    pub pubmed_id: Option<String>,
    #[desc = "Filter by user-requested status (true/false)."]
    pub user_requested: Option<bool>,
    #[desc = "Filter by full p-value set availability (true/false)."]
    pub full_pvalue_set: Option<bool>,
    #[desc = "Page number (0-indexed)."]
    pub page: Option<u32>,
    #[desc = "Page size (default 20)."]
    pub size: Option<u32>,
}

pub struct StudiesTool {
    pub(crate) client: Arc<GwasCatalogClient>,
}

#[async_trait]
impl ToolFunction for StudiesTool {
    type Input = StudiesInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        // Direct GET for single accession (findByAccessionId returns a flat entity)
        if let Some(id) = &input.accession_id {
            if input.efo_trait.is_none()
                && input.efo_uri.is_none()
                && input.disease_trait.is_none()
                && input.pubmed_id.is_none()
                && input.user_requested.is_none()
                && input.full_pvalue_set.is_none()
            {
                let study: RestStudy = self
                    .client
                    .rest_get_study(id)
                    .await
                    .map_err(super::json_err)?;
                let md = format::format_rest_studies(&EmbeddedRestStudies {
                    studies: vec![study],
                });
                return Ok(AgentToolResult::success(md));
            }
        }

        let page = input.page;
        let size = input.size;
        let query = if let Some(t) = input.efo_trait {
            StudyQuery::EfoTrait(t)
        } else if let Some(uri) = input.efo_uri {
            StudyQuery::EfoUri(uri)
        } else if let Some(t) = input.disease_trait {
            StudyQuery::DiseaseTrait(t)
        } else if let Some(pm) = input.pubmed_id {
            StudyQuery::Pmid(pm)
        } else if let Some(v) = input.user_requested {
            StudyQuery::UserRequested(v)
        } else if let Some(v) = input.full_pvalue_set {
            StudyQuery::FullPvalueSet(v)
        } else {
            // No filter → list all
            let resp: RestPage<EmbeddedRestStudies> = self
                .client
                .rest_studies(page, size)
                .await
                .map_err(super::json_err)?;
            let embedded = resp._embedded.unwrap_or_default();
            let mut md = format::format_rest_studies(&embedded);
            md.push_str(&format!(
                "\n*Page {} of {} ({} total)*\n",
                resp.page.number, resp.page.total_pages, resp.page.total_elements
            ));
            return Ok(AgentToolResult::success(md));
        };

        let resp: RestPage<EmbeddedRestStudies> = self
            .client
            .rest_find_studies(&query, page, size)
            .await
            .map_err(super::json_err)?;

        let embedded = resp._embedded.unwrap_or_default();
        let mut md = format::format_rest_studies(&embedded);
        md.push_str(&format!(
            "\n*Page {} of {} ({} total)*\n",
            resp.page.number, resp.page.total_pages, resp.page.total_elements
        ));
        Ok(AgentToolResult::success(md))
    }
}
