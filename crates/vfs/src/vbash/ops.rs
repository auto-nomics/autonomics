//! Operation implementations for the VFS bash tool.
//!
//! Every function takes an [`opendal::Operator`] reference and returns a
//! [`ToolResult`]. No subprocess is spawned anywhere in this module.

use agentik_core::tools::{ToolError, ToolResult};
use agentik_sdk::types::{ToolImageSource, ToolResult as AgentToolResult, ToolResultBlock};
use base64::Engine as _;
use futures::StreamExt;

use crate::storage::OpendalFileStorage;

/// Extract a non-empty path argument, returning a `ToolError` when it is
/// `None` or a blank string.
///
/// A blank path (e.g. `""`, `"   "`) would be normalised to the virtual
/// root `/` by [`OpendalFileStorage::normalize_path`], which is never a
/// valid target for per-file operations — attempting a write or delete on
/// it can destroy or corrupt the root directory.
fn require_path<'a>(path: Option<&'a str>, op_name: &str) -> Result<&'a str, ToolError> {
    path.filter(|p| !p.trim().is_empty())
        .ok_or_else(|| ToolError::ValidationFailed {
            message: format!("missing or empty 'path' for {op_name}"),
        })
}

/// Same as [`require_path`] but for `src`/`dst` arguments used by `cp`/`mv`.
fn require_named_path<'a>(
    path: Option<&'a str>,
    field: &str,
    op_name: &str,
) -> Result<&'a str, ToolError> {
    path.filter(|p| !p.trim().is_empty())
        .ok_or_else(|| ToolError::ValidationFailed {
            message: format!("missing or empty '{field}' for {op_name}"),
        })
}

/// Default maximum lines for `read` when no explicit `limit` is given.
const DEFAULT_MAX_LINES: usize = 2000;
/// Maximum file size that `cat` will read into memory. Callers may request a
/// lower per-call limit, but cannot exceed this hard cap.
const CAT_MAX_BYTES: usize = 10 * 1024 * 1024;
/// Image extensions recognised by the `read` op.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];
/// Number of leading bytes sampled for binary detection.
const BINARY_SAMPLE_SIZE: usize = 8192;
/// A file is treated as binary when more than this fraction of the sampled
/// bytes are non-text control characters.
const BINARY_CONTROL_RATIO: f32 = 0.30;

// ══════════════════ reading ops ══════════════════

/// `read` — rich read with line numbers (text), base64 (image), or formatted (notebook).
pub async fn op_read(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "read")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    let ext = std::path::Path::new(raw_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if ext == "ipynb" {
        return read_notebook(&op, &remote, raw_path).await;
    }
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return read_image(&op, &remote, raw_path, &ext).await;
    }
    read_text_numbered(&op, &remote, raw_path, offset, limit).await
}

/// `cat` — plain text read, no line numbers, no special formatting.
pub async fn op_cat(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    offset: Option<usize>,
    limit: Option<usize>,
    max_bytes: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "cat")?;
    let max_bytes = max_bytes.unwrap_or(CAT_MAX_BYTES);
    if max_bytes == 0 || max_bytes > CAT_MAX_BYTES {
        return Err(ToolError::ValidationFailed {
            message: format!("cat 'max_bytes' must be between 1 and {CAT_MAX_BYTES}"),
        });
    }

    let vpath = OpendalFileStorage::normalize_path(raw_path);
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    let meta = match op.stat(&remote).await {
        Ok(m) => m,
        Err(e) if matches!(e.kind(), opendal::ErrorKind::NotFound) => {
            return Ok(AgentToolResult::error(format!(
                "cat: '{raw_path}': No such file or directory"
            )));
        }
        Err(e) => return Ok(AgentToolResult::error(format!("cat: '{raw_path}': {e}"))),
    };
    if meta.is_dir() {
        return Ok(AgentToolResult::error(format!(
            "cat: '{raw_path}' is a directory. Use 'ls' or 'tree' to list its contents."
        )));
    }
    let total_size = meta.content_length();
    if total_size == 0 {
        return Ok(AgentToolResult::success_json(serde_json::json!({
            "path": raw_path,
            "content": "(file is empty)",
            "total_size": 0,
        })));
    }
    if total_size > max_bytes as u64 {
        return Ok(AgentToolResult::error(format!(
            "cat: '{raw_path}' is too large: {total_size} bytes (max_bytes: {max_bytes})"
        )));
    }

    let buf = op.read(&remote).await.map_err(|e| e.to_string())?;
    let bytes = buf.to_vec();
    if is_binary(&bytes) {
        return Ok(binary_refused(raw_path, bytes.len() as u64));
    }
    let content = String::from_utf8_lossy(&bytes);

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    let start = offset.unwrap_or(1).saturating_sub(1).min(total_lines);
    let end = match limit {
        Some(n) => (start + n).min(total_lines),
        None => total_lines,
    };

    let out: String = lines[start..end].join("\n");

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "content": out,
        "start_line": start + 1,
        "lines_returned": end.saturating_sub(start),
        "total_lines": total_lines,
    })))
}

