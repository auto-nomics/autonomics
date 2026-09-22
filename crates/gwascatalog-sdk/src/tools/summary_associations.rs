//! Study-level summary-statistics file discovery over the EBI FTP (HTTPS
//! mirror).
//!
//! The Summary Statistics REST API this tool originally queried has been
//! deprecated by EBI — every endpoint answers `410 Gone` and points at
//! <https://www.ebi.ac.uk/gwas/docs/methods/summary-statistics>, which names
//! the FTP mirror as the sanctioned access path (no replacement API yet).
//! The tool therefore lists the study's published `.tsv` / `.tsv.gz` files
//! — the same discovery step [`super::download::DownloadSummaryStatsTool`]
//! performs before streaming — so the agent can see what exists before
//! committing to a download.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{client::GwasCatalogClient, format};

#[tool(
    name = "gwascatalog_summary_associations",
    description = "List the full summary-statistics files published for a GWAS Catalog \
                  study on the EBI FTP mirror (the Summary Statistics API that used to \
                  serve per-variant rows has been deprecated by EBI and answers 410). \
                  \
                  Returns each file's name and direct URL — raw per-build `.tsv` and \
                  harmonised `harmonised/*.h.tsv.gz` — plus the download tool to fetch \
                  them. Pass `study_accession` (e.g. `GCST005038`). For a trait, first \
                  resolve studies with `gwascatalog_search` / `gwascatalog_studies`, \
                  then call this tool per accession."
)]
pub struct SummaryAssociationsInput {
    #[desc = "Study accession ID (e.g. `GCST005038`) whose summary-statistics files to list."]
    pub study_accession: Option<String>,
    #[desc = "EFO trait ID. No longer resolvable directly (API deprecated); the reply \
              explains how to get from a trait to per-study files."]
    pub trait_id: Option<String>,
}

pub struct SummaryAssociationsTool {
    pub(crate) client: Arc<GwasCatalogClient>,
}

/// List the summary-statistics files of one study on the FTP mirror as a
/// structured JSON value (`{ accession, study_url, files: [...] }`).
///
/// Shared with [`super::summary_variant`], which forwards a `study_accession`
/// here.
pub(crate) async fn list_study_summary_files(
    client: &GwasCatalogClient,
    accession: &str,
) -> Result<serde_json::Value, ToolError> {
    let block_url = client.ftp_block_url(accession).map_err(super::json_err)?;
    let study_url = format!("{block_url}/{accession}");

    let root_entries = client
        .list_ftp_directory(&study_url)
        .await
        .map_err(super::json_err)?;

    if root_entries.is_empty() {
        return Err(super::json_err(
            crate::error::GwasCatalogError::FtpListing {
                url: study_url,
                reason: format!("no summary-statistics files found for {accession}"),
            },
        ));
    }

    let mut files: Vec<serde_json::Value> = Vec::new();
    for name in &root_entries {
        if name.starts_with(&format!("{accession}_build"))
            && (name.ends_with(".tsv") || name.ends_with(".tsv.gz"))
        {
            files.push(serde_json::json!({
                "name": name,
                "url": format!("{study_url}/{name}"),
                "variant": "raw",
            }));
        }
    }

    if root_entries.iter().any(|e| e.starts_with("harmonised")) {
        let harm_url = format!("{study_url}/harmonised");
        if let Ok(harm_entries) = client.list_ftp_directory(&harm_url).await {
            for name in &harm_entries {
                if name.ends_with(".h.tsv.gz") {
                    files.push(serde_json::json!({
                        "name": format!("harmonised/{name}"),
                        "url": format!("{harm_url}/{name}"),
                        "variant": "harmonised",
                    }));
                }
            }
        }
    }

    Ok(serde_json::json!({
        "accession": accession,
        "study_url": study_url,
        "count": files.len(),
        "files": files,
    }))
}

#[async_trait]
impl ToolFunction for SummaryAssociationsTool {
    type Input = SummaryAssociationsInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let Some(accession) = input
            .study_accession
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
        else {
            // Guidance-only replies are successes (the migration path IS the
            // answer); only a call with no usable input is an error.
            return Ok(match input.trait_id.as_deref().map(str::trim) {
                Some(trait_id) if !trait_id.is_empty() => AgentToolResult::success(format!(
                    "The Summary Statistics API has been deprecated by EBI (410 Gone), so \
                     trait `{trait_id}` cannot be resolved to association rows directly. \
                     Per-study summary-statistics files are served from the FTP mirror: \
                     resolve the trait to studies with `gwascatalog_search` or \
                     `gwascatalog_studies`, then call `gwascatalog_summary_associations` \
                     with each `study_accession`, and `gwascatalog_download_summary_stats` \
                     to fetch the files."
                )),
                _ => AgentToolResult::error(
                    "Provide `study_accession` (e.g. `GCST005038`) to list a study's \
                     summary-statistics files."
                        .to_string(),
                ),
            });
        };

        let listing = list_study_summary_files(&self.client, accession).await?;
        Ok(AgentToolResult::success(format::format_summary_files(
            &listing,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn tool() -> SummaryAssociationsTool {
        SummaryAssociationsTool {
            client: Arc::new(GwasCatalogClient::new()),
        }
    }

    fn input_json(value: serde_json::Value) -> SummaryAssociationsInput {
        serde_json::from_value(value).unwrap()
    }

    #[tokio::test]
    async fn without_inputs_asks_for_an_accession() {
        let result = tool().run(input_json(serde_json::json!({}))).await.unwrap();
        assert_eq!(result.is_error, Some(true));
        let text = result.text_content();
        assert!(text.contains("study_accession"), "got: {text}");
    }

    #[tokio::test]
    async fn trait_only_returns_migration_guidance() {
        let result = tool()
            .run(input_json(serde_json::json!({ "trait_id": "EFO_0000400" })))
            .await
            .unwrap();
        assert_eq!(result.is_error, None);
        let text = result.text_content();
        assert!(text.contains("410"), "got: {text}");
        assert!(
            text.contains("gwascatalog_download_summary_stats"),
            "got: {text}"
        );
    }

    #[tokio::test]
    async fn invalid_accession_fails_before_any_network_call() {
        let result = tool()
            .run(input_json(
                serde_json::json!({ "study_accession": "not-an-accession" }),
            ))
            .await;
        assert!(result.is_err());
    }

    /// Live FTP round-trip: GCST90002395 publishes both raw and harmonised
    /// files. Run with `cargo test -p gwascatalog-sdk -- --ignored`.
    #[tokio::test]
    #[ignore = "requires network access to the EBI FTP mirror"]
    async fn lists_files_for_a_published_study() {
        let listing = list_study_summary_files(&GwasCatalogClient::new(), "GCST90002395")
            .await
            .unwrap();
        assert!(listing["count"].as_u64().unwrap_or(0) > 0, "{listing}");
    }
}
