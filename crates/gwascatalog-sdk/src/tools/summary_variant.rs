//! Per-variant summary-statistics guidance.
//!
//! The Summary Statistics REST API this tool originally queried has been
//! deprecated by EBI (every endpoint answers `410 Gone`; see
//! <https://www.ebi.ac.uk/gwas/docs/methods/summary-statistics>), and the
//! FTP mirror publishes summary statistics **per study**, not per variant —
//! there is no variant-level lookup to forward to. The tool therefore
//! explains the migration path and, when the caller already knows a study
//! accession, forwards to the per-study FTP listing of
//! [`super::summary_associations`].

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{client::GwasCatalogClient, format, tools::summary_associations};

#[tool(
    name = "gwascatalog_summary_variant",
    description = "Deprecated entry point for per-variant summary statistics: the EBI \
                  Summary Statistics API this used to query has been turned off (410) \
                  and there is no variant-level replacement. \
                  \
                  Summary statistics now exist only as per-study files on the EBI FTP \
                  mirror. Call `gwascatalog_search` / `gwascatalog_associations` to find \
                  which studies report your variant, then use \
                  `gwascatalog_summary_associations` (list files for a `study_accession`) \
                  and `gwascatalog_download_summary_stats` (fetch them). If you already \
                  know the accession, pass it as `study_accession` here to get the file \
                  listing directly."
)]
pub struct SummaryVariantInput {
    #[desc = "Variant rsID, e.g. `rs7903146`. Used only to shape the guidance reply; \
              no variant-level summary-statistics API exists any more."]
    pub variant_id: String,
    #[desc = "Study accession ID (e.g. `GCST90000061`). When given, returns the \
              study's summary-statistics file listing from the FTP mirror."]
    pub study_accession: Option<String>,
}

pub struct SummaryVariantTool {
    pub(crate) client: Arc<GwasCatalogClient>,
}

#[async_trait]
impl ToolFunction for SummaryVariantTool {
    type Input = SummaryVariantInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        if let Some(accession) = input
            .study_accession
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
        {
            let listing =
                summary_associations::list_study_summary_files(&self.client, accession).await?;
            return Ok(AgentToolResult::success(format::format_summary_files(
                &listing,
            )));
        }

        Ok(AgentToolResult::success(format!(
            "Per-variant summary statistics are no longer available: EBI deprecated the \
             Summary Statistics API (every endpoint answers 410 Gone) and the FTP mirror \
             publishes summary statistics per study, not per variant.\n\n\
             To get summary statistics for `{}`:\n\
             1. `gwascatalog_associations` or `gwascatalog_search` — find which studies \
             report the variant (study accessions).\n\
             2. `gwascatalog_summary_associations` with each `study_accession` — list \
             the published summary-statistics files.\n\
             3. `gwascatalog_download_summary_stats` — fetch the harmonised `.h.tsv.gz`.",
            input.variant_id
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn tool() -> SummaryVariantTool {
        SummaryVariantTool {
            client: Arc::new(GwasCatalogClient::new()),
        }
    }

    #[tokio::test]
    async fn variant_without_study_returns_migration_path() {
        let input: SummaryVariantInput =
            serde_json::from_value(serde_json::json!({ "variant_id": "rs7903146" })).unwrap();
        let result = tool().run(input).await.unwrap();
        assert_eq!(result.is_error, None);
        let text = result.text_content();
        assert!(text.contains("rs7903146"), "got: {text}");
        assert!(text.contains("410"), "got: {text}");
        assert!(
            text.contains("gwascatalog_download_summary_stats"),
            "got: {text}"
        );
    }

    #[tokio::test]
    async fn invalid_accession_fails_before_any_network_call() {
        let input: SummaryVariantInput = serde_json::from_value(serde_json::json!({
            "variant_id": "rs7903146",
            "study_accession": "not-an-accession",
        }))
        .unwrap();
        let result = tool().run(input).await;
        assert!(result.is_err());
    }
}
