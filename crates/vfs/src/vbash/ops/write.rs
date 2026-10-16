//! Writing and content-mutation operations: `write`, `edit`, `patch`, `touch`.

use agentik_core::tools::{ToolError, ToolResult};
use agentik_sdk::types::ToolResult as AgentToolResult;

use super::common::require_path;
use crate::storage::OpendalFileStorage;

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
    if let Err(error) = storage.check_readable(&vpath) {
        return Ok(AgentToolResult::error(format!(
            "edit: '{raw_path}': {error}"
        )));
    }
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
            if let Err(error) = storage.check_writable(&vpath) {
                return Ok(AgentToolResult::error(format!(
                    "edit: '{raw_path}': {error}"
                )));
            }
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
