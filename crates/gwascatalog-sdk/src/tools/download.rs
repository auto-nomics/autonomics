//! Download full summary-statistics files from the EBI GWAS Catalog FTP
//! (HTTPS mirror).
//!
//! Unlike the Summary Statistics REST API (which returns paginated JSON),
//! this tool downloads the **complete** `.tsv` / `.tsv.gz` files that the
//! GWAS Catalog publishes per study — the same files behind the website's
//! "Download" links.
//!
//! Each study lives under:
//! ```text
//! {FTP_BASE}/{block_dir}/{accession}/
//!   ├── {accession}_buildGRCh37.tsv          (raw, unharmonised)
//!   ├── {accession}_buildGRCh38.tsv          (raw, if available)
//!   ├── harmonised/
//!   │     └── {prefix}-{accession}-{EFO}.h.tsv.gz   (harmonised)
//!   └── md5sum.txt
//! ```
//! Filenames are not fully predictable (the harmonised name embeds a
//! submission-specific prefix and the EFO code), so the tool lists the
//! directory at runtime to discover the actual filenames before streaming.

use std::sync::Arc;

use agentik_core::tools::{ProgressRecord, ToolContext, ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;
use fs::OpendalFileStorage;
use serde_json::Value;

use crate::client::GwasCatalogClient;

/// Which files to fetch.
#[derive(Debug, Clone, Default, PartialEq)]
enum FileVariant {
    /// Harmonised `.h.tsv.gz` from the `harmonised/` subdirectory (default,
    /// recommended for cross-study analysis).
    #[default]
    Harmonised,
    /// Raw per-build `.tsv` from the study root.
    Raw,
    /// Both harmonised and raw.
    All,
}

impl FileVariant {
    fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "raw" => FileVariant::Raw,
            "all" => FileVariant::All,
            _ => FileVariant::Harmonised,
        }
    }
}

#[tool(
    name = "gwascatalog_download_summary_stats",
    description = "Download full summary-statistics files for a GWAS Catalog study from the \
                  EBI FTP (HTTPS mirror). Streams the complete `.tsv` / `.tsv.gz` file(s) \
                  — not the paginated JSON API — to local storage. \
                  \
                  By default downloads the **harmonised** file (`harmonised/*.h.tsv.gz`, \
                  recommended for cross-study analysis). Use `variant=\"raw\"` for \
                  unharmonised per-build `.tsv`, or `variant=\"all\"` for both. \
                  \
                  Provide a study accession like `GCST90000061`."
)]
pub struct DownloadSummaryStatsInput {
    #[desc = "GWAS Catalog study accession, e.g. `GCST90000061`."]
    pub accession: String,
    #[desc = "Which files to fetch: `harmonised` (default), `raw`, or `all`."]
    pub variant: Option<String>,
    #[desc = "Genome assembly for raw files: `GRCh37` or `GRCh38`. If omitted, \
              prefers GRCh38 and falls back to GRCh37."]
    pub assembly: Option<String>,
    #[desc = "Destination directory under storage (default: `/{accession}`)."]
    pub dest_dir: Option<String>,
}

pub struct DownloadSummaryStatsTool {
    pub(crate) client: Arc<GwasCatalogClient>,
    pub(crate) storage: Arc<OpendalFileStorage>,
}

#[async_trait]
impl ToolFunction for DownloadSummaryStatsTool {
    type Input = DownloadSummaryStatsInput;

    // Summary stats files can be several hundred MiB; keep the sync window
    // short so the tool transitions to background quickly, letting the agent
    // poll progress via `view_task_status`.
    fn sync_seconds(&self) -> u64 {
        5
    }

    fn timeout_seconds(&self) -> u64 {
        6000
    }

