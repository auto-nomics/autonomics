use std::sync::Arc;

use super::json_err;
use crate::format::format_download;
use crate::{OpengwasClient, types::GwasInfoFilesRequest};
use agentik_core::tools::{ProgressRecord, ToolContext, ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;
use fs::OpendalFileStorage;
use serde_json::Value;

#[tool(
    name = "opengwas_download_files",
    description = "Download dataset files (summary stats .vcf.gz, index .vcf.gz.tbi, \
                  QC report _report.html) to local storage. Provide GWAS dataset \
                  IDs and the files will be streamed to the configured storage."
)]
pub struct DownloadFilesInput {
    #[desc = "List of GWAS study IDs to download files for, e.g. ['ieu-a-2', 'ukb-b-19953']."]
    pub id: Vec<String>,
}

/// Flatten the nested JSON response into `(study_id, filename, url)` triples.
///
/// The API returns `{ study_id: ["url1", "url2", ...] }` where each URL ends
/// with the filename (e.g. `.../ieu-a-2.vcf.gz`).
fn parse_file_entries(resp: &Value) -> Vec<(String, String, String)> {
    resp.as_object()
        .into_iter()
        .flatten()
        .flat_map(|(study_id, files)| {
            files
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(move |url_val| {
                    let url = url_val.as_str()?;
                    let filename = url.rsplit('/').next()?;
                    Some((study_id.clone(), filename.to_string(), url.to_string()))
                })
        })
        .collect()
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

pub struct DownloadFilesTool {
    client: Arc<OpengwasClient>,
    storage: Arc<OpendalFileStorage>,
}

impl DownloadFilesTool {
    pub fn new(client: Arc<OpengwasClient>, storage: Arc<OpendalFileStorage>) -> Self {
        Self { client, storage }
    }
}

#[async_trait]
impl ToolFunction for DownloadFilesTool {
    type Input = DownloadFilesInput;

    // Downloads are I/O-bound and typically exceed 30s for .vcf.gz files
    // (often several GiB). Keep the sync window short so the tool transitions
    // to background quickly, letting the agent poll progress via
    // `view_task_status` while the download streams.
    fn sync_seconds(&self) -> u64 {
        5
    }

    fn timeout_seconds(&self) -> u64 {
        6000
    }

    // ── Streaming path (used by the toolset) ──────────────────────────────
    //
    // Emits structured `ProgressRecord`s for every file and at regular byte
    // intervals during streaming, so `view_task_status` can report real-time
    // download progress while the tool runs in the background.

    async fn execute_with_context(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        // Deserialize manually — overriding execute_with_context bypasses
        // the default `execute → run` chain.
        let input: Self::Input = serde_json::from_value(input)?;

        // 1. Resolve download URLs from the API.
        let resp = self
            .client
            .gwasinfo_files(&GwasInfoFilesRequest {
                id: input.id.clone(),
                commercial_approval_received: None,
            })
            .await
            .map_err(json_err)?;

        let entries = parse_file_entries(&resp);
        let total_files = entries.len();

        ctx.emit(
            ProgressRecord::new("status")
                .status("downloading")
                .message(format!("resolved {total_files} file(s) for {} study/studies", input.id.len())),
        );

        // 2. Stream-download each file, emitting progress along the way.
        let mut downloaded = Vec::new();
        let mut errors = Vec::new();

        for (i, (study_id, filename, url)) in entries.iter().enumerate() {
            let storage_path = format!("/{study_id}/{filename}");

            ctx.emit(
                ProgressRecord::new("progress")
                    .label(filename)
                    .current(i as u64)
                    .total(total_files as u64)
                    .message(format!("starting {filename} ({}/{total_files})", i + 1)),
            );

            // Emit byte-level progress every this many bytes, so we don't
            // flood the ProgressBuffer for tiny chunks.
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
                            ))
                            .elapsed_ms(0),
                    );
                    downloaded.push(serde_json::json!({
                        "study_id": study_id,
                        "filename": filename,
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
                        "study_id": study_id,
                        "filename": filename,
                        "url": url,
                        "error": msg,
                    }));
                }
            }
        }

        // Final summary — update the overall progress to 100%.
        ctx.emit(
            ProgressRecord::new("progress")
                .current(total_files as u64)
                .total(total_files as u64),
        );

        let summary = serde_json::json!({
            "count": downloaded.len(),
            "files": downloaded,
            "errors": errors,
        });

        Ok(AgentToolResult::success(format_download(&summary)))
    }

    // ── Non-streaming path (direct programmatic calls / tests) ────────────
    //
    // This path is NOT reachable through the toolset (which calls
    // `execute_with_context`), but preserves the original simple API for
    // callers that invoke the tool directly.

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let resp = self
            .client
            .gwasinfo_files(&GwasInfoFilesRequest {
                id: input.id.clone(),
                commercial_approval_received: None,
            })
            .await
            .map_err(json_err)?;

        let mut downloaded = Vec::new();

        for (study_id, filename, url) in parse_file_entries(&resp) {
            let storage_path = format!("/{study_id}/{filename}");
            let size = self
                .client
                .download_file_to_storage(&url, &self.storage, &storage_path)
                .await
                .map_err(json_err)?;
            downloaded.push(serde_json::json!({
                "study_id": study_id,
                "filename": filename,
                "path": storage_path,
                "size": size,
            }));
        }

        Ok(AgentToolResult::success(format_download(
            &serde_json::json!({
                "count": downloaded.len(),
                "files": downloaded,
            }),
        )))
    }
}
