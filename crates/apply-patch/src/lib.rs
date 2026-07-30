//! Patch-based file editing engine, ported from Codex's `apply_patch`.
//!
//! This crate provides a self-contained engine for parsing and applying
//! structured patches (add / delete / update / move files) with fuzzy
//! line-matching, delta tracking, and `tokio::fs` as the file system backend.
//!
//! ## Quick start
//!
//! ```ignore
//! use apply_patch::apply_patch;
//!
//! let patch = "*** Begin Patch
//! *** Update File: src/main.rs
//! @@
//! -fn old() {}
//! +fn new() {}
//! *** End Patch";
//!
//! let delta = apply_patch(patch, std::path::Path::new("/project")).await?;
//! ```

mod parser;
mod seek_sequence;

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::path::PathBuf;

use thiserror::Error;

pub use parser::Hunk;
pub use parser::ParseError;
pub use parser::UpdateFileChunk;
pub use parser::parse_patch;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ApplyPatchError {
    #[error(transparent)]
    ParseError(#[from] ParseError),
    #[error("{0}")]
    IoError(String),
    /// Error while computing replacements for an update chunk.
    #[error("{0}")]
    ComputeReplacements(String),
}

// ---------------------------------------------------------------------------
// Parsed patch data model
// ---------------------------------------------------------------------------

/// Parsed patch arguments: the raw text plus the parsed hunks.
#[derive(Debug, PartialEq)]
pub struct ApplyPatchArgs {
    pub patch: String,
    pub hunks: Vec<Hunk>,
    pub environment_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Delta tracking
// ---------------------------------------------------------------------------

/// A committed file change, preserved in the order it was applied.
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedPatchChange {
    pub path: PathBuf,
    pub change: AppliedPatchFileChange,
}

/// Textual file change committed during patch application.
#[derive(Clone, Debug, PartialEq)]
pub enum AppliedPatchFileChange {
    Add {
        content: String,
        overwritten_content: Option<String>,
    },
    Delete {
        content: String,
    },
    Update {
        move_path: Option<PathBuf>,
        old_content: String,
        new_content: String,
    },
}

/// All committed changes from a patch application, plus an `exact` flag that
/// is `false` when a partial failure may have left the filesystem in an
/// indeterminate state.
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedPatchDelta {
    changes: Vec<AppliedPatchChange>,
    exact: bool,
}

impl AppliedPatchDelta {
    fn new(changes: Vec<AppliedPatchChange>, exact: bool) -> Self {
        Self { changes, exact }
    }

    fn empty() -> Self {
        Self::new(Vec::new(), true)
    }

    pub fn changes(&self) -> &[AppliedPatchChange] {
        &self.changes
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub fn is_exact(&self) -> bool {
        self.exact
    }

    /// Append a later delta, preserving aggregate exactness.
    pub fn append(&mut self, other: Self) {
        self.changes.extend(other.changes);
        self.exact &= other.exact;
    }
}

impl Default for AppliedPatchDelta {
    fn default() -> Self {
        Self::empty()
    }
}

/// A failed patch application with the textual mutations committed before
/// the failure was observed.
#[derive(Debug, Error)]
#[error("{error}")]
pub struct ApplyPatchFailure {
    error: ApplyPatchError,
    delta: AppliedPatchDelta,
}

impl ApplyPatchFailure {
    pub fn delta(&self) -> &AppliedPatchDelta {
        &self.delta
    }

    pub fn into_parts(self) -> (ApplyPatchError, AppliedPatchDelta) {
        (self.error, self.delta)
    }
}

// ---------------------------------------------------------------------------
// Affected paths summary
// ---------------------------------------------------------------------------

pub struct AffectedPaths {
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

// ---------------------------------------------------------------------------
// Public API: apply_patch
// ---------------------------------------------------------------------------

/// Applies a patch text relative to `cwd` using `tokio::fs`.
///
/// Returns the [`AppliedPatchDelta`] on success, or an [`ApplyPatchFailure`]
/// carrying the error and any partial changes committed before the failure.
pub async fn apply_patch(patch: &str, cwd: &Path) -> Result<AppliedPatchDelta, ApplyPatchFailure> {
    let hunks = match parse_patch(patch) {
        Ok(source) => source.hunks,
        Err(e) => {
            return Err(ApplyPatchFailure::new(
                ApplyPatchError::ParseError(e),
                AppliedPatchDelta::empty(),
            ));
        }
    };

    apply_hunks(&hunks, cwd).await
}

// ---------------------------------------------------------------------------
// Public API: preview_patch (dry-run)
// ---------------------------------------------------------------------------

/// A preview of what a patch would do, without writing anything to disk.
#[derive(Debug)]
pub struct PatchPreview {
    pub changes: Vec<PreviewChange>,
}

/// One file-level preview.
#[derive(Debug)]
pub enum PreviewChange {
    /// A new file would be created (possibly overwriting an existing one).
    Add {
        path: PathBuf,
        content: String,
        overwrites: Option<String>,
    },
    /// A file would be deleted.
    Delete { path: PathBuf, content: String },
    /// A file would be modified (and optionally moved).
    Update {
        path: PathBuf,
        move_path: Option<PathBuf>,
        old_content: String,
        new_content: String,
        /// Unified diff of old → new.
        diff: String,
    },
}

impl PatchPreview {
    /// Returns a human-readable summary string (A/M/D per file).
    pub fn summary(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        for c in &self.changes {
            let (tag, path, extra) = match c {
                PreviewChange::Add { path, content, .. } => (
                    "A",
                    path.display().to_string(),
                    format!("({} bytes)", content.len()),
                ),
                PreviewChange::Delete { path, .. } => {
                    ("D", path.display().to_string(), String::new())
                }
                PreviewChange::Update {
                    path,
                    move_path,
                    diff,
                    ..
                } => {
                    let extra = if let Some(dest) = move_path {
                        format!("→ {}", dest.display())
                    } else {
                        format!("({} diff lines)", diff.lines().count())
                    };
                    ("M", path.display().to_string(), extra)
                }
            };
            let _ = writeln!(out, "{tag} {path} {extra}");
        }
        out.trim_end().to_string()
    }

    /// Returns the full unified diff for all update changes.
    pub fn full_diff(&self) -> String {
        let mut out = String::new();
        for c in &self.changes {
            if let PreviewChange::Update { diff, .. } = c {
                out.push_str(diff);
                out.push('\n');
            }
        }
        out
    }
}

/// Preview what a patch would do without writing anything to disk.
///
/// Parses the patch, reads existing files, computes the resulting content for
/// each hunk, and returns a [`PatchPreview`]. No files are created, modified,
/// or deleted.
pub async fn preview_patch(patch: &str, cwd: &Path) -> Result<PatchPreview, ApplyPatchError> {
    let hunks = parse_patch(patch)
        .map_err(ApplyPatchError::ParseError)?
        .hunks;

    if hunks.is_empty() {
        return Err(ApplyPatchError::IoError(
            "No files were modified.".to_string(),
        ));
    }

    let mut changes = Vec::with_capacity(hunks.len());

    for hunk in &hunks {
        let path = hunk.resolve_path(cwd);
        match hunk {
            Hunk::AddFile { contents, .. } => {
                let overwrites = tokio::fs::read_to_string(&path)
                    .await
                    .ok()
                    .filter(|c| !c.is_empty());
                changes.push(PreviewChange::Add {
                    path,
                    content: contents.clone(),
                    overwrites,
                });
            }
            Hunk::DeleteFile { .. } => {
                let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
                    ApplyPatchError::IoError(format!(
                        "Failed to read file to delete {}: {e}",
                        path.display()
                    ))
                })?;
                changes.push(PreviewChange::Delete { path, content });
            }
            Hunk::UpdateFile {
                move_path, chunks, ..
            } => {
                let AppliedPatchContents {
                    original_contents,
                    new_contents,
                } = derive_new_contents_from_chunks(&path, chunks).await?;

                let diff = make_unified_diff(&original_contents, &new_contents);

                let dest = move_path.as_ref().map(|dest| {
                    if dest.is_absolute() {
                        dest.clone()
                    } else {
                        cwd.join(dest)
                    }
                });

                changes.push(PreviewChange::Update {
                    path,
                    move_path: dest,
                    old_content: original_contents,
                    new_content: new_contents,
                    diff,
                });
            }
        }
    }

    Ok(PatchPreview { changes })
}

/// Generate a minimal unified diff between two strings.
fn make_unified_diff(old: &str, new: &str) -> String {
    use similar::TextDiff;
    TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .to_string()
}

/// Applies pre-parsed hunks relative to `cwd`.
pub async fn apply_hunks(
    hunks: &[Hunk],
    cwd: &Path,
) -> Result<AppliedPatchDelta, ApplyPatchFailure> {
    let mut delta = AppliedPatchDelta::empty();
    match apply_hunks_to_files(hunks, cwd, &mut delta).await {
        Ok(affected) => {
            print_summary(&affected);
            Ok(delta)
        }
        Err(error) => {
            let apply_err = if let Some(io_err) = error.downcast_ref::<io::Error>() {
                ApplyPatchError::IoError(io_err.to_string())
            } else {
                ApplyPatchError::IoError(error.to_string())
            };
            Err(ApplyPatchFailure::new(apply_err, delta))
        }
    }
}

impl ApplyPatchFailure {
    fn new(error: ApplyPatchError, delta: AppliedPatchDelta) -> Self {
        Self { error, delta }
    }
}

// ---------------------------------------------------------------------------
// Core application logic
// ---------------------------------------------------------------------------

async fn apply_hunks_to_files(
    hunks: &[Hunk],
    cwd: &Path,
    delta: &mut AppliedPatchDelta,
) -> anyhow::Result<AffectedPaths> {
    if hunks.is_empty() {
        anyhow::bail!("No files were modified.");
    }

    let mut added: Vec<PathBuf> = Vec::new();
    let mut modified: Vec<PathBuf> = Vec::new();
    let mut deleted: Vec<PathBuf> = Vec::new();

    // A failed write can still have modified the target before surfacing an
    // error (e.g. truncating before ENOSPC), so the delta is no longer exact.
    macro_rules! try_write {
        ($result:expr) => {
            match $result {
                Ok(value) => value,
                Err(error) => {
                    delta.exact = false;
                    return Err(anyhow::Error::from(error));
                }
            }
        };
    }

    for hunk in hunks {
        let affected_path = hunk.path().to_path_buf();
        let path = hunk.resolve_path(cwd);
        match hunk {
            Hunk::AddFile { contents, .. } => {
                let overwritten_content = read_optional_file_text(&path, &mut delta.exact).await;
                try_write!(
                    write_file_with_missing_parent_retry(&path, contents.clone().into_bytes())
                        .await
                );
                delta.changes.push(AppliedPatchChange {
                    path: path.clone(),
                    change: AppliedPatchFileChange::Add {
                        content: contents.clone(),
                        overwritten_content,
                    },
                });
                added.push(affected_path);
            }
            Hunk::DeleteFile { .. } => {
                let deleted_content = tokio::fs::read_to_string(&path).await.ok();
                if deleted_content.is_none() {
                    // File may not exist — check if it's a directory.
                    if let Ok(meta) = tokio::fs::metadata(&path).await {
                        if meta.is_dir() {
                            anyhow::bail!(
                                "Failed to delete file {}: path is a directory",
                                path.display()
                            );
                        }
                    }
                    delta.exact = false;
                }
                match tokio::fs::remove_file(&path).await {
                    Ok(()) => {}
                    Err(e) => {
                        // If the file content matches what we expected, the
                        // failure was side-effect-free.
                        if let Some(expected) = &deleted_content {
                            if let Ok(current) = tokio::fs::read_to_string(&path).await {
                                if &current == expected {
                                    // Side-effect-free, delta stays exact.
                                } else {
                                    delta.exact = false;
                                }
                            } else {
                                delta.exact = false;
                            }
                        } else {
                            delta.exact = false;
                        }
                        return Err(anyhow::Error::from(ApplyPatchError::IoError(format!(
                            "Failed to delete file {}: {e}",
                            path.display()
                        ))));
                    }
                }
                if let Some(content) = deleted_content {
                    delta.changes.push(AppliedPatchChange {
                        path: path.clone(),
                        change: AppliedPatchFileChange::Delete { content },
                    });
                }
                deleted.push(affected_path);
            }
            Hunk::UpdateFile {
                move_path, chunks, ..
            } => {
                let AppliedPatchContents {
                    original_contents,
                    new_contents,
                } = derive_new_contents_from_chunks(&path, chunks).await?;

                if let Some(dest) = move_path {
                    let dest_path = if dest.is_absolute() {
                        dest.clone()
                    } else {
                        cwd.join(dest)
                    };
                    try_write!(
                        write_file_with_missing_parent_retry(
                            &dest_path,
                            new_contents.clone().into_bytes()
                        )
                        .await
                    );
                    delta.changes.push(AppliedPatchChange {
                        path: dest_path.clone(),
                        change: AppliedPatchFileChange::Update {
                            move_path: Some(dest_path.clone()),
                            old_content: original_contents.clone(),
                            new_content: new_contents.clone(),
                        },
                    });
                    // Remove original.
                    if let Err(e) = tokio::fs::remove_file(&path).await {
                        // Destination was already written.
                        if let Ok(current) = tokio::fs::read_to_string(&path).await {
                            if current != original_contents {
                                delta.exact = false;
                            }
                        } else {
                            delta.exact = false;
                        }
                        return Err(anyhow::Error::from(ApplyPatchError::IoError(format!(
                            "Failed to remove original {}: {e}",
                            path.display()
                        ))));
                    }
                    modified.push(affected_path);
                } else {
                    try_write!(
                        tokio::fs::write(&path, new_contents.clone().into_bytes())
                            .await
                            .map_err(|e| ApplyPatchError::IoError(format!(
                                "Failed to write file {}: {e}",
                                path.display()
                            )))
                    );
                    delta.changes.push(AppliedPatchChange {
                        path: path.clone(),
                        change: AppliedPatchFileChange::Update {
                            move_path: None,
                            old_content: original_contents,
                            new_content: new_contents,
                        },
                    });
                    modified.push(affected_path);
                }
            }
        }
    }
    Ok(AffectedPaths {
        added,
        modified,
        deleted,
    })
}

async fn read_optional_file_text(path: &Path, exact: &mut bool) -> Option<String> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => Some(content),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(_) => {
            *exact = false;
            None
        }
    }
}