    async fn execute_with_context(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let input: Self::Input = serde_json::from_value(input)?;
        let variant = input
            .variant
            .as_deref()
            .map(FileVariant::from_str)
            .unwrap_or_default();
        let dest_dir = input
            .dest_dir
            .clone()
            .unwrap_or_else(|| format!("/{}", input.accession));

        ctx.emit(
            ProgressRecord::new("status")
                .status("resolving")
                .message(format!("locating files for {}", input.accession)),
        );

        // 1. Compute study FTP directory and list its contents.
        let block_url = self
            .client
            .ftp_block_url(&input.accession)
            .map_err(super::json_err)?;
        let study_url = format!("{block_url}/{}", input.accession);

        let root_entries = self
            .client
            .list_ftp_directory(&study_url)
            .await
            .map_err(super::json_err)?;

        if root_entries.is_empty() {
            return Err(super::json_err(
                crate::error::GwasCatalogError::FtpListing {
                    url: study_url,
                    reason: format!("no files found for accession {}", input.accession),
                },
            ));
        }

        // 2. Collect the set of (filename, full_url) pairs to download.
        let mut targets: Vec<(String, String)> = Vec::new();

        if variant != FileVariant::Raw {
            // Harmonised files live under harmonised/*.h.tsv.gz.
            if root_entries.iter().any(|e| e.starts_with("harmonised")) {
                let harm_url = format!("{study_url}/harmonised");
                let harm_entries = self
                    .client
                    .list_ftp_directory(&harm_url)
                    .await
                    .map_err(super::json_err)?;
                for name in &harm_entries {
                    if name.ends_with(".h.tsv.gz") || name.ends_with(".h.tsv.gz-meta.yaml") {
                        targets.push((format!("harmonised/{name}"), format!("{harm_url}/{name}")));
                    }
                }
            }
        }

        if variant != FileVariant::Harmonised {
            // Raw files are {accession}_build{assembly}.tsv at the root.
            let want_assembly = input.assembly.as_deref();
            let mut candidates: Vec<&String> = root_entries
                .iter()
                .filter(|e| {
                    e.starts_with(&format!("{}_build", input.accession))
                        && (e.ends_with(".tsv") || e.ends_with(".tsv.gz"))
                })
                .collect();

            // Sort so that preferred assembly comes first.
            if let Some(want) = want_assembly {
                candidates.sort_by_key(|e| !e.to_ascii_uppercase().contains(want));
            } else {
                // Default: prefer GRCh38 over GRCh37.
                candidates.sort_by_key(|e| !e.to_ascii_uppercase().contains("GRCH38"));
            }

            for c in &candidates {
                targets.push((c.to_string(), format!("{study_url}/{c}")));
            }
        }

        if targets.is_empty() {
            return Ok(AgentToolResult::error(format!(
                "No downloadable summary-statistics files found at {study_url} \
                 (variant={:?}, assembly={:?})",
                variant, input.assembly
            )));
        }

        // 3. Download every target with streaming + progress.
        let total_files = targets.len();
        ctx.emit(
            ProgressRecord::new("status")
                .status("downloading")
                .message(format!(
                    "resolved {total_files} file(s) for {}",
                    input.accession
                )),
        );

        let mut downloaded = Vec::new();
        let mut errors = Vec::new();

        for (i, (relpath, url)) in targets.iter().enumerate() {
            let filename = relpath.rsplit('/').next().unwrap_or(relpath);
            let storage_path = format!("{dest_dir}/{relpath}");

            ctx.emit(
                ProgressRecord::new("progress")
                    .label(filename)
                    .current(i as u64)
                    .total(total_files as u64)
                    .message(format!("starting {filename} ({}/{total_files})", i + 1)),
            );

            const PROGRESS_INTERVAL: u64 = 1024 * 1024; // 1 MiB
            let mut last_emitted: u64 = 0;

            let result = self
                .client
                .download_stream_to_storage(url, &self.storage, &storage_path, |bytes, total| {
                    if total.is_some() && bytes - last_emitted >= PROGRESS_INTERVAL {
                        last_emitted = bytes;
                        ctx.emit(
                            ProgressRecord::new("progress")
                                .label(filename)
                                .current(bytes)
                                .total(total.unwrap_or(0)),
                        );
                    }
                })
                .await;

            match result {
                Ok(size) => {
                    ctx.emit(
                        ProgressRecord::new("finished")
                            .label(filename)
                            .status("success")
                            .message(format!(
                                "{filename}: {} ({})",
                                human_bytes(size),
                                storage_path,
                            )),
                    );
                    downloaded.push(serde_json::json!({
                        "accession": input.accession,
                        "filename": relpath,
                        "path": storage_path,
                        "size": size,
                    }));
                }
                Err(e) => {
                    let msg = e.to_string();
                    ctx.emit(
                        ProgressRecord::new("error")
                            .label(filename)
                            .message(format!("{filename}: {msg}")),
                    );
                    errors.push(serde_json::json!({
                        "accession": input.accession,
                        "filename": relpath,
                        "url": url,
                        "error": msg,
                    }));
                }
            }
        }

        ctx.emit(
            ProgressRecord::new("progress")
                .current(total_files as u64)
                .total(total_files as u64),
        );

        let summary = serde_json::json!({
            "accession": input.accession,
            "count": downloaded.len(),
            "files": downloaded,
            "errors": errors,
        });

        Ok(AgentToolResult::success(crate::format::format_download(
            &summary,
        )))
    }

