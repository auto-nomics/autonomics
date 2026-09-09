use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::ReactomeClient;
use crate::format::{format_pathway, format_pathways};

#[tool(
    name = "reactome_top_pathways",
    description = "List top-level Reactome pathways for a species (e.g. 'Homo sapiens'). \
                  Returns stable IDs and display names."
)]
pub struct TopPathwaysInput {
    #[desc = "Species name, e.g. 'Homo sapiens' or 'Mus musculus'."]
    pub species: String,
}

pub struct ReactomeTopPathwaysTool {
    pub(crate) client: Arc<ReactomeClient>,
}

#[async_trait]
impl ToolFunction for ReactomeTopPathwaysTool {
    type Input = TopPathwaysInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let pathways = self
            .client
            .top_level_pathways(&input.species)
            .await
            .map_err(json_err)?;
        Ok(AgentToolResult::success(format_pathways(&pathways)))
    }
}

#[tool(
    name = "reactome_pathway_detail",
    description = "Retrieve detailed information for a single Reactome pathway by \
                  stable ID (e.g. 'R-HSA-1640170') or dbId."
)]
pub struct PathwayDetailInput {
    #[desc = "Reactome stable ID or numeric dbId."]
    pub id: String,
}

pub struct ReactomePathwayDetailTool {
    pub(crate) client: Arc<ReactomeClient>,
}

#[async_trait]
impl ToolFunction for ReactomePathwayDetailTool {
    type Input = PathwayDetailInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let detail = self.client.query(&input.id).await.map_err(json_err)?;
        let pathway: crate::types::Pathway = serde_json::from_value(detail)
            .map_err(|e| json_err(crate::ReactomeError::Decode(e)))?;
        Ok(AgentToolResult::success(format_pathway(&pathway)))
    }
}