/// `head` — first N lines (default 10).
///
/// `offset` (1-indexed) skips that many leading lines before applying
/// `limit`. Mirrors `cat` / `read` semantics so the three ops stay
/// interchangeable for paging through a file.
pub async fn op_head(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "head")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    let n = limit.unwrap_or(10);
    let start = offset.unwrap_or(1).saturating_sub(1);
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    let meta = match op.stat(&remote).await {
        Ok(m) => m,
        Err(e) if matches!(e.kind(), opendal::ErrorKind::NotFound) => {
            return Ok(AgentToolResult::error(format!(
                "head: '{raw_path}': No such file or directory"
            )));
        }
        Err(e) => return Ok(AgentToolResult::error(format!("head: '{raw_path}': {e}"))),
    };
    if meta.is_dir() {
        return Ok(AgentToolResult::error(format!(
            "head: '{raw_path}' is a directory. Use 'ls' or 'tree' to list its contents."
        )));
    }

    let buf = op.read(&remote).await.map_err(|e| e.to_string())?;
    let bytes = buf.to_vec();
    if is_binary(&bytes) {
        return Ok(binary_refused(raw_path, bytes.len() as u64));
    }
    let content = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let start = start.min(total_lines);
    let end = (start + n).min(total_lines);
    let out: String = lines[start..end].join("\n");

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "content": out,
        "start_line": start + 1,
        "lines_returned": end - start,
        "total_lines": total_lines,
    })))
}

/// `tail` — last N lines (default 10).
///
/// `offset` is **not** meaningful for `tail` (Unix tail has no offset
/// concept). If the caller passes one it is silently ignored and the
/// response sets `offset_ignored: true` so the caller can detect the
/// mismatch and switch to `cat` / `read` if they meant to page.
pub async fn op_tail(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "tail")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    let n = limit.unwrap_or(10);
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    let meta = match op.stat(&remote).await {
        Ok(m) => m,
        Err(e) if matches!(e.kind(), opendal::ErrorKind::NotFound) => {
            return Ok(AgentToolResult::error(format!(
                "tail: '{raw_path}': No such file or directory"
            )));
        }
        Err(e) => return Ok(AgentToolResult::error(format!("tail: '{raw_path}': {e}"))),
    };
    if meta.is_dir() {
        return Ok(AgentToolResult::error(format!(
            "tail: '{raw_path}' is a directory. Use 'ls' or 'tree' to list its contents."
        )));
    }

    let buf = op.read(&remote).await.map_err(|e| e.to_string())?;
    let bytes = buf.to_vec();
    if is_binary(&bytes) {
        return Ok(binary_refused(raw_path, bytes.len() as u64));
    }
    let content = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let start = total.saturating_sub(n);

    let out: String = lines[start..].join("\n");

    let mut payload = serde_json::json!({
        "path": raw_path,
        "content": out,
        "lines_returned": total - start,
        "total_lines": total,
    });
    if offset.is_some() {
        payload["offset_ignored"] = serde_json::json!(true);
    }

    Ok(AgentToolResult::success_json(payload))
}

// ══════════════════ writing ops ══════════════════

