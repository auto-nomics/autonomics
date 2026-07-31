use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{
    client::GwasCatalogClient,
    format,
    summary_stats::{AssociationQuery, EmbeddedAssociations, PaginatedResponse, RevealMode},
};

#[tool(
    name = "gwascatalog_summary_variant",
    description = "Fetch summary statistics (Summary Statistics API) for a single variant \
                  (by rsID), optionally restricted to a chromosome or study. \
                  Faster than iterating all associations when you know the variant. \
                  \
                  If `chromosome` is given, uses the chromosome-specific endpoint for speed. \
                  `reveal`: `raw` for original values, `all` for harmonised + raw."
)]
pub struct SummaryVariantInput {
    #[desc = "Variant rsID, e.g. `rs7903146`."]
    pub variant_id: String,
    #[desc = "Chromosome number (e.g. `10`) for the faster chromosome-scoped lookup."]
    pub chromosome: Option<String>,
    #[desc = "Study accession ID to filter within the variant."]
    pub study_accession: Option<String>,
    #[desc = "Lower p-value threshold."]
    pub p_lower: Option<f64>,
    #[desc = "Upper p-value threshold."]
    pub p_upper: Option<f64>,
    #[desc = "Reveal mode: `raw` (original) or `all` (harmonised + raw)."]
    pub reveal: Option<String>,
    #[desc = "Page number (0-indexed)."]
    pub page: Option<u32>,
    #[desc = "Page size (default 20)."]
    pub size: Option<u32>,
}

pub struct SummaryVariantTool {
    pub(crate) client: Arc<GwasCatalogClient>,
}

#[async_trait]
impl ToolFunction for SummaryVariantTool {
    type Input = SummaryVariantInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let page = input.page.unwrap_or(0) as usize;
        let size = input.size.unwrap_or(20) as usize;
        let reveal = input.reveal.as_deref().map(|v| match v.to_lowercase().as_str() {
            "raw" => RevealMode::Raw,
            _ => RevealMode::All,
        });

        let query = AssociationQuery {
            start: Some(page),
            size: Some(size),
            reveal,
            p_lower: input.p_lower,
            p_upper: input.p_upper,
            study_accession: input.study_accession.clone(),
        };

        let resp: PaginatedResponse<EmbeddedAssociations> = if let Some(chr) = &input.chromosome {
            self.client
                .get_variant_on_chromosome(chr, &input.variant_id, &query)
                .await
        } else {
            self.client
                .get_variant_associations(&input.variant_id, &query)
                .await
        }
        .map_err(super::json_err)?;

        Ok(AgentToolResult::success(format::format_summary_associations(
            &resp,
        )))
    }
}
