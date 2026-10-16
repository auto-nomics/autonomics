//! Shared helpers and constants used by the VFS op submodules.

use agentik_core::tools::ToolError;
use agentik_sdk::types::ToolResult as AgentToolResult;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::storage::OpendalFileStorage;

/// Extract a non-empty path argument, returning a `ToolError` when it is
/// `None` or a blank string.
///
/// A blank path (e.g. `""`, `"   "`) would be normalised to the virtual
/// root `/` by [`OpendalFileStorage::normalize_path`], which is never a
/// valid target for per-file operations — attempting a write or delete on
/// it can destroy or corrupt the root directory.
pub(crate) fn require_path<'a>(
    path: Option<&'a str>,
    op_name: &str,
) -> Result<&'a str, ToolError> {
    path.filter(|p| !p.trim().is_empty())
        .ok_or_else(|| ToolError::ValidationFailed {
            message: format!("missing or empty 'path' for {op_name}"),
        })
}

/// Same as [`require_path`] but for `src`/`dst` arguments used by `cp`/`mv`.
pub(crate) fn require_named_path<'a>(
    path: Option<&'a str>,
    field: &str,
    op_name: &str,
) -> Result<&'a str, ToolError> {
    path.filter(|p| !p.trim().is_empty())
        .ok_or_else(|| ToolError::ValidationFailed {
            message: format!("missing or empty '{field}' for {op_name}"),
        })
}

/// Number of leading bytes sampled for binary detection.
pub(crate) const BINARY_SAMPLE_SIZE: usize = 8192;
/// A file is treated as binary when more than this fraction of the sampled
/// bytes are non-text control characters.
pub(crate) const BINARY_CONTROL_RATIO: f32 = 0.30;

#[derive(Debug, Clone)]
pub(crate) struct FollowedEntry {
    pub path: String,
    pub is_dir: bool,
}

pub(crate) fn local_backend_root(op: &opendal::Operator) -> Option<PathBuf> {
    (op.info().scheme() == "fs").then(|| PathBuf::from(op.info().root().trim_end_matches('/')))
}

/// Compare paths by component so pagination follows rendered tree order.
pub(crate) fn tree_path_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    let mut left = left.trim_matches('/').split('/');
    let mut right = right.trim_matches('/').split('/');
    loop {
        match (left.next(), right.next()) {
            (Some(left_part), Some(right_part)) => match left_part.cmp(right_part) {
                std::cmp::Ordering::Equal => continue,
                ordering => return ordering,
            },
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
        }
    }
}

pub(crate) fn entry_is_readable(
    storage: &OpendalFileStorage,
    path: &str,
    is_directory: bool,
) -> bool {
    if is_directory {
        storage.check_listable(path).is_ok()
    } else {
        storage.check_readable(path).is_ok()
    }
}

pub(crate) fn safe_local_join(root: &Path, remote: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for component in Path::new(remote).components() {
        match component {
            std::path::Component::Normal(part) => path.push(part),
            std::path::Component::ParentDir => {
                path.pop();
            }
            std::path::Component::CurDir | std::path::Component::RootDir => {}
            std::path::Component::Prefix(_) => {}
        }
    }
    path
}

pub(crate) async fn collect_followed_entries(
    op: &opendal::Operator,
    remote: &str,
    limit: usize,
) -> Result<Vec<FollowedEntry>, String> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let Some(root) = local_backend_root(op) else {
        return Ok(Vec::new());
    };

    let physical_root = safe_local_join(&root, remote);
    let root_metadata = match tokio::fs::metadata(&physical_root).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error.to_string()),
    };
    if !root_metadata.is_dir() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    let mut visited = HashSet::new();
    let root_canonical = tokio::fs::canonicalize(&physical_root)
        .await
        .map_err(|e| e.to_string())?;
    visited.insert(root_canonical);

    collect_followed_directory(
        &physical_root,
        remote.trim_matches('/'),
        limit,
        &mut entries,
        &mut visited,
    )
    .await?;

    Ok(entries)
}