/// Write a file, retrying after creating parent directories if the initial
/// write fails with NotFound.
async fn write_file_with_missing_parent_retry(
    path: &Path,
    contents: Vec<u8>,
) -> Result<(), ApplyPatchError> {
    match tokio::fs::write(path, &contents).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    ApplyPatchError::IoError(format!(
                        "Failed to create parent directories for {}: {e}",
                        path.display()
                    ))
                })?;
            }
            tokio::fs::write(path, &contents).await.map_err(|e| {
                ApplyPatchError::IoError(format!("Failed to write file {}: {e}", path.display()))
            })?;
            Ok(())
        }
        Err(e) => Err(ApplyPatchError::IoError(format!(
            "Failed to write file {}: {e}",
            path.display()
        ))),
    }
}

struct AppliedPatchContents {
    original_contents: String,
    new_contents: String,
}

/// Read the file at `path`, apply `chunks` to derive the new content.
async fn derive_new_contents_from_chunks(
    path: &Path,
    chunks: &[UpdateFileChunk],
) -> Result<AppliedPatchContents, ApplyPatchError> {
    let original_contents = tokio::fs::read_to_string(path).await.map_err(|e| {
        ApplyPatchError::IoError(format!(
            "Failed to read file to update {}: {e}",
            path.display()
        ))
    })?;

    let mut original_lines: Vec<String> = original_contents.split('\n').map(String::from).collect();

    // Drop the trailing empty element from the final newline so line counts
    // match standard `diff` behaviour.
    if original_lines.last().is_some_and(String::is_empty) {
        original_lines.pop();
    }

    let path_text = path.display().to_string();
    let replacements = compute_replacements(&original_lines, &path_text, chunks)?;
    let new_lines = apply_replacements(original_lines, &replacements);
    let mut new_lines = new_lines;
    if !new_lines.last().is_some_and(String::is_empty) {
        new_lines.push(String::new());
    }
    let new_contents = new_lines.join("\n");
    Ok(AppliedPatchContents {
        original_contents,
        new_contents,
    })
}

