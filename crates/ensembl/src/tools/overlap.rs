use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::EnsemblClient;
use crate::format::format_overlap;

#[tool(
    name = "ensembl_overlap",
    description = "Preview genomic features overlapping a region. Use this for exploratory annotation counts; future DAG nodes can call the same SDK method and emit the full feature table."
)]
pub struct EnsemblOverlapInput {
    #[desc = "Species name, alias, common name, or taxonomy ID, e.g. 'human'."]
    pub species: String,

    #[desc = "Region in chromosome:start-end form, e.g. '13:32355000-32357000'. An optional ':strand' suffix is accepted."]
    pub region: String,

    #[desc = "Feature types: gene, transcript, exon, cdna, cds, five_prime_utr, three_prime_utr, variation, regulatory_feature, motif, etc."]
    pub features: Vec<String>,
}

pub struct EnsemblOverlapTool {
    pub(crate) client: Arc<EnsemblClient>,
}

#[async_trait]
impl ToolFunction for EnsemblOverlapTool {
    type Input = EnsemblOverlapInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let features = self
            .client
            .overlap_region(&input.species, &input.region, &input.features)
            .await
            .map_err(json_err)?;
        Ok(AgentToolResult::success(format_overlap(&features)))
    }
}
