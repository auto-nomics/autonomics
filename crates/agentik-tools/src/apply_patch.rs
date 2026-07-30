//! `apply_patch` tool — structured multi-file patch editing.
//!
//! Wraps the [`apply_patch`] crate's engine in a [`ToolFunction`] so agents
//! can create, update, move, and delete files in a single atomic call using
//! Codex's patch format.
//!
//! ## Patch format
//!
//! ```text
//! *** Begin Patch
//! *** Add File: path/to/new.txt
//! +line 1
//! +line 2
//! *** Update File: path/to/existing.rs
//! @@ fn function_name(
//! -    old line
//! +    new line
//!  context line
//! *** Delete File: path/to/old.txt
//! *** End Patch
//! ```
//!
//! See the [`apply_patch`] crate docs for full format details and the fuzzy
//! matching strategy.

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use apply_patch::AppliedPatchFileChange;
use async_trait::async_trait;

#[tool(
    name = "apply_patch",
    description = "Apply a structured patch to create, update, move, or delete files. \
                   Supports multi-file edits in a single call with fuzzy line matching. \
                   The patch uses a custom format with *** markers (NOT unified diff). \
                   Format: *** Begin Patch / *** Add File: <path> / *** Delete File: <path> / \
                   *** Update File: <path> / @@ <context> / +/-/ lines / *** End Patch. \
                   Set dry_run=true to preview changes without writing."
)]
pub struct ApplyPatchInput {
    #[desc = "The patch text in apply_patch format (*** Begin Patch ... *** End Patch)"]
    pub patch: String,

    #[desc = "If true, preview what would change without writing anything. Returns unified diffs."]
    pub dry_run: Option<bool>,
}

pub struct ApplyPatchTool;

#[async_trait]
impl ToolFunction for ApplyPatchTool {
    type Input = ApplyPatchInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let cwd = std::env::current_dir().map_err(|e| ToolError::ExecutionFailed {
            source: Box::new(e),
        })?;

        if input.dry_run.unwrap_or(false) {
            return run_dry(&input.patch, &cwd).await;
        }

        run_apply(&input.patch, &cwd).await
    }
}

async fn run_apply(patch: &str, cwd: &std::path::Path) -> Result<ToolResult, ToolError> {
    match apply_patch::apply_patch(patch, cwd).await {
        Ok(delta) => {
            let summary = format_delta_summary(&delta);
            Ok(ToolResult::success_json(serde_json::json!({
                "success": true,
                "exact": delta.is_exact(),
                "changes": delta.changes().iter().map(|c| {
                    serde_json::json!({
                        "path": c.path.display().to_string(),
                        "change": change_to_json(&c.change),
                    })
                }).collect::<Vec<_>>(),
                "summary": summary,
            })))
        }
        Err(failure) => {
            let (error, delta) = failure.into_parts();
            let committed = if delta.is_empty() {
                String::new()
            } else {
                format!(
                    "\n\nPartial changes committed before failure:\n{}",
                    format_delta_summary(&delta)
                )
            };
            Ok(ToolResult::error(format!(
                "apply_patch failed: {error}{committed}"
            )))
        }
    }
}

async fn run_dry(patch: &str, cwd: &std::path::Path) -> Result<ToolResult, ToolError> {
    match apply_patch::preview_patch(patch, cwd).await {
        Ok(preview) => {
            let changes: Vec<serde_json::Value> = preview
                .changes
                .iter()
                .map(|c| match c {
                    apply_patch::PreviewChange::Add {
                        path,
                        content,
                        overwrites,
                    } => serde_json::json!({
                        "type": "add",
                        "path": path.display().to_string(),
                        "size": content.len(),
                        "overwrites_existing": overwrites.is_some(),
                    }),
                    apply_patch::PreviewChange::Delete { path, content } => serde_json::json!({
                        "type": "delete",
                        "path": path.display().to_string(),
                        "size": content.len(),
                    }),
                    apply_patch::PreviewChange::Update {
                        path,
                        move_path,
                        diff,
                        old_content,
                        new_content,
                    } => serde_json::json!({
                        "type": "update",
                        "path": path.display().to_string(),
                        "move_path": move_path.as_ref().map(|p| p.display().to_string()),
                        "old_size": old_content.len(),
                        "new_size": new_content.len(),
                        "diff": diff,
                    }),
                })
                .collect();

            Ok(ToolResult::success_json(serde_json::json!({
                "dry_run": true,
                "summary": preview.summary(),
                "diff": preview.full_diff(),
                "changes": changes,
            })))
        }
        Err(e) => Ok(ToolResult::error(format!(
            "apply_patch preview failed: {e}"
        ))),
    }
}

