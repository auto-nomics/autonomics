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
    name = "gwascatalog_summary_associations",
    description = "Fetch per-variant summary statistics (Summary Statistics API) for a \
                  GWAS Catalog study or trait. Returns harmonised effect sizes, alleles, \
                  p-values, and confidence intervals. \
                  \
                  Use this for fine-mapping / meta-analysis inputs. \
                  \
                  Pass `study_accession` to filter by study (GCST...) or `trait` to get \
                  associations for an EFO trait. `p_lower` / `p_upper` filter by p-value range. \
                  `reveal`: `raw` for original values, `all` for harmonised + raw (hm_ prefix)."
)]
pub struct SummaryAssociationsInput {
    #[desc = "Study accession ID (e.g. `GCST005038`) to filter associations."]
    pub study_accession: Option<String>,
    #[desc = "EFO trait ID to fetch associations via `/traits/{trait}/associations`."]
    pub trait_id: Option<String>,
    #[desc = "Lower p-value threshold (e.g. `1e-5`)."]
    pub p_lower: Option<f64>,
    #[desc = "Upper p-value threshold."]
    pub p_upper: Option<f64>,
    #[desc = "Reveal mode: `raw` (original) or `all` (harmonised + raw with hm_ prefix)."]
    pub reveal: Option<String>,
    #[desc = "Page number (0-indexed). Internally converted to start offset."]
    pub page: Option<u32>,
    #[desc = "Page size (default 20)."]
    pub size: Option<u32>,
}

pub struct SummaryAssociationsTool {
    pub(crate) client: Arc<GwasCatalogClient>,
}

fn parse_reveal(s: &Option<String>) -> Option<RevealMode> {
    s.as_deref().map(|v| match v.to_lowercase().as_str() {
        "raw" => RevealMode::Raw,
        _ => RevealMode::All,
    })
}

fn page_to_start(page: Option<u32>, size: Option<u32>) -> (Option<usize>, Option<usize>) {
    let s = size.unwrap_or(20) as usize;
    let start = page.unwrap_or(0) as usize * s;
    (Some(start), Some(s))
}

#[async_trait]
impl ToolFunction for SummaryAssociationsTool {
    type Input = SummaryAssociationsInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let (start, size) = page_to_start(input.page, input.size);
        let reveal = parse_reveal(&input.reveal);

        let query = AssociationQuery {
            start,
            size,
            reveal,
            p_lower: input.p_lower,
            p_upper: input.p_upper,
            study_accession: input.study_accession.clone(),
        };

        let resp: PaginatedResponse<EmbeddedAssociations> = if let Some(trait_id) =
            &input.trait_id
        {
            self.client
                .list_trait_associations(trait_id, &query)
                .await
        } else {
            self.client.list_associations(&query).await
        }
        .map_err(super::json_err)?;

        Ok(AgentToolResult::success(format::format_summary_associations(
            &resp,
        )))
    }
}