/// Compute `(start_index, old_len, new_lines)` replacements from the chunk
/// definitions, using [`seek_sequence`] to locate each chunk in the file.
fn compute_replacements(
    original_lines: &[String],
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<Vec<(usize, usize, Vec<String>)>, ApplyPatchError> {
    let mut replacements: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut line_index: usize = 0;

    for chunk in chunks {
        // Use the change_context line to narrow the search position.
        if let Some(ctx_line) = &chunk.change_context {
            if let Some(idx) = seek_sequence::seek_sequence(
                original_lines,
                std::slice::from_ref(ctx_line),
                line_index,
                false,
            ) {
                line_index = idx + 1;
            } else {
                return Err(ApplyPatchError::ComputeReplacements(format!(
                    "Failed to find context '{ctx_line}' in {path}"
                )));
            }
        }

        if chunk.old_lines.is_empty() {
            // Pure addition.
            let insertion_idx = if original_lines.last().is_some_and(String::is_empty) {
                original_lines.len() - 1
            } else {
                original_lines.len()
            };
            replacements.push((insertion_idx, 0, chunk.new_lines.clone()));
            continue;
        }

        let mut pattern: &[String] = &chunk.old_lines;
        let mut found =
            seek_sequence::seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);

        let mut new_slice: &[String] = &chunk.new_lines;

        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            // Retry without the trailing empty line representing the final
            // newline in the file.
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }
            found = seek_sequence::seek_sequence(
                original_lines,
                pattern,
                line_index,
                chunk.is_end_of_file,
            );
        }

        if let Some(start_idx) = found {
            replacements.push((start_idx, pattern.len(), new_slice.to_vec()));
            line_index = start_idx + pattern.len();
        } else {
            return Err(ApplyPatchError::ComputeReplacements(format!(
                "Failed to find expected lines in {}:\n{}",
                path,
                chunk.old_lines.join("\n"),
            )));
        }
    }

    replacements.sort_by_key(|(index, _, _)| *index);
    Ok(replacements)
}