fn format_delta_summary(delta: &apply_patch::AppliedPatchDelta) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for change in delta.changes() {
        let (op, detail) = match &change.change {
            AppliedPatchFileChange::Add { content, .. } => {
                ("A", format!("({} bytes)", content.len()))
            }
            AppliedPatchFileChange::Delete { .. } => ("D", String::new()),
            AppliedPatchFileChange::Update { move_path, .. } => {
                if let Some(dest) = move_path {
                    ("M", format!("→ {}", dest.display()))
                } else {
                    ("M", String::new())
                }
            }
        };
        let _ = writeln!(out, "{op} {} {detail}", change.path.display());
    }
    out.trim_end().to_string()
}

fn change_to_json(change: &AppliedPatchFileChange) -> serde_json::Value {
    match change {
        AppliedPatchFileChange::Add {
            content,
            overwritten_content,
        } => serde_json::json!({
            "type": "add",
            "size": content.len(),
            "overwrote_existing": overwritten_content.is_some(),
        }),
        AppliedPatchFileChange::Delete { content } => serde_json::json!({
            "type": "delete",
            "size": content.len(),
        }),
        AppliedPatchFileChange::Update {
            move_path,
            old_content,
            new_content,
        } => serde_json::json!({
            "type": "update",
            "move_path": move_path.as_ref().map(|p| p.display().to_string()),
            "old_size": old_content.len(),
            "new_size": new_content.len(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apply_patch::parse_patch;

    #[test]
    fn test_parse_patch_from_tool_input() {
        let patch_text = "*** Begin Patch
*** Add File: test.txt
+hello
*** End Patch";
        let parsed = parse_patch(patch_text).unwrap();
        assert_eq!(parsed.hunks.len(), 1);
    }

    #[tokio::test]
    async fn test_apply_patch_tool_add_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("new.txt");

        let patch = format!(
            "*** Begin Patch\n*** Add File: {}\n+hello world\n*** End Patch",
            file_path.display()
        );

        let tool = ApplyPatchTool;
        let result = tool
            .run(ApplyPatchInput {
                patch,
                dry_run: None,
            })
            .await
            .unwrap();

        assert!(result.is_error.is_none());
        assert!(file_path.exists());
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "hello world\n"
        );
    }

    #[tokio::test]
    async fn test_apply_patch_tool_update_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("code.rs");
        std::fs::write(&file_path, "fn old() {}\n").unwrap();

        let patch = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n-fn old() {{}}\n+fn new() {{}}\n*** End Patch",
            file_path.display()
        );

        let tool = ApplyPatchTool;
        let result = tool
            .run(ApplyPatchInput {
                patch,
                dry_run: None,
            })
            .await
            .unwrap();

        assert!(result.is_error.is_none());
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "fn new() {}\n"
        );
    }

    #[tokio::test]
    async fn test_apply_patch_tool_invalid_patch() {
        let tool = ApplyPatchTool;
        let result = tool
            .run(ApplyPatchInput {
                patch: "not a valid patch".to_string(),
                dry_run: None,
            })
            .await
            .unwrap();

        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn test_apply_patch_tool_dry_run_no_write() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("code.rs");
        std::fs::write(&file_path, "fn old() {}\n").unwrap();

        let patch = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n-fn old() {{}}\n+fn new() {{}}\n*** End Patch",
            file_path.display()
        );

        let tool = ApplyPatchTool;
        let result = tool
            .run(ApplyPatchInput {
                patch,
                dry_run: Some(true),
            })
            .await
            .unwrap();

        assert!(result.is_error.is_none());
        // File must be unchanged.
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "fn old() {}\n"
        );
    }
}