/// Walk local directories depth-first in deterministic path order. Early
/// stopping is safe for pagination because collected paths are already in
/// the same order used by the rendered tree.
async fn collect_followed_directory(
    directory: &Path,
    logical_directory: &str,
    limit: usize,
    entries: &mut Vec<FollowedEntry>,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), String> {
    let mut reader = tokio::fs::read_dir(directory)
        .await
        .map_err(|e| e.to_string())?;
    let mut children = Vec::new();
    while let Some(child) = reader.next_entry().await.map_err(|e| e.to_string())? {
        let physical = child.path();
        let link_metadata = child.metadata().await.map_err(|e| e.to_string())?;
        let followed_metadata = tokio::fs::metadata(&physical).await;
        let metadata = match followed_metadata {
            Ok(metadata) => metadata,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && link_metadata.is_symlink() =>
            {
                link_metadata
            }
            Err(error) => return Err(error.to_string()),
        };

        let name = child.file_name().to_string_lossy().to_string();
        let mut path = logical_directory.trim_end_matches('/').to_string();
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(&name);
        let is_dir = metadata.is_dir();
        children.push((FollowedEntry { path, is_dir }, physical, is_dir));
    }
    children.sort_by(|a, b| tree_path_cmp(&a.0.path, &b.0.path));

    for (entry, physical, is_dir) in children {
        if entries.len() >= limit {
            return Ok(());
        }
        let should_descend = is_dir
            && match tokio::fs::canonicalize(&physical).await {
                Ok(canonical) => visited.insert(canonical),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error.to_string()),
            };
        let child_directory = entry.path.trim_end_matches('/').to_string();
        entries.push(entry);
        if entries.len() >= limit {
            return Ok(());
        }
        if should_descend {
            Box::pin(collect_followed_directory(
                &physical,
                &child_directory,
                limit,
                entries,
                visited,
            ))
            .await?;
        }
    }

    Ok(())
}

pub(crate) async fn followed_entry_metadata(
    op: &opendal::Operator,
    entry: &opendal::Entry,
) -> Result<opendal::Metadata, String> {
    if entry.metadata().is_dir() || entry.metadata().is_file() {
        return Ok(entry.metadata().clone());
    }
    match op.stat(entry.path()).await {
        Ok(metadata) => Ok(metadata),
        Err(error) if error.kind() == opendal::ErrorKind::NotFound => Ok(entry.metadata().clone()),
        Err(error) => Err(error.to_string()),
    }
}

/// Heuristic check for whether a byte slice is binary (non-text) data.
///
/// Mirrors the approach used by git and ripgrep: sample the leading bytes
/// and declare binary if either
/// 1. a NUL byte (`\0`) is present — a strong binary signal, or
/// 2. more than [`BINARY_CONTROL_RATIO`] of the sampled bytes are
///    non-text control characters (anything below `0x20` other than the
///    common whitespace bytes `\t`, `\n`, `\r`, plus `0x7f` DEL).
///
/// High bytes (`>= 0x80`) are intentionally ignored so that valid UTF-8
/// multibyte content (CJK, emoji, etc.) is not misclassified.
pub fn is_binary(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(BINARY_SAMPLE_SIZE)];
    if sample.is_empty() {
        return false;
    }
    if sample.contains(&0u8) {
        return true;
    }
    let non_text = sample
        .iter()
        .filter(|&&b| (b < 0x20 && b != b'\t' && b != b'\n' && b != b'\r') || b == 0x7f)
        .count();
    let ratio = non_text as f32 / sample.len() as f32;
    ratio > BINARY_CONTROL_RATIO
}

/// Build a structured "refused: binary" tool result so the agent gets a
/// clear explanation instead of garbled `from_utf8_lossy` output.
pub(crate) fn binary_refused(display_path: &str, size: u64) -> AgentToolResult {
    AgentToolResult::error(format!(
        "Refused to read '{display_path}': the file appears to be binary \
         ({size} bytes, non-text content detected). Reading it as text would \
         produce garbled output; use a dedicated tool to inspect binary data."
    ))
}
