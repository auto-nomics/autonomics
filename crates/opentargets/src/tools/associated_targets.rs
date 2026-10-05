// Tool Input structs marked `#[deprecated]` so the derived tool
// schema advertises `"deprecated": true` (survey T3 dual-track
// guidance); the module-local allow keeps the macro-generated
// impls in this file warning-free.
#![allow(deprecated)]

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::OpenTargetsClient;
use crate::Pagination;
use crate::format::format_associated_targets;

#[tool(
    name = "opentargets_associated_targets",
    description = "Pipeline/dataframe use: prefer the DAG node `source_opentargets_associated_targets` (typed table) — this tool stays for interactive lookup. Get genes (targets) associated with a disease / phenotype, ranked by \
                  the Open Targets overall association score. The disease→target mirror \
                  of opentargets_associated_diseases. Useful for finding candidate \
                  genes for a given condition."
)]
#[deprecated(note = "prefer the DAG node source_opentargets_associated_targets for pipeline use")]
pub struct AssociatedTargetsInput {
    #[desc = "Ontology ID, e.g. 'MONDO_0004975' (Alzheimer disease)."]
    pub efo_id: String,
    #[desc = "Number of results per page (default 25, max 3000). Ignored when fetch_all=true."]
    pub size: Option<u32>,
    #[desc = "0-based page index (default 0). Ignored when fetch_all=true."]
    pub index: Option<u32>,
    #[desc = "Include indirect (propagated) associations. Default false."]
    pub enable_indirect: Option<bool>,
    #[desc = "Server-side target filter (e.g. a gene symbol)."]
    pub b_filter: Option<String>,
    #[desc = "Client-side minimum overall score in [0,1]; associations below this are dropped."]
    pub min_score: Option<f64>,
    #[desc = "Fetch ALL associations across pages. Default false — returns a single page."]
    pub fetch_all: Option<bool>,
}

pub struct AssociatedTargetsTool {
    pub(crate) client: Arc<OpenTargetsClient>,
}

#[async_trait]
impl ToolFunction for AssociatedTargetsTool {
    type Input = AssociatedTargetsInput;

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
                .associated_targets_all_filtered(&input.efo_id, enable_indirect, b_filter)
                .await
                .map_err(json_err)?;
            let total = rows.len();
            apply_min_score_target(&mut rows, min_score);
            Ok(AgentToolResult::success(format_associated_targets(
                &rows,
                total as i64,
            )))
        } else {
            let size = input.size.unwrap_or(25).min(3000);
            let index = input.index.unwrap_or(0);
            let page = self
                .client
                .associated_targets(
                    &input.efo_id,
                    Pagination::new(index, size),
                    enable_indirect,
                    b_filter,
                )
                .await
                .map_err(json_err)?;
            let mut rows = page.rows;
            apply_min_score_target(&mut rows, min_score);
            Ok(AgentToolResult::success(format_associated_targets(
                &rows, page.count,
            )))
        }
    }
}

fn apply_min_score_target(rows: &mut Vec<crate::AssociatedTarget>, min_score: f64) {
    if min_score > 0.0 {
        rows.retain(|a| a.score >= min_score);
    }
}
