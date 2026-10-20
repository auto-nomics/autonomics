use std::sync::Arc;

use agentik_core::tools::{ExecutionMode, ProgressRecord, ToolContext, ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use serde_json::Value;

use crate::manager::{DownloadManager, DownloadRequest, DownloadSource};

#[tool(
    name = "resource_download",
    description = "Download an allowlisted HTTP(S) URL or host-managed S3/OSS object into the agent VFS. \
                  The transfer streams in the background, reports byte progress, verifies \
                  SHA-256 when provided, and installs the file atomically under \
                  /downloads. Poll view_task_status for progress and use wait_task or \
                  view_task_results to collect the receipt."
)]
pub struct ResourceDownloadInput {
    #[desc = "Absolute HTTP(S) URL to download. HTTPS is preferred; HTTP is policy-gated. Credentials in the URL are rejected. Omit when using profile/bucket/key."]
    pub uri: Option<String>,
    #[desc = "Host-managed object-store profile ID. Required for S3/OSS sources; omit for HTTP(S)."]
    pub profile: Option<String>,
    #[desc = "Authorized object-store bucket; required with profile."]
    pub bucket: Option<String>,
    #[desc = "Object key beginning with /; required with profile and restricted by the profile prefixes."]
    pub key: Option<String>,
    #[desc = "Destination VFS path; must start with /downloads/."]
    pub destination: String,
    #[desc = "Overwrite an existing destination file. Default false."]
    #[default = false]
    pub overwrite: Option<bool>,
    #[desc = "Optional expected SHA-256 digest (64 hexadecimal characters)."]
    pub expected_sha256: Option<String>,
}

pub struct ResourceDownloadTool {
    manager: Arc<DownloadManager>,
}

impl ResourceDownloadTool {
    pub fn new(manager: Arc<DownloadManager>) -> Self {
        Self { manager }
    }
}

fn source_from_input(input: &ResourceDownloadInput) -> Result<DownloadSource, ToolError> {
    match (&input.uri, &input.profile) {
        (Some(uri), None) if !uri.trim().is_empty() => Ok(DownloadSource::Uri { uri: uri.clone() }),
        (None, Some(profile)) => {
            let bucket = input
                .bucket
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| ToolError::ValidationFailed {
                    message: "bucket is required for object-store downloads".to_string(),
                })?;
            let key = input
                .key
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| ToolError::ValidationFailed {
                    message: "key is required for object-store downloads".to_string(),
                })?;
            Ok(DownloadSource::ObjectStore {
                profile: profile.trim().to_string(),
                bucket: bucket.to_string(),
                key: key.to_string(),
            })
        }
        (Some(_), Some(_)) => Err(ToolError::ValidationFailed {
            message: "provide either uri or profile/bucket/key, not both".to_string(),
        }),
        _ => Err(ToolError::ValidationFailed {
            message: "provide uri or profile/bucket/key".to_string(),
        }),
    }
}

#[async_trait]
impl ToolFunction for ResourceDownloadTool {
    type Input = ResourceDownloadInput;

    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::Async
    }

    fn timeout_seconds(&self) -> u64 {
        24 * 60 * 60
    }

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let source = source_from_input(&input)?;
        let receipt = self
            .manager
            .download(DownloadRequest {
                source,
                destination: input.destination,
                overwrite: input.overwrite.unwrap_or(false),
                expected_sha256: input.expected_sha256,
            })
            .await?;
        Ok(ToolResult::success(
            serde_json::to_string(&receipt).unwrap_or_else(|_| "{}".to_string()),
        ))
    }

    async fn execute_with_context(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let input: Self::Input = serde_json::from_value(input)?;
        let source = source_from_input(&input)?;
        let source_label = match &source {
            DownloadSource::Uri { uri } => uri.clone(),
            DownloadSource::ObjectStore {
                profile,
                bucket,
                key,
            } => format!("{profile}:{bucket}/{key}"),
        };
        ctx.emit(
            ProgressRecord::new("status")
                .status("downloading")
                .message(format!("requesting {source_label}"))
                .label(input.destination.clone()),
        );
        let result = self
            .manager
            .download_with_progress(
                DownloadRequest {
                    source,
                    destination: input.destination,
                    overwrite: input.overwrite.unwrap_or(false),
                    expected_sha256: input.expected_sha256,
                },
                |bytes, total| {
                    ctx.emit(
                        ProgressRecord::new("progress")
                            .status("downloading")
                            .current(bytes)
                            .total(total.unwrap_or(0)),
                    );
                },
            )
            .await;
        match result {
            Ok(receipt) => {
                ctx.emit(
                    ProgressRecord::new("finished")
                        .status("success")
                        .current(receipt.bytes)
                        .total(receipt.bytes)
                        .message(format!("{} → {}", receipt.source, receipt.destination)),
                );
                Ok(ToolResult::success(
                    serde_json::to_string(&receipt).unwrap_or_else(|_| "{}".to_string()),
                ))
            }
            Err(error) => {
                ctx.emit(
                    ProgressRecord::new("error")
                        .status("failed")
                        .message(error.to_string()),
                );
                Err(error.into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DownloaderConfig;
    use crate::policy::DownloadPolicy;
    use vfs::OpendalFileStorage;

    fn manager() -> Arc<DownloadManager> {
        let mut config = DownloaderConfig::default();
        config.policy = DownloadPolicy::allow_https("example.com", "/");
        Arc::new(
            DownloadManager::new(
                config,
                Arc::new(OpendalFileStorage::new(tempfile::tempdir().unwrap().keep())),
            )
            .unwrap(),
        )
    }

    #[test]
    fn definition_names_resource_download_and_is_async() {
        let tool = ResourceDownloadTool::new(manager());
        assert!(
            tool.definition()
                .input_schema
                .properties
                .contains_key("profile")
        );
        assert_eq!(tool.definition().name, "resource_download");
        assert_eq!(tool.execution_mode(), ExecutionMode::Async);
    }

    #[test]
    fn source_input_accepts_uri_or_object_store_but_not_both() {
        let uri = serde_json::from_value::<ResourceDownloadInput>(serde_json::json!({
            "uri": "https://example.com/file",
            "destination": "/downloads/file"
        }))
        .unwrap();
        assert!(matches!(
            source_from_input(&uri).unwrap(),
            DownloadSource::Uri { .. }
        ));

        let object = serde_json::from_value::<ResourceDownloadInput>(serde_json::json!({
            "profile": "public",
            "bucket": "open-data",
            "key": "/reference/file",
            "destination": "/downloads/file"
        }))
        .unwrap();
        assert!(matches!(
            source_from_input(&object).unwrap(),
            DownloadSource::ObjectStore { .. }
        ));

        let both = serde_json::from_value::<ResourceDownloadInput>(serde_json::json!({
            "uri": "https://example.com/file",
            "profile": "public",
            "destination": "/downloads/file"
        }))
        .unwrap();
        assert!(source_from_input(&both).is_err());
    }
}
