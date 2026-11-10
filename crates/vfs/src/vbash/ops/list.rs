//! Listing and metadata operations: `ls`, `tree`, `wc`, `stat`, `mount_list`.
//!
//! `ls`, `stat` and `mount_list` are thin adapters over the typed
//! [`crate::browse`] facade so the agent tool and the gateway VFS API
//! share one mount-aware implementation.

use agentik_core::tools::{ToolError, ToolResult};
use agentik_sdk::types::ToolResult as AgentToolResult;
use futures::StreamExt;

use super::common::{
    FollowedEntry, collect_followed_entries, entry_is_readable, followed_entry_metadata,
    local_backend_root, require_path, tree_path_cmp,
};
use crate::browse::BrowseError;
use crate::storage::OpendalFileStorage;

/// Default and hard maximum entry counts for a paginated `tree` response.
const DEFAULT_TREE_LIMIT: usize = 500;
const TREE_MAX_LIMIT: usize = 1000;

/// `ls` — list directory entries.
///
/// Results are paginated: `offset` entries are skipped, then up to
/// `limit` entries are collected. The response includes a `truncated`
/// flag and a `next_offset` hint so the caller can continue listing when
/// more entries remain.
pub async fn op_ls(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    recursive: Option<bool>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let raw = path.unwrap_or("/");
    let listing =
        match crate::browse::list(storage, raw, recursive.unwrap_or(false), limit, offset).await {
            Ok(listing) => listing,
            Err(error) => {
                let vpath = storage
                    .try_resolve_virtual(raw)
                    .map_err(|e| e.to_string())?;
                return Ok(AgentToolResult::error(format!(
                    "ls: '{vpath}': {}",
                    error.message()
                )));
            }
        };

    let items: Vec<serde_json::Value> = listing
        .entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "name": entry.name,
                "path": entry.path,
                "uri": entry.uri,
                "is_dir": entry.is_dir,
                "size": entry.size,
            })
        })
        .collect();
    let mut payload = serde_json::json!({
        "path": listing.path,
        "entries": items,
        "returned": listing.returned,
        "truncated": listing.truncated,
    });
    if let Some(next_offset) = listing.next_offset {
        payload["next_offset"] = serde_json::json!(next_offset);
    }
    Ok(AgentToolResult::success_json(payload))
}

/// `stat` — file metadata.
pub async fn op_stat(
    storage: &OpendalFileStorage,
    path: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "stat")?;
    match crate::browse::stat(storage, raw_path).await {
        Ok(entry) => Ok(AgentToolResult::success_json(serde_json::json!({
            "path": raw_path,
            "is_dir": entry.is_dir,
            "size": entry.size,
            "last_modified": entry.last_modified.unwrap_or_default(),
        }))),
        Err(BrowseError::NotFound(message)) => Ok(AgentToolResult::error(message)),
        Err(error) => Ok(AgentToolResult::error(format!(
            "stat: '{raw_path}': {}",
            error.message()
        ))),
    }
}

/// `mount_list` — list every mount defined for this VFS.
///
/// Returns the full mount manifest (path / backend / source /
/// read_only) so the agent can introspect the available
/// mount points without needing them listed in the system prompt.
pub async fn op_mount_list(storage: &OpendalFileStorage) -> Result<AgentToolResult, ToolError> {
    let mounts = crate::vbash::agent_mounts(storage)?;
    Ok(AgentToolResult::success_json(serde_json::json!({
        "mounts": mounts,
        "count": mounts.len(),
    })))
}

/// `wc` — count lines, words, and bytes.
pub async fn op_wc(
    storage: &OpendalFileStorage,
    path: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "wc")?;
    let vpath = storage
        .try_resolve_virtual(raw_path)
        .map_err(|e| e.to_string())?;
    if let Err(error) = storage.check_readable(&vpath) {
        return Ok(AgentToolResult::error(format!("wc: '{raw_path}': {error}")));
    }
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    let buf = op.read(&remote).await.map_err(|e| e.to_string())?;
    let bytes = buf.len();
    let data = buf.to_vec();
    let text = String::from_utf8_lossy(&data);
    let lines = text.lines().count();
    let words = text.split_whitespace().count();

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "lines": lines,
        "words": words,
        "bytes": bytes,
    })))
}

