//! Listing and metadata operations: `ls`, `tree`, `wc`, `stat`, `mount_list`.

use agentik_core::tools::{ToolError, ToolResult};
use agentik_sdk::types::ToolResult as AgentToolResult;
use futures::StreamExt;

use super::common::{
    collect_followed_entries, entry_is_readable, followed_entry_metadata, local_backend_root,
    require_path, tree_path_cmp, FollowedEntry,
};
use crate::storage::OpendalFileStorage;

/// Default maximum entries returned by `ls` when no explicit `limit` is given.
///
/// Kept modest so a single listing does not flood the agent's context
/// window. Callers may raise it via `limit`, or page with `offset`.
const DEFAULT_LS_LIMIT: usize = 200;
/// Default and hard maximum entry counts for a paginated `tree` response.
const DEFAULT_TREE_LIMIT: usize = 500;
const TREE_MAX_LIMIT: usize = 1000;

/// `ls` — list directory entries.
///
/// Results are paginated: `offset` entries are skipped, then up to `limit`
/// entries (default [`DEFAULT_LS_LIMIT`]) are collected. The response
/// includes a `truncated` flag and a `next_offset` hint so the caller can
/// continue listing when more entries remain.
pub async fn op_ls(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    recursive: Option<bool>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let vpath = OpendalFileStorage::normalize_path(path.unwrap_or("/"));
    if let Err(error) = storage.check_listable(&vpath) {
        return Ok(AgentToolResult::error(format!("ls: '{vpath}': {error}")));
    }
    let recursive = recursive.unwrap_or(false);
    let max = limit.unwrap_or(DEFAULT_LS_LIMIT).max(1);
    let skip = offset.unwrap_or(0);

    // ── Composite listing helper ──
    //
    // We collect entries from (a) the default-fs backend (after
    // dispatching through the mount table to find the covering mount
    // if any), and (b) synthetic entries for mount descendants, into a
    // single deduped, sorted, paginated list.
    //
    // The list returned has the form Vec<serde_json::Value> with one
    // entry per `name`, `is_dir`, `size`.
    let mut items: Vec<serde_json::Value> = Vec::with_capacity(max.min(1024));
    let mut truncated = false;

    let mount = storage.mounts.as_ref().and_then(|m| {
        use datafusion::object_store::path::Path as DsPath;
        let ds = DsPath::parse(&vpath).ok()?;
        m.handle_for(&ds)
    });
    let base = vpath.trim_end_matches('/');
    let child_mounts: Vec<String> = storage
        .mounts
        .as_ref()
        .map(|m| {
            m.mount_paths()
                .into_iter()
                .filter_map(|mount| {
                    let mount = mount.trim_end_matches('/');
                    if mount == base {
                        return None;
                    }
                    let suffix = if base == "/" {
                        mount.strip_prefix('/')?
                    } else {
                        mount.strip_prefix(base)?.strip_prefix('/')?
                    };
                    let first = suffix.split('/').next()?;
                    if first.is_empty() {
                        return None;
                    }
                    Some(if base == "/" {
                        format!("/{first}")
                    } else {
                        format!("{base}/{first}")
                    })
                })
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .unwrap_or_default();
    let child_mounts = child_mounts
        .into_iter()
        .filter(|mount| storage.check_listable(mount).is_ok())
        .collect::<Vec<_>>();

    // ── Case 1: recursive — use ObjectStore-style stream via opendal ──
    if recursive {
        // For recursive listing we walk the path on the resolved
        // operator (single backend), then append child mounts' contents.
        let op = storage.resolve(&vpath);
        let remote = storage.resolve_path(&vpath);
        let scan = if remote.is_empty() {
            "/".to_string()
        } else if remote.ends_with('/') {
            remote.clone()
        } else {
            format!("{remote}/")
        };
        let mut lister = op
            .lister_with(&scan)
            .recursive(true)
            .await
            .map_err(|e| e.to_string())?;
        let scan_root = remote.trim_end_matches('/').to_string();
        let mut idx = 0usize;
        while let Some(entry) = lister.next().await {
            let entry = entry.map_err(|e| e.to_string())?;
            let entry_path = entry.path().to_string();
            // Remap the backend-local entry back into the virtual
            // namespace for display and shadow checks.
            let display = storage.remap_entry_to_virtual(&vpath, &entry_path);
            if !entry_is_readable(storage, &display, entry.metadata().is_dir()) {
                continue;
            }
            // Skip the scan-root self-entry.
            if entry.metadata().is_dir() && entry_path.trim_end_matches('/') == scan_root {
                continue;
            }
            // Skip entries shadowed by a child mount.
            let trimmed = display.trim_end_matches('/');
            if child_mounts
                .iter()
                .any(|m| m.trim_end_matches('/') == trimmed)
            {
                continue;
            }
            idx += 1;
            if idx <= skip {
                continue;
            }
            if items.len() >= max {
                truncated = true;
                break;
            }
            let meta = entry.metadata();
            let is_dir = meta.is_dir();
            let size = if is_dir { 0 } else { meta.content_length() };
            items.push(serde_json::json!({
                "name": display,
                "is_dir": is_dir,
                "size": size,
            }));
        }

        // If we still have room, append each child mount's contents.
        if !truncated {
            for mp in &child_mounts {
                if let Some(mounts) = &storage.mounts {
                    use datafusion::object_store::path::Path as DsPath;
                    if let Ok(ds) = DsPath::parse(mp) {
                        if let Some(h) = mounts.handle_for(&ds) {
                            let child_op = (*h.backend_op).clone();
                            let child_remote = storage.resolve_path(mp);
                            let child_scan = if child_remote.is_empty() {
                                "/".to_string()
                            } else {
                                child_remote
                            };
                            let mut ml =
                                match child_op.lister_with(&child_scan).recursive(true).await {
                                    Ok(l) => l,
                                    Err(_) => continue,
                                };
                            while let Some(entry) = ml.next().await {
                                if let Ok(entry) = entry {
                                    let meta = entry.metadata();
                                    let entry_path = entry.path().to_string();
                                    let display = storage.remap_entry_to_virtual(mp, &entry_path);
                                    let is_dir = meta.is_dir();
                                    if !entry_is_readable(storage, &display, is_dir) {
                                        continue;
                                    }
                                    idx += 1;
                                    if idx <= skip {
                                        continue;
                                    }
                                    if items.len() >= max {
                                        truncated = true;
                                        break;
                                    }
                                    let size = if is_dir { 0 } else { meta.content_length() };
                                    items.push(serde_json::json!({
                                        "name": display,
                                        "is_dir": is_dir,
                                        "size": size,
                                    }));
                                }
                            }
                            if truncated {
                                break;
                            }
                        }
                    }
                }
            }
        }
    } else {
        // ── Case 2: non-recursive — list children of `vpath` ──
        // If a single mount covers vpath entirely, list inside it.
        if let Some(handle) = mount {
            let op = (*handle.backend_op).clone();
            let scan_remote = storage.resolve_path(&vpath);
            let scan_remote_cmp = scan_remote.clone();
            let scan = if scan_remote.ends_with('/') || scan_remote.is_empty() {
                if scan_remote.is_empty() {
                    "/".to_string()
                } else {
                    scan_remote
                }
            } else {
                format!("{scan_remote}/")
            };
            let mut lister = op
                .lister_with(&scan)
                .recursive(false)
                .await
                .map_err(|e| e.to_string())?;
            let mut idx = 0usize;

            // Mount points are namespace boundaries, not merely entries that
            // happen to exist in the covering backend. Emit them first so a
            // truncated listing still exposes the mounted branch.
            for mp in &child_mounts {
                idx += 1;
                if idx <= skip {
                    continue;
                }
                if items.len() >= max {
                    truncated = true;
                    break;
                }
                items.push(serde_json::json!({
                    "name": mp,
                    "is_dir": true,
                    "size": 0,
                }));
            }

            while let Some(entry) = lister.next().await {
                let entry = entry.map_err(|e| e.to_string())?;
                let entry_path_raw = entry.path().to_string();
                let metadata = followed_entry_metadata(&op, &entry).await?;
                if metadata.is_dir()
                    && entry_path_raw.trim_end_matches('/') == scan_remote_cmp.trim_end_matches('/')
                {
                    continue;
                }
                idx += 1;
                if idx <= skip {
                    continue;
                }
                if items.len() >= max {
                    truncated = true;
                    break;
                }
                let is_dir = metadata.is_dir();
                let size = if is_dir { 0 } else { metadata.content_length() };
                // Remap to virtual form (strip the mount's backend
                // source prefix, re-attach the virtual prefix).
                let display = storage.remap_entry_to_virtual(&vpath, &entry_path_raw);
                if !entry_is_readable(storage, &display, metadata.is_dir()) {
                    continue;
                }
                if metadata.is_dir()
                    && child_mounts
                        .iter()
                        .any(|m| m.trim_end_matches('/') == display.trim_end_matches('/'))
                {
                    continue;
                }
                items.push(serde_json::json!({
                    "name": display,
                    "is_dir": is_dir,
                    "size": size,
                }));
            }
        } else {
            // Default-fs non-recursive + synthetic direct-child mounts.
            let op = storage.resolve(&vpath);
            let scan = if vpath.ends_with('/') {
                vpath.clone()
            } else {
                format!("{vpath}/")
            };
            let mut lister = op
                .lister_with(&scan)
                .recursive(false)
                .await
                .map_err(|e| e.to_string())?;
            let scan_root = vpath.trim_end_matches('/').to_string();
            let mut idx = 0usize;

            for mp in &child_mounts {
                idx += 1;
                if idx <= skip {
                    continue;
                }
                if items.len() >= max {
                    truncated = true;
                    break;
                }
                items.push(serde_json::json!({
                    "name": mp,
                    "is_dir": true,
                    "size": 0,
                }));
            }

            while let Some(entry) = lister.next().await {
                let entry = entry.map_err(|e| e.to_string())?;
                let entry_path_raw = entry.path().to_string();
                let trimmed = entry_path_raw.trim_end_matches('/').to_string();
                let metadata = followed_entry_metadata(&op, &entry).await?;
                if !entry_is_readable(storage, &entry_path_raw, metadata.is_dir()) {
                    continue;
                }
                // Skip default-fs directories that are shadowed by mounts.
                if metadata.is_dir()
                    && child_mounts
                        .iter()
                        .any(|m| m.trim_end_matches('/') == trimmed)
                {
                    continue;
                }
                if metadata.is_dir() && trimmed == scan_root {
                    continue;
                }
                idx += 1;
                if idx <= skip {
                    continue;
                }
                if items.len() >= max {
                    truncated = true;
                    break;
                }
                let is_dir = metadata.is_dir();
                let size = if is_dir { 0 } else { metadata.content_length() };
                items.push(serde_json::json!({
                    "name": entry_path_raw,
                    "is_dir": is_dir,
                    "size": size,
                }));
            }
        }
    }

    let returned = items.len();
    let next_offset = if truncated {
        Some(skip + returned)
    } else {
        None
    };
    let mut payload = serde_json::json!({
        "path": vpath,
        "entries": items,
        "returned": returned,
        "truncated": truncated,
    });
    if let Some(no) = next_offset {
        payload["next_offset"] = serde_json::json!(no);
    }
    Ok(AgentToolResult::success_json(payload))
}

/// `stat` — file metadata.
pub async fn op_stat(
    storage: &OpendalFileStorage,
    path: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "stat")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    if let Err(error) = storage.check_readable(&vpath) {
        return Ok(AgentToolResult::error(format!(
            "stat: '{raw_path}': {error}"
        )));
    }
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);
    let meta = match op.stat(&remote).await {
        Ok(m) => m,
        Err(_) => {
            return Ok(AgentToolResult::error(format!(
                "File or directory does not exist: {raw_path}"
            )));
        }
    };

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "is_dir": meta.is_dir(),
        "size": meta.content_length(),
        "last_modified": meta.last_modified().map(|t| t.to_string()).unwrap_or_default(),
    })))
}

/// `mount_list` — list every mount defined for this VFS.
///
/// Returns the full mount manifest (path / backend / source /
/// read_only) so the agent can introspect the available
/// mount points without needing them listed in the system prompt.
pub async fn op_mount_list(storage: &OpendalFileStorage) -> Result<AgentToolResult, ToolError> {
    let mounts: Vec<serde_json::Value> = storage
        .mount_definitions()
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "path": d.path,
                "backend": d.backend,
                "source": d.source,
                "read_only": d.read_only,
            })
        })
        .collect();
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
    let vpath = OpendalFileStorage::normalize_path(raw_path);
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
    let vpath = OpendalFileStorage::normalize_path(path.unwrap_or("/"));
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
    out.push_str(&vpath);
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