/// `write` — create or overwrite a file.
///
/// The implementation performs an explicit delete-then-write to sidestep
/// the OpenDAL Fs backend race that produces `writer got too little data`
/// when an existing file is overwritten with a longer payload. After the
/// write we retry-stat to defend against the same backend's brief
/// final-consistency window where a freshly written file is not yet
/// visible to a follow-up stat.
pub async fn op_write(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    content: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "write")?;
    let content = content.unwrap_or("").to_string();
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    if let Err(e) = storage.check_writable(&vpath) {
        return Ok(AgentToolResult::error(format!("write: {raw_path}: {e}")));
    }
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);
    let size = content.len() as u64;

    // Atomic-replace path: drop any existing entry first so the Fs
    // backend's overwrite path cannot race with its size bookkeeping.
    if let Err(e) = op.delete(&remote).await {
        if !matches!(e.kind(), opendal::ErrorKind::NotFound) {
            return Ok(AgentToolResult::error(format!(
                "write: failed to clear {raw_path}: {e}"
            )));
        }
    }

    op.write(&remote, content.into_bytes())
        .await
        .map_err(|e| e.to_string())?;

    // Visibility probe: the Fs backend occasionally returns a stale
    // NotFound on the stat that immediately follows a write in the same
    // process. Retry a handful of times before declaring a real failure.
    for attempt in 0..5 {
        if op.stat(&remote).await.is_ok() {
            return Ok(AgentToolResult::success_json(serde_json::json!({
                "path": raw_path,
                "size": size,
            })));
        }
        tokio::time::sleep(std::time::Duration::from_millis(5 * (attempt as u64 + 1))).await;
    }

    Ok(AgentToolResult::error(format!(
        "write: wrote {size} bytes to {raw_path} but the file is not yet visible to a follow-up stat. \
         The backend may be in an inconsistent state; retry or verify with a separate tool call."
    )))
}

/// `edit` — fuzzy line-based string replacement backed by `apply-patch`.
///
/// Uses [`apply_patch::fuzzy_edit`] which matches `old_string` as a sequence
/// of complete lines with progressive tolerance:
///
/// 1. **Exact** — byte-for-byte equality per line.
/// 2. **rstrip** — ignore trailing whitespace.
/// 3. **trim** — ignore leading and trailing whitespace.
/// 4. **Unicode-normalised** — map smart quotes, en-dashes, NBSP, … to ASCII.
/// If a one-line pattern has no whole-line match, it is also tried as a
/// substring within each physical line.
///
/// All error paths return `Ok(ToolResult::error(...))` (never `Err`) so the
/// LLM always receives a structured result.
pub async fn op_edit(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    old_string: Option<&str>,
    new_string: Option<&str>,
    replace_all: Option<bool>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = match path {
        Some(p) if !p.trim().is_empty() => p,
        _ => return Ok(AgentToolResult::error("missing or empty 'path' for edit")),
    };
    let old = match old_string {
        Some(s) => s,
        None => return Ok(AgentToolResult::error("missing 'old_string' for edit")),
    };
    let new = match new_string {
        Some(s) => s,
        None => return Ok(AgentToolResult::error("missing 'new_string' for edit")),
    };

    if old == new {
        return Ok(AgentToolResult::error(
            "old_string must differ from new_string",
        ));
    }

    let vpath = OpendalFileStorage::normalize_path(raw_path);
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    // Read current content.
    let buf = match op.read(&remote).await {
        Ok(b) => b,
        Err(e) => {
            return Ok(AgentToolResult::error(format!(
                "Failed to read {raw_path}: {e}"
            )));
        }
    };
    let text = match String::from_utf8(buf.to_vec()) {
        Ok(s) => s,
        Err(_) => {
            return Ok(AgentToolResult::error(format!(
                "File {raw_path} is not valid UTF-8"
            )));
        }
    };

    let replace_all = replace_all.unwrap_or(false);
    let outcome = apply_patch::fuzzy_edit(&text, old, new, replace_all);

    match outcome {
        apply_patch::FuzzyEditOutcome::NotFound => Ok(AgentToolResult::error(
            "old_string not found in file (tried exact, rstrip, trim, and Unicode-normalised matching)",
        )),
        apply_patch::FuzzyEditOutcome::Ambiguous { count } => Ok(AgentToolResult::error(format!(
            "old_string matches {count} locations; set replace_all=true or make old_string unique"
        ))),
        apply_patch::FuzzyEditOutcome::Replaced {
            new_content,
            count,
            fuzzy,
        } => {
            if let Err(e) = op.write(&remote, new_content.into_bytes()).await {
                return Ok(AgentToolResult::error(format!(
                    "Failed to write {raw_path}: {e}"
                )));
            }
            Ok(AgentToolResult::success_json(serde_json::json!({
                "path": raw_path,
                "replacements": count,
                "fuzzy": fuzzy,
            })))
        }
    }
}