    // ── Non-streaming path (direct programmatic calls / tests) ────────────

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let variant = input
            .variant
            .as_deref()
            .map(FileVariant::from_str)
            .unwrap_or_default();
        let dest_dir = input
            .dest_dir
            .clone()
            .unwrap_or_else(|| format!("/{}", input.accession));

        let block_url = self
            .client
            .ftp_block_url(&input.accession)
            .map_err(super::json_err)?;
        let study_url = format!("{block_url}/{}", input.accession);

        let root_entries = self
            .client
            .list_ftp_directory(&study_url)
            .await
            .map_err(super::json_err)?;

        let mut targets: Vec<(String, String)> = Vec::new();

        if variant != FileVariant::Raw && root_entries.iter().any(|e| e.starts_with("harmonised")) {
            let harm_url = format!("{study_url}/harmonised");
            let harm_entries = self
                .client
                .list_ftp_directory(&harm_url)
                .await
                .map_err(super::json_err)?;
            for name in &harm_entries {
                if name.ends_with(".h.tsv.gz") || name.ends_with(".h.tsv.gz-meta.yaml") {
                    targets.push((format!("harmonised/{name}"), format!("{harm_url}/{name}")));
                }
            }
        }

        if variant != FileVariant::Harmonised {
            let want_assembly = input.assembly.as_deref();
            let mut candidates: Vec<&String> = root_entries
                .iter()
                .filter(|e| {
                    e.starts_with(&format!("{}_build", input.accession))
                        && (e.ends_with(".tsv") || e.ends_with(".tsv.gz"))
                })
                .collect();
            if let Some(want) = want_assembly {
                candidates.sort_by_key(|e| !e.to_ascii_uppercase().contains(want));
            } else {
                candidates.sort_by_key(|e| !e.to_ascii_uppercase().contains("GRCH38"));
            }
            for c in &candidates {
                targets.push((c.to_string(), format!("{study_url}/{c}")));
            }
        }

        let mut downloaded = Vec::new();
        for (relpath, url) in &targets {
            let storage_path = format!("{dest_dir}/{relpath}");
            let size = self
                .client
                .download_stream_to_storage(url, &self.storage, &storage_path, |_, _| {})
                .await
                .map_err(super::json_err)?;
            downloaded.push(serde_json::json!({
                "accession": input.accession,
                "filename": relpath,
                "path": storage_path,
                "size": size,
            }));
        }

        let summary = serde_json::json!({
            "accession": input.accession,
            "count": downloaded.len(),
            "files": downloaded,
        });

        Ok(AgentToolResult::success(crate::format::format_download(
            &summary,
        )))
    }
}

/// Format a byte count into a human-readable string (e.g. "1.4 GiB").
fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    if n == 0 {
        return "0 B".to_string();
    }
    let mut f = n as f64;
    let mut unit = 0;
    while f >= 1024.0 && unit < UNITS.len() - 1 {
        f /= 1024.0;
        unit += 1;
    }
    format!("{:.1} {}", f, UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_variant_defaults_to_harmonised() {
        assert_eq!(FileVariant::default(), FileVariant::Harmonised);
    }

    #[test]
    fn file_variant_from_str() {
        assert_eq!(FileVariant::from_str("raw"), FileVariant::Raw);
        assert_eq!(FileVariant::from_str("ALL"), FileVariant::All);
        assert_eq!(FileVariant::from_str("harmonised"), FileVariant::Harmonised);
        // Unknown → default.
        assert_eq!(FileVariant::from_str("xyz"), FileVariant::Harmonised);
    }

    #[test]
    fn human_bytes_formats() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512.0 B");
        assert_eq!(human_bytes(2048), "2.0 KiB");
        assert_eq!(human_bytes(1048576), "1.0 MiB");
        assert_eq!(human_bytes(1073741824), "1.0 GiB");
    }
}
