use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::{helpers::query, json_err};
use crate::ChEMBLClient;
use crate::format::{format_molecule_search, format_target_search};

#[tool(
    name = "chembl_search",
    description = "Search ChEMBL molecules by name or research code, or search targets by \
                  preferred name. Returns compact previews with stable ChEMBL IDs. Use a \
                  returned ID before calling the summary or activity tools."
)]
pub struct SearchInput {
    #[desc = "Resource to search: 'molecule' (recommended) or 'target'."]
    pub resource: Option<String>,

    #[desc = "Name or code, e.g. 'aspirin' or 'imatinib'."]
    pub query: String,

    #[desc = "Number of results (default 20, max 100)."]
    pub limit: Option<u32>,

    #[desc = "Zero-based result offset for pagination."]
    pub offset: Option<u32>,
}

pub struct SearchTool {
    pub(crate) client: Arc<ChEMBLClient>,
}

#[async_trait]
impl ToolFunction for SearchTool {
    type Input = SearchInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let base_query = query(input.limit, input.offset);
        let output = match input.resource.as_deref().unwrap_or("molecule") {
            "molecule" => format_molecule_search(
                &self
                    .client
                    .molecules_by_name(
                        &input.query,
                        &base_query.clone().only([
                            "molecule_chembl_id",
                            "pref_name",
                            "molecule_type",
                            "max_phase",
                            "molecule_properties",
                            "molecule_structures",
                        ]),
                    )
                    .await
                    .map_err(json_err)?,
            ),
            "target" => format_target_search(
                &self
                    .client
                    .targets_by_name(
                        &input.query,
                        &base_query.clone().only([
                            "target_chembl_id",
                            "pref_name",
                            "organism",
                            "target_type",
                            "tax_id",
                            "target_components",
                        ]),
                    )
                    .await
                    .map_err(json_err)?,
            ),
            other => {
                return Err(ToolError::ValidationFailed {
                    message: format!("unsupported chembl_search resource '{other}'"),
                });
            }
        };
        Ok(AgentToolResult::success(output))
    }
}