/// `patch` — apply a Codex-format multi-file patch through the VFS.
///
/// Parses the patch text with [`apply_patch::parse_patch`], then for each
/// hunk performs the appropriate OpenDAL operation:
///
/// * **AddFile** — write new content (creates parent dirs implicitly via
///   OpenDAL).
/// * **DeleteFile** — remove the file.
/// * **UpdateFile** — read original, compute new content via
///   [`apply_patch::compute_updated_content`] (fuzzy line matching), write
///   result.  If `*** Move to:` is present, writes to the destination and
///   deletes the original.
///
/// All error paths return `Ok(ToolResult::error(...))` so the LLM always
/// receives a structured result.
pub async fn op_patch(
    storage: &OpendalFileStorage,
    patch: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let patch_text = match patch {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            return Ok(AgentToolResult::error(
                "missing or empty 'patch' for patch op",
            ));
        }
    };

    let args = match apply_patch::parse_patch(patch_text) {
        Ok(a) => a,
        Err(e) => {
            return Ok(AgentToolResult::error(format!("Parse error: {e}")));
        }
    };

    if args.hunks.is_empty() {
        return Ok(AgentToolResult::error("Patch contains no file operations"));
    }

    let mut changes = Vec::new();

    for hunk in &args.hunks {
        match hunk {
            apply_patch::Hunk::AddFile { path, contents } => {
                let vpath = OpendalFileStorage::normalize_path(&path.display().to_string());
                if let Err(e) = storage.check_writable(&vpath) {
                    return Ok(AgentToolResult::error(format!(
                        "Failed to write {}: {e}",
                        path.display()
                    )));
                }
                let op = storage.resolve(&vpath);
                let remote = storage.resolve_path(&vpath);
                if let Err(e) = op.write(&remote, contents.clone().into_bytes()).await {
                    return Ok(AgentToolResult::error(format!(
                        "Failed to write {}: {e}",
                        path.display()
                    )));
                }
                changes.push(serde_json::json!({
                    "action": "add",
                    "path": path.display().to_string(),
                }));
            }

            apply_patch::Hunk::DeleteFile { path } => {
                let vpath = OpendalFileStorage::normalize_path(&path.display().to_string());
                if let Err(e) = storage.check_writable(&vpath) {
                    return Ok(AgentToolResult::error(format!(
                        "Failed to delete {}: {e}",
                        path.display()
                    )));
                }
                let op = storage.resolve(&vpath);
                let remote = storage.resolve_path(&vpath);
                if let Err(e) = op.delete(&remote).await {
                    return Ok(AgentToolResult::error(format!(
                        "Failed to delete {}: {e}",
                        path.display()
                    )));
                }
                changes.push(serde_json::json!({
                    "action": "delete",
                    "path": path.display().to_string(),
                }));
            }

            apply_patch::Hunk::UpdateFile {
                path,
                move_path,
                chunks,
            } => {
                let src_vpath = OpendalFileStorage::normalize_path(&path.display().to_string());
                let src_op = storage.resolve(&src_vpath);
                let src_remote = storage.resolve_path(&src_vpath);

                // Read original content.
                let buf = match src_op.read(&src_remote).await {
                    Ok(b) => b,
                    Err(e) => {
                        return Ok(AgentToolResult::error(format!(
                            "Failed to read {}: {e}",
                            path.display()
                        )));
                    }
                };
                let original = match String::from_utf8(buf.to_vec()) {
                    Ok(s) => s,
                    Err(_) => {
                        return Ok(AgentToolResult::error(format!(
                            "File {} is not valid UTF-8",
                            path.display()
                        )));
                    }
                };

                // Compute new content (fuzzy line matching, no I/O).
                let new_content = match apply_patch::compute_updated_content(&original, chunks) {
                    Ok(c) => c,
                    Err(e) => {
                        return Ok(AgentToolResult::error(format!(
                            "Failed to compute patch for {}: {e}",
                            path.display()
                        )));
                    }
                };

                let dest_display = move_path
                    .as_ref()
                    .map(|d| d.display().to_string())
                    .unwrap_or_else(|| path.display().to_string());

                let dest_vpath = OpendalFileStorage::normalize_path(&dest_display);
                if let Err(e) = storage.check_writable(&dest_vpath) {
                    return Ok(AgentToolResult::error(format!(
                        "Failed to write {}: {e}",
                        dest_display
                    )));
                }
                let dest_op = storage.resolve(&dest_vpath);
                let dest_remote = storage.resolve_path(&dest_vpath);

                // Write result.
                if let Err(e) = dest_op.write(&dest_remote, new_content.into_bytes()).await {
                    return Ok(AgentToolResult::error(format!(
                        "Failed to write {}: {e}",
                        dest_display
                    )));
                }

                // Remove original on move. The original lives on the
                // source backend (it may differ from the destination's),
                // so delete through the source operator + source key.
                if move_path.is_some() {
                    if let Err(e) = src_op.delete(&src_remote).await {
                        return Ok(AgentToolResult::error(format!(
                            "Failed to remove original {}: {e}",
                            path.display()
                        )));
                    }
                }

                changes.push(serde_json::json!({
                    "action": if move_path.is_some() { "move" } else { "update" },
                    "path": dest_display,
                }));
            }
        }
    }

    Ok(AgentToolResult::success_json(serde_json::json!({
        "changes": changes,
    })))
}

