use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{EmbaseClient, format::format_retrieval, types::RetrievalId};

#[tool(
    name = "embase_retrieve",
    description = "Retrieve a full Embase record by identifier (DOI, PII, PubMed ID, \
                  MEDLINE ID, Embase accession number, or LUI). Returns the complete \
                  article record including title, author, full abstract, journal \
                  details, ISSN, and DOI. More detailed than embase_search."
)]
pub struct EmbaseRetrieveInput {
    #[desc = "Type of identifier to retrieve by. One of: 'doi', 'pii', 'pubmed_id', \
             'medline', 'embase' (accession number), 'lui' (internal ID). \
             Default: 'doi'."]
    pub id_type: Option<String>,

    #[desc = "The identifier value (e.g. '10.1016/j.cell.2024.01.001' for DOI, \
             '12345678' for Embase accession number)."]
    pub id: String,
}

pub struct EmbaseRetrieveTool {
    pub(crate) client: Arc<EmbaseClient>,
}

#[async_trait]
impl ToolFunction for EmbaseRetrieveTool {
    type Input = EmbaseRetrieveInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let id_type = input
            .id_type
            .as_deref()
            .map(parse_id_type)
            .unwrap_or(RetrievalId::Doi);

        let resp = self
            .client
            .retrieve(id_type, &input.id)
            .await
            .map_err(super::json_err)?;

        Ok(AgentToolResult::success(format_retrieval(&resp.entry)))
    }
}

fn parse_id_type(s: &str) -> RetrievalId {
    match s.to_ascii_lowercase().as_str() {
        "pii" => RetrievalId::Pii,
        "pubmed_id" | "pubmedid" | "pmid" | "pubmed" => RetrievalId::PubmedId,
        "medline" => RetrievalId::Medline,
        "embase" => RetrievalId::Embase,
        "lui" => RetrievalId::Lui,
        _ => RetrievalId::Doi,
    }
}
