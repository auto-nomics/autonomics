use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::OpenTargetsClient;
use crate::Pagination;
use crate::format::format_associated_diseases;

#[tool(
    name = "opentargets_associated_diseases",
    description = "Get diseases associated with a gene (target), ranked by the Open \
                  Targets overall association score. This is the core target-\
                  prioritisation endpoint: for a given Ensembl gene ID it returns the \
                  top diseases/phenotypes with evidence, including the contributing \
                  datasources and a novelty metric. Set fetch_all=true to retrieve \
                  every association (may be thousands)."
)]
pub struct AssociatedDiseasesInput {
    #[desc = "Ensembl gene ID, e.g. 'ENSG00000012048'."]
    pub ensembl_id: String,
    #[desc = "Number of results per page (default 25, max 3000). Ignored when fetch_all=true."]
    pub size: Option<u32>,
    #[desc = "0-based page index (default 0). Ignored when fetch_all=true."]
    pub index: Option<u32>,
    #[desc = "Include indirect (propagated) associations. Default false."]
    pub enable_indirect: Option<bool>,
    #[desc = "Server-side disease-name filter, e.g. 'cancer' restricts to diseases \
             whose name matches the term. Greatly reduces result count."]
    pub b_filter: Option<String>,
    #[desc = "Client-side minimum overall score in [0,1]; associations below this are dropped."]
    pub min_score: Option<f64>,
    #[desc = "Fetch ALL associations across pages (capped at min_score if given). \
             Default false — returns a single page."]
    pub fetch_all: Option<bool>,
}

pub struct AssociatedDiseasesTool {
    pub(crate) client: Arc<OpenTargetsClient>,
}

#[async_trait]
impl ToolFunction for AssociatedDiseasesTool {
    type Input = AssociatedDiseasesInput;

    fn timeout_seconds(&self) -> u64 {
        300
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let enable_indirect = input.enable_indirect.unwrap_or(false);
        let b_filter = input.b_filter.as_deref();
        let min_score = input.min_score.unwrap_or(0.0);

        if input.fetch_all.unwrap_or(false) {
            let mut rows = self
                .client
                .associated_diseases_all_filtered(ensembl_id(&input), enable_indirect, b_filter)
                .await
                .map_err(json_err)?;
            let total = rows.len();
            apply_min_score_disease(&mut rows, min_score);
            Ok(AgentToolResult::success(format_associated_diseases(
                &rows,
                total as i64,
            )))
        } else {
            let size = input.size.unwrap_or(25).min(3000);
            let index = input.index.unwrap_or(0);
            let page = self
                .client
                .associated_diseases(
                    ensembl_id(&input),
                    Pagination::new(index, size),
                    enable_indirect,
                    b_filter,
                )
                .await
                .map_err(json_err)?;
            let mut rows = page.rows;
            apply_min_score_disease(&mut rows, min_score);
            Ok(AgentToolResult::success(format_associated_diseases(
                &rows, page.count,
            )))
        }
    }
}

fn ensembl_id(input: &AssociatedDiseasesInput) -> &str {
    &input.ensembl_id
}

fn apply_min_score_disease(rows: &mut Vec<crate::AssociatedDisease>, min_score: f64) {
    if min_score > 0.0 {
        rows.retain(|a| a.score >= min_score);
    }
}