/// `touch` — create empty file if it doesn't exist; no-op if it does
/// (OpenDAL cannot set mtime).
pub async fn op_touch(
    storage: &OpendalFileStorage,
    path: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "touch")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    if let Err(e) = storage.check_writable(&vpath) {
        return Ok(AgentToolResult::error(format!("touch: {raw_path}: {e}")));
    }
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    let exists = op.stat(&remote).await.is_ok();
    if !exists {
        op.write(&remote, "").await.map_err(|e| e.to_string())?;
    }

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "created": !exists,
    })))
}

// ══════════════════ filesystem ops ══════════════════

/// Default maximum entries returned by `ls` when no explicit `limit` is given.
///
/// Kept modest so a single listing does not flood the agent's context
/// window. Callers may raise it via `limit`, or page with `offset`.
const DEFAULT_LS_LIMIT: usize = 200;

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
                                    idx += 1;
                                    if idx <= skip {
                                        continue;
                                    }
                                    if items.len() >= max {
                                        truncated = true;
                                        break;
                                    }
                                    let meta = entry.metadata();
                                    let entry_path = entry.path().to_string();
                                    let display = storage.remap_entry_to_virtual(mp, &entry_path);
                                    let is_dir = meta.is_dir();
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
                if entry.metadata().is_dir()
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
                let meta = entry.metadata();
                let is_dir = meta.is_dir();
                let size = if is_dir { 0 } else { meta.content_length() };
                // Remap to virtual form (strip the mount's backend
                // source prefix, re-attach the virtual prefix).
                let display = storage.remap_entry_to_virtual(&vpath, &entry_path_raw);
                if entry.metadata().is_dir()
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
                // Skip default-fs directories that are shadowed by mounts.
                if entry.metadata().is_dir()
                    && child_mounts
                        .iter()
                        .any(|m| m.trim_end_matches('/') == trimmed)
                {
                    continue;
                }
                if entry.metadata().is_dir() && trimmed == scan_root {
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

/// `mkdir` — create a directory.
pub async fn op_mkdir(
    storage: &OpendalFileStorage,
    path: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "mkdir")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    if let Err(e) = storage.check_writable(&vpath) {
        return Ok(AgentToolResult::error(format!("mkdir: {raw_path}: {e}")));
    }
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    // OpenDAL Fs backend requires a trailing '/' for directory creation.
    let dir_path = if remote.is_empty() {
        "/".to_string()
    } else if remote.ends_with('/') {
        remote
    } else {
        format!("{remote}/")
    };

    op.create_dir(&dir_path).await.map_err(|e| e.to_string())?;

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "created": true,
    })))
}

/// `rm` — delete a file or directory.
///
/// Safety contract:
/// - The virtual root (`/`) is always refused.
/// - Missing targets are reported as errors (no silent success).
/// - Non-empty directories are refused unless `recursive=true` is passed;
///   empty directories can be removed without the flag.
pub async fn op_rm(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    recursive: Option<bool>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "rm")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);

    if vpath == "/" {
        return Ok(AgentToolResult::error(
            "Refusing to rm '/': that is the virtual filesystem root. \
             Use a sub-path like '/tmp' instead.",
        ));
    }
    if let Err(e) = storage.check_writable(&vpath) {
        return Ok(AgentToolResult::error(format!("rm: {raw_path}: {e}")));
    }
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    let meta = match op.stat(&remote).await {
        Ok(m) => m,
        Err(e) if matches!(e.kind(), opendal::ErrorKind::NotFound) => {
            return Ok(AgentToolResult::error(format!(
                "rm: cannot remove '{raw_path}': No such file or directory"
            )));
        }
        Err(e) => return Ok(AgentToolResult::error(format!("rm: {e}"))),
    };

    let recursive_flag = recursive.unwrap_or(false);

    if meta.is_dir() && !recursive_flag {
        // Empty-directory removal: succeeds only if the directory has no
        // entries. Non-empty directories are explicitly refused here so
        // the caller sees a clear message instead of a silent wipe.
        return match op.delete(&remote).await {
            Ok(()) => Ok(AgentToolResult::success_json(serde_json::json!({
                "path": raw_path,
                "deleted": true,
                "recursive": false,
            }))),
            Err(e) => Ok(AgentToolResult::error(format!(
                "rm: cannot remove '{raw_path}': {e}. \
                 If the directory is non-empty, pass recursive=true to remove it and its contents."
            ))),
        };
    }

    op.delete_with(&remote)
        .recursive(recursive_flag)
        .await
        .map_err(|e| e.to_string())?;

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "deleted": true,
        "recursive": recursive_flag,
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

