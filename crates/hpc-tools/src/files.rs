//! Bounded remote account filesystem requests. Large transfers use the same
//! chunk protocol through the VFS transfer tool, not agent-context file bodies.

use crate::HpcClient;
use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub const REMOTE_FS: &str = include_str!("remote_fs.py");

#[tool(
    name = "hpc_files",
    description = "Access files permitted to your authenticated HPC account through the configured data node. Actions: info, list, stat, read, search, mkdir, write, move, copy, remove, upload_begin, upload_status, upload_chunk, upload_commit, upload_abort. No shell execution. Omit unused optional fields or use JSON null, never the string \"null\". read returns a bounded UTF-8 or base64 range; use hpc_transfer for large local/VFS transfers. File writes default to atomic no-clobber; replacement requires the current SHA-256. Directory copy/move defaults to strict kernel no-replace and may be unsupported by a filesystem. Explicit directory_publish_mode=cooperative_reservation enables a private lock and exclusive empty destination reservation; this provides no compare-and-swap guarantee against noncooperating processes in the same Unix account. Failed reservations are retained for inspection, never automatically deleted. Upload chunks are resumable and final commit verifies size and SHA-256. For upload_status/chunk/commit/abort, reuse the exact full path and upload_id returned by upload_begin; a mismatched path does not prove staging loss. Symlinks are listed as links and destructive operations do not follow their final component. Scientific restricted-data authorization still applies."
)]
pub struct HpcFilesInput {
    /// Operation listed in the tool description.
    #[desc = "Operation listed in the tool description. Omit unused optional fields or use JSON null (without quotes); never send the string null or empty placeholder strings. All upload actions require the original destination path returned by upload_begin as well as upload_id."]
    pub action: String,
    /// Remote absolute path or path relative to the account home. Omit for info.
    /// All upload actions require the same destination path returned by upload_begin.
    pub path: Option<String>,
    /// Destination path for copy/move.
    pub destination: Option<String>,
    /// Text content for write, or base64 bytes when encoding=base64.
    pub content: Option<String>,
    /// utf8 (default) or base64.
    pub encoding: Option<String>,
    /// Byte offset for read, or expected sequential upload offset.
    pub offset: Option<u64>,
    /// Read byte count (maximum 1 MiB), or listing/search result count (maximum 500).
    pub limit: Option<usize>,
    /// Listing pagination offset. Search instead uses cursor.
    pub page: Option<usize>,
    /// Literal substring matched against descendant paths by search.
    pub query: Option<String>,
    /// Search continuation cursor returned by the previous search.
    pub cursor: Option<String>,
    /// Permit bounded recursive search/removal or parent directory creation.
    pub recursive: Option<bool>,
    /// Directory copy/move only: strict (default) or explicit cooperative_reservation.
    pub directory_publish_mode: Option<String>,
    /// Expected existing destination SHA-256; required to replace an existing file.
    /// For new files omit this field or use JSON null, never the string "null".
    pub expected_sha256: Option<String>,
    /// SHA-256 of uploaded chunk or full committed upload.
    pub sha256: Option<String>,
    /// Expected total upload size at commit.
    pub size: Option<u64>,
    /// Upload handle returned by upload_begin. upload_status returns its validated resume offset.
    pub upload_id: Option<String>,
}

pub struct HpcFilesTool {
    pub client: Arc<HpcClient>,
}

pub async fn request(client: &HpcClient, input: &Value) -> Result<Value, String> {
    validate_request(input).map_err(|e| e.to_string())?;
    let mut result = client.run_json(REMOTE_FS, input, true).await?;
    if let Some(object) = result.as_object_mut() {
        object.insert(
            "endpoint".into(),
            Value::String(client.config().data_ssh_host.clone().unwrap_or_default()),
        );
    }
    Ok(result)
}

fn validate_request(input: &Value) -> Result<(), ToolError> {
    crate::reject_placeholder_strings(
        input,
        &[
            "path",
            "destination",
            "encoding",
            "cursor",
            "directory_publish_mode",
            "expected_sha256",
            "sha256",
            "upload_id",
        ],
    )?;
    if input
        .get("action")
        .and_then(Value::as_str)
        .is_some_and(|a| a.starts_with("upload_"))
        && input.get("path").and_then(Value::as_str).is_none()
    {
        return Err(ToolError::ValidationFailed {
            message: "upload actions require the original destination path returned by upload_begin; upload_id alone is insufficient".into(),
        });
    }
    for field in ["expected_sha256", "sha256"] {
        if let Some(value) = input.get(field).filter(|v| !v.is_null()) {
            if value.as_str().is_none_or(|s| {
                s.len() != 64
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }) {
                return Err(ToolError::ValidationFailed {
                    message: format!(
                        "{field} must be 64 lowercase hexadecimal characters; omit it or use JSON null when absent"
                    ),
                });
            }
        }
    }
    Ok(())
}

#[async_trait]
impl ToolFunction for HpcFilesTool {
    type Input = HpcFilesInput;
    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let input = serde_json::to_value(input)?;
        if let Err(error) = validate_request(&input) {
            return Ok(ToolResult::error(error.to_string()));
        }
        match request(&self.client, &input).await {
            Ok(value) if value.get("ok").and_then(Value::as_bool) == Some(false) => {
                Ok(ToolResult::error(value.to_string()))
            }
            Ok(value) => Ok(ToolResult::success_json(value)),
            Err(error) => Ok(ToolResult::error(error)),
        }
    }
}

pub fn registrations(client: Arc<HpcClient>) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(HpcFilesTool { client })]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_schema_accepts_all_null_options_without_placeholder_strings() {
        use agentik_sdk::types::ToolInput;
        let definition = HpcFilesInput::definition();
        let minimal = serde_json::json!({"action":"mkdir", "path":"/toy"});
        definition.validate_input(&minimal).unwrap();
        let expanded =
            serde_json::to_value(serde_json::from_value::<HpcFilesInput>(minimal).unwrap())
                .unwrap();
        definition.validate_input(&expanded).unwrap();
        validate_request(&expanded).unwrap();
        for field in ["destination", "cursor", "expected_sha256", "upload_id"] {
            assert!(expanded[field].is_null());
            let mut malformed = expanded.clone();
            malformed[field] = serde_json::json!("null");
            assert!(validate_request(&malformed).is_err());
        }
    }

    #[tokio::test]
    async fn malformed_controls_are_rejected_before_any_transport() {
        let tool = HpcFilesTool {
            client: Arc::new(HpcClient::definition_only()),
        };
        for (field, value) in [
            ("expected_sha256", "null"),
            ("expected_sha256", "bad-hash"),
            ("path", "null"),
        ] {
            let mut input = serde_json::json!({"action":"write", "path":"/toy", "content":"null"});
            input[field] = serde_json::json!(value);
            let result = tool
                .run(serde_json::from_value(input).unwrap())
                .await
                .unwrap();
            assert_eq!(result.is_error, Some(true));
            assert!(result.text_content().contains(field));
        }
        // File contents and actual paths containing the word remain literal.
        assert!(
            validate_request(
                &serde_json::json!({"path":"/toy/null", "content":"null", "expected_sha256":null})
            )
            .is_ok()
        );
    }
}
