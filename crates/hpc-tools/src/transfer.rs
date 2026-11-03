//! Bounded-memory account transfers through the agent's principal-scoped VFS.
//! File contents never pass through the model transcript or arbitrary host paths.
use crate::{HpcClient, files};
use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use futures::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use vfs::OpendalFileStorage;

const CHUNK: usize = 1024 * 1024;

#[tool(
    name = "hpc_transfer",
    description = "Stream a file between your authorized VFS mount and your HPC account through its data-transfer node. direction=upload or download, explicit remote_path and vfs_path. Bounded-memory chunks stay outside the model context; returns measured SHA256 and size. Upload uses durable remote staging and atomic verified commit; interrupted uploads remain recoverable through hpc_files. Download verifies every chunk, source identity throughout and optional expected_sha256, then atomically publishes the VFS file. Set replace=true explicitly to replace a local VFS file; replacing a remote target additionally requires its expected existing SHA256. No arbitrary local host filesystem access and no compute/job management on the data node. Use explicit /dssg or /lustre paths; remote home can differ from login home."
)]
pub struct HpcTransferInput {
    pub direction: String,
    pub remote_path: String,
    pub vfs_path: String,
    /// Expected transferred content SHA256, if supplied.
    pub expected_sha256: Option<String>,
    /// Existing remote target SHA256, required for remote replacement.
    pub existing_remote_sha256: Option<String>,
    /// Explicitly permit replacement of an existing VFS destination.
    pub replace: Option<bool>,
    /// Resume a durable remote upload returned by a prior interrupted call.
    pub upload_id: Option<String>,
}

pub struct HpcTransferTool {
    pub client: Arc<HpcClient>,
    pub storage: Arc<OpendalFileStorage>,
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

async fn request(client: &HpcClient, value: Value) -> Result<Value, String> {
    let response = files::request(client, &value).await?;
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(format!("remote transfer operation refused: {response}"));
    }
    Ok(response)
}

impl HpcTransferTool {
    async fn upload(&self, input: &HpcTransferInput) -> Result<Value, String> {
        self.storage
            .check_readable(&input.vfs_path)
            .map_err(|e| e.to_string())?;
        let size = self
            .storage
            .content_length(&input.vfs_path)
            .await
            .map_err(|e| e.to_string())?;
        // This stream retains the VFS object's read lock until the upload finishes.
        let source = self
            .storage
            .read_stream(&input.vfs_path, 0..size)
            .await
            .map_err(|e| e.to_string())?;
        let begin = if let Some(id) = &input.upload_id {
            request(
                &self.client,
                json!({"action":"upload_status","path":input.remote_path,"upload_id":id}),
            )
            .await?
        } else {
            request(&self.client, json!({"action":"upload_begin","path":input.remote_path,"expected_sha256":input.existing_remote_sha256})).await?
        };
        let id = begin
            .get("upload_id")
            .and_then(Value::as_str)
            .ok_or("missing upload identity")?
            .to_string();
        let destination = begin
            .get("path")
            .and_then(Value::as_str)
            .ok_or("missing resolved upload path")?
            .to_string();
        if input
            .upload_id
            .as_ref()
            .is_some_and(|expected| expected != &id)
        {
            return Err("resumed upload identity differs".into());
        }
        let resume = begin
            .get("offset")
            .and_then(Value::as_u64)
            .ok_or("missing upload offset")?;
        if resume > size {
            return Err("remote staging is larger than the VFS source".into());
        }
        let result = async {
            let mut source = Box::pin(source);
            let mut offset = 0_u64;
            let mut hasher = Sha256::new();
            while let Some(buffer) = source.next().await {
                let buffer = buffer.map_err(|e| e.to_string())?;
                for chunk in buffer.chunks(CHUNK) {
                    let end = offset.checked_add(chunk.len() as u64).ok_or("transfer size overflow")?;
                    if end > size { return Err("source returned bytes outside its declared range".into()); }
                    if end > resume {
                        let skip = resume.saturating_sub(offset) as usize;
                        let pending = &chunk[skip..];
                        let response = request(&self.client, json!({"action":"upload_chunk","path":input.remote_path,"upload_id":id,"offset":offset.max(resume),"encoding":"base64","content":STANDARD.encode(pending),"sha256":hash(pending)})).await?;
                        if response.get("offset").and_then(Value::as_u64) != Some(end)
                            || response.get("chunk_sha256").and_then(Value::as_str) != Some(hash(pending).as_str())
                            || response.get("upload_id").and_then(Value::as_str) != Some(id.as_str())
                            || response.get("path").and_then(Value::as_str) != Some(destination.as_str()) {
                            return Err("remote upload chunk identity differs".into());
                        }
                    }
                    hasher.update(chunk); offset = end;
                }
            }
            if offset != size { return Err("VFS source truncated during transfer".into()); }
            let sha = format!("{:x}", hasher.finalize());
            if input.expected_sha256.as_ref().is_some_and(|expected| expected != &sha) {
                return Err("transferred content differs from expected SHA256".into());
            }
            let committed = request(&self.client, json!({"action":"upload_commit","path":input.remote_path,"upload_id":id,"size":size,"sha256":sha})).await?;
            if committed.get("sha256").and_then(Value::as_str) != Some(sha.as_str())
                || committed.get("size").and_then(Value::as_u64) != Some(size)
                || committed.get("upload_id").and_then(Value::as_str) != Some(id.as_str())
                || committed.get("path").and_then(Value::as_str) != Some(destination.as_str()) {
                return Err("remote commit content identity differs".into());
            }
            Ok(json!({"direction":"upload","vfs_path":input.vfs_path,"remote_path":input.remote_path,"bytes":size,"sha256":sha,"upload_id":id,"committed":committed.get("committed").and_then(Value::as_bool).unwrap_or(false),"current_target_bytes_verified":committed.get("current_target_bytes_verified").and_then(Value::as_bool).unwrap_or(false),"historical_commit_verified":committed.get("historical_commit_verified").and_then(Value::as_bool).unwrap_or(false),"receipt_authenticity_verified":false,"data_endpoint":self.client.config().data_ssh_host}))
        }.await;
        // Unknown network outcomes retain named staging state; never erase a
        // potentially completed upload or infer a missing successful commit.
        result.map_err(|e: String| format!("{e}; inspect remote upload_id={id} before resuming"))
    }