/// `tree` — formatted recursive directory listing using standard
/// `├──` / `└──` / `│` tree characters (instead of plain indented
/// slashes which were hard to scan visually).
pub async fn op_tree(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let vpath = storage
        .try_resolve_virtual(path.unwrap_or("/"))
        .map_err(|e| e.to_string())?;
    if let Err(error) = storage.check_listable(&vpath) {
        return Ok(AgentToolResult::error(format!("tree: '{vpath}': {error}")));
    }
    let max_entries = limit.unwrap_or(DEFAULT_TREE_LIMIT).clamp(1, TREE_MAX_LIMIT);
    let skip = offset.unwrap_or(0);
    // One extra entry makes `truncated` exact even when a page is full.
    let collection_limit = skip.saturating_add(max_entries).saturating_add(1);

    // Dispatch through the mount table: the operator plus the
    // backend-local key it expects.
    let remote = storage.resolve_path(&vpath);
    // OpenDAL Fs backend requires a trailing '/' to walk recursively.
    let scan = if remote.is_empty() {
        "/".to_string()
    } else if remote.ends_with('/') {
        remote.clone()
    } else {
        format!("{remote}/")
    };
    let op: opendal::Operator = storage.resolve(&vpath);

    // Collect entries as (depth, path, is_dir). Drop the scan-root
    // self-entry to avoid duplicating it in the rendered tree.
    let prefix = vpath.trim_end_matches('/');
    let scan_root = remote.trim_end_matches('/').to_string();
    let followed_entries = if local_backend_root(&op).is_some() {
        collect_followed_entries(&op, &remote, collection_limit).await?
    } else {
        let mut lister = op
            .lister_with(&scan)
            .recursive(true)
            .await
            .map_err(|e| e.to_string())?;
        let mut followed = Vec::new();
        while let Some(entry) = lister.next().await {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path().to_string();
            let is_dir = entry.metadata().is_dir();
            if is_dir && path.trim_end_matches('/') == scan_root {
                continue;
            }
            followed.push(FollowedEntry { path, is_dir });
            if followed.len() >= collection_limit {
                break;
            }
        }
        followed
    };

    let mut all_entries: Vec<(usize, String, bool)> = Vec::new();
    for followed in followed_entries {
        let p_raw = followed.path;
        let is_dir = followed.is_dir;

        // Remap the backend-local path into the mount's virtual
        // namespace (e.g. `mnt/.../parquet/1000g_eur.parquet` →
        // `/data/ldsc/1000g_eur.parquet`).
        let remapped = storage.remap_entry_to_virtual(&vpath, &p_raw);
        let normalized = remapped.trim_end_matches('/');
        let p = format!("/{}", normalized.trim_start_matches('/'));
        if !entry_is_readable(storage, &p, is_dir) {
            continue;
        }
        let rel = if prefix.is_empty() {
            p.as_str()
        } else {
            p.strip_prefix(prefix).unwrap_or(&p).trim_start_matches('/')
        };
        let depth = rel.matches('/').count();
        all_entries.push((depth, format!("/{rel}"), is_dir));
    }

    all_entries.sort_by(|a, b| tree_path_cmp(&a.1, &b.1));
    let start = skip.min(all_entries.len());
    let end = start.saturating_add(max_entries).min(all_entries.len());
    let entries: Vec<(usize, String, bool)> = all_entries[start..end].to_vec();
    let truncated = end < all_entries.len();
    let returned = entries.len();
    let next_offset = truncated.then_some(skip + returned);

    // A page can begin at a nested entry. Keep its known ancestors in the
    // rendering graph (without counting them in this page) so paths remain
    // readable and directory metadata is not lost.
    let known_by_path: std::collections::HashMap<String, (usize, bool)> = all_entries
        .iter()
        .map(|(depth, path, is_dir)| (path.clone(), (*depth, *is_dir)))
        .collect();
    let selected_paths: std::collections::HashSet<&str> =
        entries.iter().map(|(_, path, _)| path.as_str()).collect();
    let mut render_paths: Vec<String> = entries.iter().map(|(_, path, _)| path.clone()).collect();
    for path in &selected_paths {
        let mut ancestor: &str = path;
        while let Some(index) = ancestor.rfind('/') {
            ancestor = &ancestor[..index];
            if ancestor.is_empty() {
                break;
            }
            if known_by_path.contains_key(ancestor) && !selected_paths.contains(ancestor) {
                render_paths.push(ancestor.to_string());
            }
        }
    }
    render_paths.sort_by(|a, b| tree_path_cmp(a, b));
    render_paths.dedup();
    let render_entries: Vec<(usize, String, bool)> = render_paths
        .into_iter()
        .map(|path| {
            let (depth, is_dir) = known_by_path[&path];
            (depth, path, is_dir)
        })
        .collect();

    let meta_by_path: std::collections::HashMap<String, bool> = render_entries
        .iter()
        .map(|(_, p, d)| (p.clone(), *d))
        .collect();

    // Collect all entry paths and the parent-directory path each one
    // belongs to. The walker below uses these to render the tree.
    let all_paths: Vec<String> = render_entries.iter().map(|(_, p, _)| p.clone()).collect();

    fn walk(
        parent: &str,
        ancestor_has_more: &mut Vec<bool>,
        all_paths: &[String],
        meta_by_path: &std::collections::HashMap<String, bool>,
        out: &mut String,
    ) {
        // The `parent` for the root is '/'; for a directory it's the
        // directory path (e.g. 'd'). A child of `parent` is any entry
        // whose immediate parent directory is `parent`. For a directory
        // entry like `d/`, its immediate children are all entries with
        // path `parent/segment` (depth relative to scan root = depth of
        // parent + 1).
        let mut children: Vec<&String> = all_paths
            .iter()
            .filter(|p| {
                if parent == "/" {
                    // Top-level: the first path segment is non-empty.
                    // OpenDAL returns directories with a trailing slash
                    // (e.g. `d/`), so trim it before checking for
                    // further segments.
                    let trimmed = p.trim_start_matches('/').trim_end_matches('/');
                    !trimmed.is_empty() && !trimmed.contains('/')
                } else {
                    // Entry belongs to `parent` if it is exactly one
                    // path segment deeper (e.g. parent='d' matches
                    // 'd/a.txt' but not 'd/a/b.txt'). The directory
                    // entry 'd/' is filtered here too — `p` may have a
                    // trailing slash and equal `parent + '/'`, in which
                    // case it represents `parent` itself and must not
                    // be re-descended into.
                    let trimmed = p.trim_start_matches('/').trim_end_matches('/');
                    let parent_trimmed = parent.trim_start_matches('/').trim_end_matches('/');
                    trimmed != parent_trimmed
                        && trimmed.starts_with(&format!("{}/", parent_trimmed))
                        && !trimmed[parent_trimmed.len() + 1..].contains('/')
                }
            })
            .collect();
        children.sort_by(|a, b| tree_path_cmp(a, b));
        if children.is_empty() {
            return;
        }
        let n = children.len();
        for (i, child) in children.iter().enumerate() {
            let is_last = i + 1 == n;
            let child_path = child.as_str();
            let is_dir = meta_by_path.get(*child).copied().unwrap_or(false);
            // Display name: the last path segment, with '/' for dirs.
            // We trim trailing '/' so a directory entry like 'd/'
            // produces the segment name 'd' rather than an empty string.
            let trimmed = child_path.trim_end_matches('/');
            let seg = trimmed.rsplit('/').next().unwrap_or(trimmed);
            let display = if is_dir {
                format!("{seg}/")
            } else {
                seg.to_string()
            };
            let connector = if is_last { "└── " } else { "├── " };
            for more in ancestor_has_more.iter() {
                out.push_str(if *more { "│   " } else { "    " });
            }
            out.push_str(connector);
            out.push_str(&display);
            out.push('\n');

            if is_dir {
                ancestor_has_more.push(!is_last);
                // Recurse with the directory path (no trailing slash)
                // so subsequent prefix lookups match child entries.
                walk(
                    child.trim_end_matches('/'),
                    ancestor_has_more,
                    all_paths,
                    meta_by_path,
                    out,
                );
                ancestor_has_more.pop();
            }
        }
    }

    let mut out = String::new();
    out.push_str(&crate::vbash::address::uri(storage, &vpath)?);
    out.push('\n');

    let mut ancestor_has_more: Vec<bool> = Vec::new();
    walk(
        "/",
        &mut ancestor_has_more,
        &all_paths,
        &meta_by_path,
        &mut out,
    );

    if truncated {
        let next = next_offset.unwrap_or_default();
        out.push_str(&format!(
            "... [truncated after {returned} entries; next_offset={next}]\n"
        ));
    }

    let mut payload = serde_json::json!({
        "path": vpath,
        "content": out,
        "entries": returned,
        "returned": returned,
        "truncated": truncated,
    });
    if let Some(offset) = next_offset {
        payload["next_offset"] = serde_json::json!(offset);
    }

    Ok(AgentToolResult::success_json(payload))
}