/// `cp` — copy a file.
pub async fn op_cp(
    storage: &OpendalFileStorage,
    src: Option<&str>,
    dst: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_src = require_named_path(src, "src", "cp")?;
    let raw_dst = require_named_path(dst, "dst", "cp")?;
    let vsrc = OpendalFileStorage::normalize_path(raw_src);
    let vdst = OpendalFileStorage::normalize_path(raw_dst);
    if let Err(e) = storage.check_writable(&vdst) {
        return Ok(AgentToolResult::error(format!("cp: {raw_dst}: {e}")));
    }
    let src_op = storage.resolve(&vsrc);
    let dst_op = storage.resolve(&vdst);
    let src_remote = storage.resolve_path(&vsrc);
    let dst_remote = storage.resolve_path(&vdst);
    // OpenDAL copy is backend-internal. If src and dst resolve to
    // different backends, fall back to read+write.
    if !storage.same_backend(&vsrc, &vdst) {
        let buf = src_op.read(&src_remote).await.map_err(|e| e.to_string())?;
        dst_op
            .write(&dst_remote, buf)
            .await
            .map_err(|e| e.to_string())?;
    } else {
        src_op
            .copy(&src_remote, &dst_remote)
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(AgentToolResult::success_json(serde_json::json!({
        "src": raw_src,
        "dst": raw_dst,
        "copied": true,
    })))
}

/// `mv` — rename/move a file.
pub async fn op_mv(
    storage: &OpendalFileStorage,
    src: Option<&str>,
    dst: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_src = require_named_path(src, "src", "mv")?;
    let raw_dst = require_named_path(dst, "dst", "mv")?;
    let vsrc = OpendalFileStorage::normalize_path(raw_src);
    let vdst = OpendalFileStorage::normalize_path(raw_dst);
    if let Err(e) = storage.check_writable(&vsrc) {
        return Ok(AgentToolResult::error(format!("mv: {raw_src}: {e}")));
    }
    if let Err(e) = storage.check_writable(&vdst) {
        return Ok(AgentToolResult::error(format!("mv: {raw_dst}: {e}")));
    }
    let src_op = storage.resolve(&vsrc);
    let dst_op = storage.resolve(&vdst);
    let src_remote = storage.resolve_path(&vsrc);
    let dst_remote = storage.resolve_path(&vdst);
    if !storage.same_backend(&vsrc, &vdst) {
        let buf = src_op.read(&src_remote).await.map_err(|e| e.to_string())?;
        dst_op
            .write(&dst_remote, buf)
            .await
            .map_err(|e| e.to_string())?;
        src_op
            .delete(&src_remote)
            .await
            .map_err(|e| e.to_string())?;
    } else {
        src_op
            .rename(&src_remote, &dst_remote)
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(AgentToolResult::success_json(serde_json::json!({
        "src": raw_src,
        "dst": raw_dst,
        "moved": true,
    })))
}

/// `wc` — count lines, words, and bytes.
pub async fn op_wc(
    storage: &OpendalFileStorage,
    path: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "wc")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
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
) -> Result<AgentToolResult, ToolError> {
    let vpath = OpendalFileStorage::normalize_path(path.unwrap_or("/"));
    let max_entries = limit.unwrap_or(500);

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
    let mut lister = op
        .lister_with(&scan)
        .recursive(true)
        .await
        .map_err(|e| e.to_string())?;

    // Collect entries as (depth, path, is_dir). Drop the scan-root
    // self-entry to avoid duplicating it in the rendered tree.
    let mut entries: Vec<(usize, String, bool)> = Vec::new();
    let prefix = vpath.trim_end_matches('/');
    let scan_root = remote.trim_end_matches('/').to_string();
    while let Some(entry) = lister.next().await {
        let entry = entry.map_err(|e| e.to_string())?;
        let p_raw = entry.path().to_string();
        let is_dir = entry.metadata().is_dir();

        if is_dir && p_raw.trim_end_matches('/') == scan_root {
            continue;
        }

        // Remap the backend-local path into the mount's virtual
        // namespace (e.g. `mnt/.../parquet/1000g_eur.parquet` →
        // `/data/ldsc/1000g_eur.parquet`).
        let p = storage.remap_entry_to_virtual(&vpath, &p_raw);
        let rel = if prefix.is_empty() {
            p.as_str()
        } else {
            p.strip_prefix(prefix).unwrap_or(&p).trim_start_matches('/')
        };
        let depth = rel.matches('/').count();
        entries.push((depth, p, is_dir));
        if entries.len() >= max_entries {
            break;
        }
    }

    entries.sort_by(|a, b| a.1.cmp(&b.1));

    // Index immediate children by parent path. Top-level entries
    // (depth == 1 relative to the scan root) are always parented to '/'.
    // OpenDAL returns each top-level directory as a trailing-slash
    // entry like 'd/', so computing parent from `rfind('/')` would
    // wrongly assign `d/` as a child of `d` (its own directory name).
    // Using the recorded depth sidesteps that.
    let mut children_of: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    children_of.insert("/".to_string(), Vec::new());
    for (depth, p, _) in &entries {
        let parent = if *depth == 1 {
            "/".to_string()
        } else if let Some(idx) = p.rfind('/') {
            if idx == 0 {
                "/".to_string()
            } else {
                p[..idx].to_string()
            }
        } else {
            "/".to_string()
        };
        children_of.entry(parent).or_default().push(p.clone());
    }
    for v in children_of.values_mut() {
        v.sort();
    }

    let meta_by_path: std::collections::HashMap<String, bool> =
        entries.iter().map(|(_, p, d)| (p.clone(), *d)).collect();

    // Collect all entry paths and the parent-directory path each one
    // belongs to. The walker below uses these to render the tree.
    let all_paths: Vec<String> = entries.iter().map(|(_, p, _)| p.clone()).collect();

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
        children.sort();
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

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": vpath,
        "content": out,
        "entries": entries.len(),
        "_dbg_co": format!("{:?}", children_of),
        "_dbg_meta": format!("{:?}", meta_by_path),
    })))
}