/// Apply `(start_index, old_len, new_lines)` replacements to `lines`.
/// Replacements are applied in descending order so earlier replacements
/// don't shift positions of later ones.
fn apply_replacements(
    mut lines: Vec<String>,
    replacements: &[(usize, usize, Vec<String>)],
) -> Vec<String> {
    for (start_idx, old_len, new_segment) in replacements.iter().rev() {
        let start_idx = *start_idx;
        let old_len = *old_len;

        for _ in 0..old_len {
            if start_idx < lines.len() {
                lines.remove(start_idx);
            }
        }

        for (offset, new_line) in new_segment.iter().enumerate() {
            lines.insert(start_idx + offset, new_line.clone());
        }
    }

    lines
}

/// Print a git-style summary of changes to stdout.
pub fn print_summary(affected: &AffectedPaths) {
    println!("Success. Updated the following files:");
    for path in &affected.added {
        println!("A {}", path.display());
    }
    for path in &affected.modified {
        println!("M {}", path.display());
    }
    for path in &affected.deleted {
        println!("D {}", path.display());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::fs;
    use tempfile::tempdir;

    fn wrap_patch(body: &str) -> String {
        format!("*** Begin Patch\n{body}\n*** End Patch")
    }

    #[tokio::test]
    async fn test_add_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("add.txt");
        let patch = wrap_patch(&format!("*** Add File: {}\n+ab\n+cd", path.display()));

        apply_patch(&patch, dir.path()).await.unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "ab\ncd\n");
    }

    #[tokio::test]
    async fn test_delete_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("del.txt");
        fs::write(&path, "x").unwrap();
        let patch = wrap_patch(&format!("*** Delete File: {}", path.display()));

        apply_patch(&patch, dir.path()).await.unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn test_update_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("update.txt");
        fs::write(&path, "foo\nbar\n").unwrap();
        let patch = wrap_patch(&format!(
            "*** Update File: {}\n@@\n foo\n-bar\n+baz",
            path.display()
        ));

        apply_patch(&patch, dir.path()).await.unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "foo\nbaz\n");
    }

    #[tokio::test]
    async fn test_update_file_move() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dest = dir.path().join("dst.txt");
        fs::write(&src, "line\n").unwrap();
        let patch = wrap_patch(&format!(
            "*** Update File: {}\n*** Move to: {}\n@@\n-line\n+line2",
            src.display(),
            dest.display()
        ));

        apply_patch(&patch, dir.path()).await.unwrap();

        assert!(!src.exists());
        let contents = fs::read_to_string(&dest).unwrap();
        assert_eq!(contents, "line2\n");
    }

    #[tokio::test]
    async fn test_multiple_update_chunks_single_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("multi.txt");
        fs::write(&path, "foo\nbar\nbaz\nqux\n").unwrap();
        let patch = wrap_patch(&format!(
            "*** Update File: {}\n@@\n foo\n-bar\n+BAR\n@@\n baz\n-qux\n+QUX",
            path.display()
        ));

        apply_patch(&patch, dir.path()).await.unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "foo\nBAR\nbaz\nQUX\n");
    }

    #[tokio::test]
    async fn test_update_with_context_anchor() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("code.rs");
        fs::write(&path, "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
        // Use "fn b() {}" as context to target the second function.
        let patch = wrap_patch(&format!(
            "*** Update File: {}\n@@ fn b() {{}}\n-fn c() {{}}\n+fn d() {{}}",
            path.display()
        ));

        apply_patch(&patch, dir.path()).await.unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "fn a() {}\nfn b() {}\nfn d() {}\n");
    }

    #[tokio::test]
    async fn test_pure_addition_at_eof() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("append.txt");
        fs::write(&path, "a\nb\nc\n").unwrap();
        let patch = wrap_patch(&format!(
            "*** Update File: {}\n@@\n+a\n*** End of File",
            path.display()
        ));

        apply_patch(&patch, dir.path()).await.unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "a\nb\nc\na\n");
    }

    #[tokio::test]
    async fn test_unicode_dash_matching() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("unicode.py");
        let original = "import asyncio  # local import \u{2013} avoids dep\n";
        fs::write(&path, original).unwrap();

        // Patch uses plain ASCII dash.
        let patch = wrap_patch(&format!(
            "*** Update File: {}\n@@\n-import asyncio  # local import - avoids dep\n+import asyncio  # HELLO",
            path.display()
        ));

        apply_patch(&patch, dir.path()).await.unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "import asyncio  # HELLO\n");
    }

    #[tokio::test]
    async fn test_add_file_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested/deep/file.txt");
        let patch = wrap_patch(&format!("*** Add File: {}\n+hello", path.display()));

        apply_patch(&patch, dir.path()).await.unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "hello\n");
    }

    #[tokio::test]
    async fn test_delta_tracking_on_success() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("track.txt");
        fs::write(&path, "old\n").unwrap();
        let patch = wrap_patch(&format!(
            "*** Update File: {}\n@@\n-old\n+new",
            path.display()
        ));

        let delta = apply_patch(&patch, dir.path()).await.unwrap();
        assert!(delta.is_exact());
        assert_eq!(delta.changes().len(), 1);
        match &delta.changes()[0].change {
            AppliedPatchFileChange::Update {
                old_content,
                new_content,
                ..
            } => {
                assert_eq!(old_content, "old\n");
                assert_eq!(new_content, "new\n");
            }
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_empty_patch_is_error() {
        let dir = tempdir().unwrap();
        let patch = "*** Begin Patch\n*** End Patch";
        let result = apply_patch(patch, dir.path()).await;
        assert!(result.is_err());
    }

    // ----- dry-run preview tests -----

    #[tokio::test]
    async fn test_preview_add_file_no_write() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("new.txt");
        let patch = wrap_patch(&format!("*** Add File: {}\n+hello", path.display()));

        let preview = preview_patch(&patch, dir.path()).await.unwrap();
        assert_eq!(preview.changes.len(), 1);
        match &preview.changes[0] {
            PreviewChange::Add {
                content,
                overwrites,
                ..
            } => {
                assert_eq!(content, "hello\n");
                assert!(overwrites.is_none());
            }
            other => panic!("expected Add, got {other:?}"),
        }
        // File must NOT exist after preview.
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn test_preview_update_shows_diff() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("code.rs");
        fs::write(&path, "fn old() {}\n").unwrap();
        let patch = wrap_patch(&format!(
            "*** Update File: {}\n@@\n-fn old() {{}}\n+fn new() {{}}",
            path.display()
        ));

        let preview = preview_patch(&patch, dir.path()).await.unwrap();
        match &preview.changes[0] {
            PreviewChange::Update {
                diff,
                old_content,
                new_content,
                ..
            } => {
                assert!(diff.contains("-fn old() {}"));
                assert!(diff.contains("+fn new() {}"));
                assert_eq!(old_content, "fn old() {}\n");
                assert_eq!(new_content, "fn new() {}\n");
            }
            other => panic!("expected Update, got {other:?}"),
        }
        // File must be unchanged.
        assert_eq!(fs::read_to_string(&path).unwrap(), "fn old() {}\n");
    }

    #[tokio::test]
    async fn test_preview_delete_shows_content() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("del.txt");
        fs::write(&path, "bye bye\n").unwrap();
        let patch = wrap_patch(&format!("*** Delete File: {}", path.display()));

        let preview = preview_patch(&patch, dir.path()).await.unwrap();
        match &preview.changes[0] {
            PreviewChange::Delete { content, .. } => {
                assert_eq!(content, "bye bye\n");
            }
            other => panic!("expected Delete, got {other:?}"),
        }
        // File must still exist.
        assert!(path.exists());
    }

    #[tokio::test]
    async fn test_preview_move_shows_dest() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dest = dir.path().join("dst.txt");
        fs::write(&src, "line\n").unwrap();
        let patch = wrap_patch(&format!(
            "*** Update File: {}\n*** Move to: {}\n@@\n-line\n+line2",
            src.display(),
            dest.display()
        ));

        let preview = preview_patch(&patch, dir.path()).await.unwrap();
        match &preview.changes[0] {
            PreviewChange::Update {
                move_path,
                new_content,
                ..
            } => {
                assert_eq!(move_path.as_ref().unwrap(), &dest);
                assert_eq!(new_content, "line2\n");
            }
            other => panic!("expected Update, got {other:?}"),
        }
        // Neither file should have changed.
        assert!(src.exists());
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn test_preview_overwrite_detection() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("existing.txt");
        fs::write(&path, "old content\n").unwrap();
        let patch = wrap_patch(&format!("*** Add File: {}\n+new content", path.display()));

        let preview = preview_patch(&patch, dir.path()).await.unwrap();
        match &preview.changes[0] {
            PreviewChange::Add { overwrites, .. } => {
                assert_eq!(overwrites.as_deref(), Some("old content\n"));
            }
            other => panic!("expected Add, got {other:?}"),
        }
        // File must be unchanged.
        assert_eq!(fs::read_to_string(&path).unwrap(), "old content\n");
    }

    #[tokio::test]
    async fn test_preview_summary_and_full_diff() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("f.txt");
        fs::write(&path, "a\nb\nc\n").unwrap();
        let patch = wrap_patch(&format!("*** Update File: {}\n@@\n-b\n+B", path.display()));

        let preview = preview_patch(&patch, dir.path()).await.unwrap();
        let summary = preview.summary();
        assert!(summary.starts_with("M "));
        assert!(summary.contains("f.txt"));

        let diff = preview.full_diff();
        assert!(diff.contains("-b"));
        assert!(diff.contains("+B"));
    }
}