    async fn download(&self, input: &HpcTransferInput) -> Result<Value, String> {
        self.storage
            .check_writable(&input.vfs_path)
            .map_err(|e| e.to_string())?;
        let operator = self.storage.resolve(&input.vfs_path);
        let key = self.storage.resolve_path(&input.vfs_path);
        // Acquire the same destination lock used by VFS writers before the
        // no-clobber check, so another cooperating VFS writer cannot win it.
        let mut writer = self
            .storage
            .atomic_writer(&input.vfs_path)
            .await
            .map_err(|e| e.to_string())?;
        if !input.replace.unwrap_or(false) {
            match operator.stat(&key).await {
                Ok(_) => {
                    let _ = writer.abort().await;
                    return Err("VFS destination exists; replace=true is required".into());
                }
                Err(e) if e.kind() == vfs::opendal::ErrorKind::NotFound => {}
                Err(e) => {
                    let _ = writer.abort().await;
                    return Err(e.to_string());
                }
            }
        }
        let result = async {
            let mut offset = 0_u64;
            let mut identity: Option<Value> = None;
            let mut size = None;
            let mut hasher = Sha256::new();
            loop {
                let value = request(&self.client, json!({"action":"read","path":input.remote_path,"offset":offset,"limit":CHUNK,"encoding":"base64"})).await?;
                let current = value.get("identity").filter(|v| v.is_array()).ok_or("missing remote file identity")?;
                if let Some(expected) = &identity {
                    if current != expected { return Err("remote source changed during transfer".into()); }
                } else { identity = Some(current.clone()); }
                let observed_size = value.get("size").and_then(Value::as_u64).ok_or("missing remote source size")?;
                if size.is_some_and(|expected| expected != observed_size) { return Err("remote size changed".into()); }
                size = Some(observed_size);
                let bytes = STANDARD.decode(value.get("content").and_then(Value::as_str).ok_or("missing remote file bytes")?).map_err(|_| "invalid remote base64")?;
                if bytes.len() > CHUNK || value.get("bytes").and_then(Value::as_u64) != Some(bytes.len() as u64)
                    || value.get("chunk_sha256").and_then(Value::as_str) != Some(hash(&bytes).as_str()) {
                    return Err("remote chunk size or SHA256 differs".into());
                }
                let end = offset.checked_add(bytes.len() as u64).ok_or("transfer size overflow")?;
                if end > observed_size || (bytes.is_empty() && offset != observed_size) { return Err("remote source truncated".into()); }
                writer.write(bytes.clone()).await.map_err(|e| e.to_string())?;
                hasher.update(&bytes); offset = end;
                if offset == observed_size { break; }
            }
            // Final zero-length observation rechecks source identity after the
            // final chunk without placing the entire file in an agent message.
            let final_value = request(&self.client, json!({"action":"read","path":input.remote_path,"offset":offset,"limit":0,"encoding":"base64"})).await?;
            if final_value.get("identity") != identity.as_ref()
                || final_value.get("size").and_then(Value::as_u64) != Some(offset)
                || final_value.get("bytes").and_then(Value::as_u64) != Some(0)
                || final_value.get("content").and_then(Value::as_str) != Some("")
                || final_value.get("next_offset").and_then(Value::as_u64) != Some(offset)
                || final_value.get("chunk_sha256").and_then(Value::as_str) != Some(hash(&[]).as_str())
                || final_value.get("eof").and_then(Value::as_bool) != Some(true) {
                return Err("remote final source identity or EOF receipt differs".into());
            }
            let sha = format!("{:x}", hasher.finalize());
            if input.expected_sha256.as_ref().is_some_and(|expected| expected != &sha) { return Err("download differs from expected SHA256".into()); }
            Ok((offset, sha))
        }.await;
        match result {
            Ok((size, sha)) => {
                let written = writer.commit().await.map_err(|e| e.to_string())?;
                if written != size {
                    return Err("committed VFS size differs".into());
                }
                Ok(
                    json!({"direction":"download","vfs_path":input.vfs_path,"remote_path":input.remote_path,"bytes":size,"sha256":sha,"committed":true,"data_endpoint":self.client.config().data_ssh_host}),
                )
            }
            Err(error) => {
                let _ = writer.abort().await;
                Err(error)
            }
        }
    }
}

#[async_trait]
impl ToolFunction for HpcTransferTool {
    type Input = HpcTransferInput;
    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        if [&input.expected_sha256, &input.existing_remote_sha256]
            .into_iter()
            .flatten()
            .any(|s| !valid_hash(s))
        {
            return Ok(ToolResult::error(
                "expected SHA256 must be lowercase hexadecimal",
            ));
        }
        let result = match input.direction.as_str() {
            "upload" => self.upload(&input).await,
            "download" => self.download(&input).await,
            _ => Err("direction must be upload or download".into()),
        };
        Ok(match result {
            Ok(value) => ToolResult::success_json(value),
            Err(error) => ToolResult::error(error),
        })
    }
}

pub fn registrations(
    client: Arc<HpcClient>,
    storage: Arc<OpendalFileStorage>,
) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(HpcTransferTool { client, storage })]
}
