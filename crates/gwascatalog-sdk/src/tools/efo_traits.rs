use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{
    client::GwasCatalogClient,
    format,
    rest::{EfoQuery, EfoTrait, EmbeddedEfoTraits, RestPage},
};

#[tool(
    name = "gwascatalog_efo_traits",
    description = "Look up EFO (Experimental Factor Ontology) traits in the curated GWAS Catalog \
                  (REST API). Search by short form (e.g. `EFO_0001360`), URI, trait name, \
                  or PubMed ID. \
                  Returns trait name, short form, and ontology URI."
)]
pub struct EfoTraitsInput {
    #[desc = "EFO short form, e.g. `EFO_0001360`."]
    pub short_form: Option<String>,
    #[desc = "Full ontology URI, e.g. `http://www.ebi.ac.uk/efo/EFO_0001360`."]
    pub uri: Option<String>,
    #[desc = "Trait name (human-readable), e.g. `body mass index`."]
    pub trait_name: Option<String>,
    #[desc = "PubMed ID to find associated traits."]
    pub pubmed_id: Option<String>,
    #[desc = "Page number (0-indexed)."]
    pub page: Option<u32>,
    #[desc = "Page size (default 20)."]
    pub size: Option<u32>,
}

pub struct EfoTraitsTool {
    pub(crate) client: Arc<GwasCatalogClient>,
}

#[async_trait]
impl ToolFunction for EfoTraitsTool {
    type Input = EfoTraitsInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        // Direct GET for single short form
        if let Some(sf) = &input.short_form {
            if input.uri.is_none() && input.trait_name.is_none() && input.pubmed_id.is_none() {
                let trait_: EfoTrait = self
                    .client
                    .rest_get_efo_trait(sf)
                    .await
                    .map_err(super::json_err)?;
                let md = format::format_rest_efo_traits(&EmbeddedEfoTraits {
                    efo_traits: vec![trait_],
                });
                return Ok(AgentToolResult::success(md));
            }
        }

        let query = if let Some(sf) = input.short_form {
            EfoQuery::ShortForm(sf)
        } else if let Some(uri) = input.uri {
            EfoQuery::Uri(uri)
        } else if let Some(t) = input.trait_name {
            EfoQuery::Trait(t)
        } else if let Some(pm) = input.pubmed_id {
            EfoQuery::Pmid(pm)
        } else {
            return Ok(AgentToolResult::error(
                "Provide at least one of: short_form, uri, trait_name, pubmed_id.",
            ));
        };

        let resp: RestPage<EmbeddedEfoTraits> = self
            .client
            .rest_find_efo_traits(&query, input.page, input.size)
            .await
            .map_err(super::json_err)?;

        let embedded = resp._embedded.unwrap_or_default();
        let mut md = format::format_rest_efo_traits(&embedded);
        md.push_str(&format!(
            "\n*Page {} of {} ({} total)*\n",
            resp.page.number, resp.page.total_pages, resp.page.total_elements
        ));
        Ok(AgentToolResult::success(md))
    }
}
