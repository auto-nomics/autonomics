use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::format::{format_agency, format_work};
use crate::CrossrefClient;

/// Retrieve a single Crossref work by its DOI, or look up the registration
/// agency for a DOI.
///
/// This returns full metadata: title, full author list with affiliations and
/// ORCIDs, abstract, journal info (ISSN, ESSN), funder data, license, citation
/// count, subjects, and references count.
///
/// Example: doi='10.1037/0003-066X.59.1.29'
#[tool(
    name = "crossref_doi",
    description = "Retrieve a full metadata record from Crossref by DOI, or look up the \
                  registration agency for a DOI. Returns complete metadata: title, full \
                  author list with affiliations and ORCIDs, abstract, journal info, \
                  funder data, license, citation count, and subjects. \
                  \
                  Set `agency=true` to only look up which registration agency owns the DOI \
                  (Crossref, DataCite, mEDRA, etc.) without fetching full metadata. \
                  \
                  Example: doi='10.1037/0003-066X.59.1.29'"
)]
pub struct CrossrefDoiInput {
    #[desc = "The DOI to look up (with or without the 'https://doi.org/' prefix)."]
    pub doi: String,

    #[desc = "If true, only look up the registration agency (Crossref / DataCite / \
             mEDRA) without fetching the full metadata record. Default: false."]
    #[serde(default)]
    pub agency: bool,
}

pub struct CrossrefDoiTool {
    pub client: Arc<CrossrefClient>,
}

#[async_trait]
impl ToolFunction for CrossrefDoiTool {
    type Input = CrossrefDoiInput;

    fn timeout_seconds(&self) -> u64 {
        60
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        if input.agency {
            let resp = self
                .client
                .works_agency(&input.doi)
                .await
                .map_err(super::json_err)?;
            Ok(AgentToolResult::success(format_agency(&resp)))
        } else {
            let resp = self
                .client
                .works_by_doi(&input.doi)
                .await
                .map_err(super::json_err)?;
            match resp.message {
                Some(work) => Ok(AgentToolResult::success(format_work(&work))),
                None => Ok(AgentToolResult::success(
                    "No work found for the given DOI.".to_string(),
                )),
            }
        }
    }
}