// ══════════════════ private helpers ══════════════════

/// Text read with line numbers (cat -n style).
async fn read_text_numbered(
    op: &opendal::Operator,
    remote_path: &str,
    display_path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let meta = match op.stat(remote_path).await {
        Ok(m) => m,
        Err(e) if matches!(e.kind(), opendal::ErrorKind::NotFound) => {
            return Ok(AgentToolResult::error(format!(
                "read: '{display_path}': No such file or directory"
            )));
        }
        Err(e) => {
            return Ok(AgentToolResult::error(format!(
                "read: '{display_path}': {e}"
            )));
        }
    };
    if meta.is_dir() {
        return Ok(AgentToolResult::error(format!(
            "read: '{display_path}' is a directory. Use 'ls' or 'tree' to list its contents."
        )));
    }
    let total_size = meta.content_length();
    if total_size == 0 {
        return Ok(AgentToolResult::success_json(serde_json::json!({
            "path": display_path,
            "content": "(file exists but is empty)",
            "total_size": 0,
        })));
    }

    let reader = op.reader(remote_path).await.map_err(|e| e.to_string())?;
    let buf = reader
        .read(0..total_size)
        .await
        .map_err(|e| e.to_string())?;
    let data = buf.to_vec();
    if is_binary(&data) {
        return Ok(binary_refused(display_path, total_size));
    }
    let content = String::from_utf8_lossy(&data);

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    let start = offset.unwrap_or(1).saturating_sub(1).min(total_lines);
    let end = match limit {
        Some(n) => (start + n).min(total_lines),
        None => (start + DEFAULT_MAX_LINES).min(total_lines),
    };

    let width = end.to_string().len().max(3);
    let mut out = String::new();
    for (i, line) in lines[start..end].iter().enumerate() {
        let n = start + i + 1;
        out.push_str(&format!("{n:>width$}\t{line}\n"));
    }

    let truncated = end < total_lines;
    if limit.is_none() && truncated {
        let remaining = total_lines - end;
        out.push_str(&format!(
            "\n... ({remaining} more lines, use offset={} to continue)\n",
            end + 1
        ));
    }

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": display_path,
        "content": out,
        "start_line": start + 1,
        "lines_read": end - start,
        "total_lines": total_lines,
        "total_size": total_size,
        "truncated": truncated,
    })))
}

