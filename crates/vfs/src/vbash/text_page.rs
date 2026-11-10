//! Bounded UTF-8 range reads; line count is deliberately irrelevant.
use agentik_core::tools::ToolError;
use agentik_sdk::types::ToolResult;
use sha2::{Digest, Sha256};

use crate::storage::OpendalFileStorage;
const MAX_PAGE_BYTES: usize = 4096;

fn invalid(message: &str) -> ToolError {
    ToolError::ValidationFailed {
        message: message.into(),
    }
}

// This detects metadata changes. It is explicitly not a content hash, file
// lock, or snapshot guarantee; callers retain their downloaded source hash.
fn version(meta: &opendal::Metadata) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{:?}",
                (
                    meta.content_length(),
                    meta.last_modified(),
                    meta.etag(),
                    meta.version()
                )
            )
            .as_bytes()
        )
    )
}

pub(super) async fn read(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    offset: Option<usize>,
    max_bytes: Option<usize>,
    expected_version: Option<&str>,
    limit: Option<usize>,
) -> Result<ToolResult, ToolError> {
    let path = path
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| invalid("read_chunk requires a nonempty path"))?;
    // Bound JSON metadata as well as the content. No page is allowed to lose
    // its cursor to the outer generic tool-output truncation.
    if path.len() > 1024 {
        return Err(invalid("read_chunk path exceeds 1024 bytes"));
    }
    if limit.is_some() {
        return Err(invalid("read_chunk uses max_bytes, not a line limit"));
    }
    let budget = max_bytes.unwrap_or(MAX_PAGE_BYTES);
    if !(4..=MAX_PAGE_BYTES).contains(&budget) {
        return Err(invalid("read_chunk max_bytes must be 4..4096"));
    }
    let virtual_path = storage
        .try_resolve_virtual(path)
        .map_err(|e| e.to_string())?;
    storage
        .check_readable(&virtual_path)
        .map_err(|e| e.to_string())?;
    let op = storage.resolve(&virtual_path);
    let remote = storage.resolve_path(&virtual_path);
    let meta = op.stat(&remote).await.map_err(|e| e.to_string())?;
    if meta.is_dir() {
        return Err(invalid("read_chunk requires a file"));
    }
    let token = version(&meta);
    if expected_version.is_some_and(|v| v != token) {
        return Err(invalid(
            "read_chunk source metadata changed; restart pagination",
        ));
    }
    let start = offset.unwrap_or(0) as u64;
    if start > 0 && expected_version.is_none() {
        return Err(invalid(
            "read_chunk continuation requires the previous source_version",
        ));
    }
    let total = meta.content_length();
    if start > total {
        return Err(invalid("read_chunk offset exceeds file size"));
    }
    let end = start.saturating_add(budget as u64 + 3).min(total);
    let data = if start == total {
        Vec::new()
    } else {
        op.reader(&remote)
            .await
            .map_err(|e| e.to_string())?
            .read(start..end)
            .await
            .map_err(|e| e.to_string())?
            .to_vec()
    };
    if data.len() as u64 != end - start {
        return Err(invalid(
            "read_chunk short range read; source may have changed",
        ));
    }
    if data.contains(&0) {
        return Err(invalid("read_chunk refuses binary/NUL data"));
    }
    let valid = match std::str::from_utf8(&data) {
        Ok(s) => s,
        Err(e) if e.error_len().is_none() && end < total => {
            std::str::from_utf8(&data[..e.valid_up_to()])
                .map_err(|_| invalid("read_chunk invalid UTF-8"))?
        }
        Err(_) => {
            return Err(invalid(
                "read_chunk requires UTF-8 at the requested byte boundary; use returned next_offset",
            ));
        }
    };
    let mut take = valid.len().min(budget);
    while !valid.is_char_boundary(take) {
        take -= 1;
    }
    if take == 0 && start < total {
        return Err(invalid("read_chunk cannot advance at this byte boundary"));
    }
    let after = op.stat(&remote).await.map_err(|e| e.to_string())?;
    if version(&after) != token {
        return Err(invalid(
            "read_chunk source metadata changed during range read",
        ));
    }
    let next = start + take as u64;
    let eof = next == total;
    Ok(ToolResult::success_json(serde_json::json!({
        "path": path,
        "content": &valid[..take],
        "byte_offset": start,
        "bytes_read": take,
        "total_bytes": total,
        "next_offset": if eof { None } else { Some(next) },
        "eof": eof,
        "source_version": token,
        "source_version_kind": "metadata_consistency_token_not_content_hash",
        "output_truncated": false,
        "content_complete": eof && start == 0,
    })))
}
