use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{EuropePmcClient, format::format_article};

#[tool(
    name = "europepmc_article",
    description = "Retrieve a full article record from Europe PMC by source and ID. \
                  Returns complete metadata: title, full author list with affiliations \
                  and ORCIDs, abstract, MeSH terms, publication types, journal info \
                  (ISSN, ESSN), and citation count. \
                  \
                  The most common source is 'MED' (PubMed/MEDLINE). Use 'PPR' for \
                  preprints, 'PMC' for PubMed Central full texts. \
                  \
                  Example: source='MED', id='29867326' returns the record for PMID 29867326."
)]
pub struct EuropePmcArticleInput {
    #[desc = "Three-letter source code. Common values: 'MED' (PubMed), 'PMC' (PubMed \
             Central), 'PPR' (preprint), 'PAT' (patent), 'CTX' (clinical guideline). \
             Default: 'MED'."]
    pub source: Option<String>,

    #[desc = "Publication identifier within the source (e.g. PubMed ID '29867326', \
             PMC ID 'PMC3258128')."]
    pub id: String,
}

pub struct EuropePmcArticleTool {
    pub(crate) client: Arc<EuropePmcClient>,
}

#[async_trait]
impl ToolFunction for EuropePmcArticleTool {
    type Input = EuropePmcArticleInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let source = input
            .source
            .as_deref()
            .map(crate::types::Source::parse_source)
            .unwrap_or(None)
            .unwrap_or(crate::types::Source::Med);

        let resp = self
            .client
            .article(source, &input.id, crate::types::ResultType::Core)
            .await
            .map_err(super::json_err)?;

        if let Some(result) = resp.result {
            Ok(AgentToolResult::success(format_article(&result)))
        } else {
            Ok(AgentToolResult::success(
                "No article found for the given source and ID.".to_string(),
            ))
        }
    }
}