/// Image read → base64 image block.
async fn read_image(
    op: &opendal::Operator,
    remote_path: &str,
    display_path: &str,
    ext: &str,
) -> Result<AgentToolResult, ToolError> {
    let total_size = op
        .stat(remote_path)
        .await
        .map(|m| m.content_length())
        .unwrap_or(0);
    if total_size == 0 {
        return Ok(AgentToolResult::error(format!(
            "Image file is empty: {display_path}"
        )));
    }

    let reader = op.reader(remote_path).await.map_err(|e| e.to_string())?;
    let buf = reader
        .read(0..total_size)
        .await
        .map_err(|e| e.to_string())?;

    let media_type = image_media_type(ext);
    let data = base64::engine::general_purpose::STANDARD.encode(buf.to_vec());

    Ok(AgentToolResult::with_blocks(vec![ToolResultBlock::Image {
        source: ToolImageSource::Base64 {
            media_type: media_type.to_string(),
            data,
        },
    }]))
}

fn image_media_type(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Notebook (.ipynb) read → formatted text.
async fn read_notebook(
    op: &opendal::Operator,
    remote_path: &str,
    display_path: &str,
) -> Result<AgentToolResult, ToolError> {
    let total_size = op
        .stat(remote_path)
        .await
        .map(|m| m.content_length())
        .unwrap_or(0);
    if total_size == 0 {
        return Ok(AgentToolResult::error(format!(
            "Notebook file is empty: {display_path}"
        )));
    }

    let reader = op.reader(remote_path).await.map_err(|e| e.to_string())?;
    let buf = reader
        .read(0..total_size)
        .await
        .map_err(|e| e.to_string())?;
    let data = buf.to_vec();
    let raw = String::from_utf8_lossy(&data);

    let nb: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return Ok(AgentToolResult::error(format!(
                "Invalid notebook JSON in {display_path}: {e}"
            )));
        }
    };

    let cells = nb.get("cells").and_then(|c| c.as_array());
    let cell_count = cells.map_or(0, |c| c.len());

    let mut out = String::new();
    out.push_str(&format!(
        "Jupyter notebook: {display_path}\n{cell_count} cells\n\n"
    ));

    if let Some(cells) = cells {
        for (idx, cell) in cells.iter().enumerate() {
            let cell_type = cell
                .get("cell_type")
                .and_then(|t| t.as_str())
                .unwrap_or("unknown");
            out.push_str(&format!("─ Cell {} [{cell_type}] ─\n", idx + 1));

            if let Some(source) = cell.get("source") {
                let text = extract_source(source);
                out.push_str(&text);
                if !text.ends_with('\n') {
                    out.push('\n');
                }
            }

            if cell_type == "code" {
                if let Some(outputs) = cell.get("outputs").and_then(|o| o.as_array()) {
                    for output in outputs {
                        if let Some(text) = output.get("text").and_then(|t| t.as_str()) {
                            out.push_str("[output]\n");
                            out.push_str(text);
                            if !text.ends_with('\n') {
                                out.push('\n');
                            }
                        } else if let Some(data) = output
                            .get("data")
                            .and_then(|d| d.get("text/plain"))
                            .and_then(|t| t.as_str())
                        {
                            out.push_str("[output]\n");
                            out.push_str(data);
                            out.push('\n');
                        }
                    }
                }
            }
            out.push('\n');
        }
    }

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": display_path,
        "content": out,
        "cell_count": cell_count,
        "total_size": total_size,
    })))
}

/// `.ipynb` `source` can be a string or an array of strings.
fn extract_source(source: &serde_json::Value) -> String {
    if let Some(s) = source.as_str() {
        return s.to_string();
    }
    if let Some(arr) = source.as_array() {
        return arr
            .iter()
            .filter_map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

// ══════════════════ binary detection ══════════════════

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
pub(crate) fn is_binary(bytes: &[u8]) -> bool {
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
fn binary_refused(display_path: &str, size: u64) -> AgentToolResult {
    AgentToolResult::error(format!(
        "Refused to read '{display_path}': the file appears to be binary \
         ({size} bytes, non-text content detected). Reading it as text would \
         produce garbled output; use a dedicated tool to inspect binary data."
    ))
}
