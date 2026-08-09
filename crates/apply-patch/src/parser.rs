//! Patch format parser — single-pass line-based parser.
//!
//! Defines the patch text format and the [`Hunk`] / [`UpdateFileChunk`] data
//! model, then parses a complete patch text into hunks.

use std::path::Path;
use std::path::PathBuf;

use thiserror::Error;

use crate::ApplyPatchArgs;

pub(crate) const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
pub(crate) const END_PATCH_MARKER: &str = "*** End Patch";
pub(crate) const ADD_FILE_MARKER: &str = "*** Add File: ";
pub(crate) const DELETE_FILE_MARKER: &str = "*** Delete File: ";
pub(crate) const UPDATE_FILE_MARKER: &str = "*** Update File: ";
pub(crate) const MOVE_TO_MARKER: &str = "*** Move to: ";
pub(crate) const EOF_MARKER: &str = "*** End of File";
pub(crate) const CHANGE_CONTEXT_MARKER: &str = "@@ ";
pub(crate) const EMPTY_CHANGE_CONTEXT_MARKER: &str = "@@";

/// GPT-4.1 sometimes wraps the patch in a heredoc. Lenient mode strips the
/// heredoc wrapper. We default to lenient for all callers.
const PARSE_IN_STRICT_MODE: bool = false;

#[derive(Debug, PartialEq, Error, Clone)]
pub enum ParseError {
    #[error("invalid patch: {0}")]
    InvalidPatchError(String),
    #[error("invalid hunk at line {line_number}, {message}")]
    InvalidHunkError { message: String, line_number: usize },
}

/// One parsed hunk: an add, delete, or update operation on a single file.
#[derive(Debug, PartialEq, Clone)]
pub enum Hunk {
    AddFile {
        path: PathBuf,
        contents: String,
    },
    DeleteFile {
        path: PathBuf,
    },
    UpdateFile {
        path: PathBuf,
        move_path: Option<PathBuf>,
        chunks: Vec<UpdateFileChunk>,
    },
}

impl Hunk {
    /// Resolve the hunk's path relative to `cwd`, returning an absolute path.
    pub fn resolve_path(&self, cwd: &Path) -> PathBuf {
        let path = match self {
            Hunk::UpdateFile { path, .. } => path,
            Hunk::AddFile { .. } | Hunk::DeleteFile { .. } => self.path(),
        };
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        }
    }

    /// Returns the path affected by this hunk, using the move destination for
    /// rename hunks.
    pub fn path(&self) -> &Path {
        match self {
            Hunk::AddFile { path, .. } => path,
            Hunk::DeleteFile { path } => path,
            Hunk::UpdateFile {
                move_path: Some(path),
                ..
            } => path,
            Hunk::UpdateFile {
                path,
                move_path: None,
                ..
            } => path,
        }
    }
}

/// A contiguous change region within an update hunk.
#[derive(Debug, PartialEq, Clone, Default)]
pub struct UpdateFileChunk {
    /// A single line of context (e.g. a function signature) used to narrow
    /// down the position of the chunk in the file.
    pub change_context: Option<String>,
    /// The contiguous block of lines to be replaced with [`new_lines`].
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
    /// If true, `old_lines` must occur at the end of the source file.
    pub is_end_of_file: bool,
}

const ENVIRONMENT_ID_MARKER: &str = "*** Environment ID:";

#[derive(Debug, Default, Clone, Copy)]
enum Mode {
    #[default]
    NotStarted,
    StartedPatch,
    AddFile,
    DeleteFile,
    UpdateFile,
    EndedPatch,
}

/// Parser state carried across lines.
#[derive(Debug, Default)]
struct Parser {
    mode: Mode,
    hunks: Vec<Hunk>,
    environment_id: Option<String>,
    /// Line number where the current `*** Update File` header appeared.
    update_hunk_line_no: usize,
}

/// Parse a complete patch text into hunks.
pub fn parse_patch(patch: &str) -> Result<ApplyPatchArgs, ParseError> {
    let mode = if PARSE_IN_STRICT_MODE {
        ParseMode::Strict
    } else {
        ParseMode::Lenient
    };
    parse_patch_text(patch, mode)
}

enum ParseMode {
    Strict,
    Lenient,
}

fn parse_patch_text(patch: &str, mode: ParseMode) -> Result<ApplyPatchArgs, ParseError> {
    let lines: Vec<&str> = patch.trim().lines().collect();
    let body = match mode {
        ParseMode::Strict => &lines[..],
        ParseMode::Lenient => strip_heredoc(&lines)?,
    };

    // Validate boundaries.
    let (first, last) = match body {
        [] => (None, None),
        [first] => (Some(*first), Some(*first)),
        [first, .., last] => (Some(*first), Some(*last)),
    };
    check_boundaries(first, last)?;

    let patch_text = body.join("\n");
    let mut parser = Parser::default();

    for (i, raw_line) in body.iter().enumerate() {
        let line_no = i + 1;
        // Strip trailing \r for CRLF tolerance.
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        parser.process_line(line, line_no)?;
    }

    if !matches!(parser.mode, Mode::EndedPatch) {
        return Err(ParseError::InvalidPatchError(
            "The last line of the patch must be '*** End Patch'".to_string(),
        ));
    }

    Ok(ApplyPatchArgs {
        patch: patch_text,
        hunks: parser.hunks,
        environment_id: parser.environment_id,
    })
}

/// In lenient mode, strip `<<'EOF'...EOF` heredoc wrappers that some models emit.
fn strip_heredoc<'a>(lines: &'a [&'a str]) -> Result<&'a [&'a str], ParseError> {
    // First try strict boundaries.
    if let Ok(()) = {
        let (first, last) = boundary_pair(lines);
        check_boundaries(first, last)
    } {
        return Ok(lines);
    }

    match lines {
        [first, .., last]
            if (first == &"<<EOF" || first == &"<<'EOF'" || first == &"<<\"EOF\"")
                && last.ends_with("EOF")
                && lines.len() >= 4 =>
        {
            let inner = &lines[1..lines.len() - 1];
            let (first, last) = boundary_pair(inner);
            check_boundaries(first, last)?;
            Ok(inner)
        }
        _ => {
            let (first, last) = boundary_pair(lines);
            check_boundaries(first, last)?;
            Ok(lines)
        }
    }
}

fn boundary_pair<'a>(lines: &[&'a str]) -> (Option<&'a str>, Option<&'a str>) {
    match lines {
        [] => (None, None),
        [first] => (Some(first), Some(first)),
        [first, .., last] => (Some(first), Some(last)),
    }
}

fn check_boundaries(first: Option<&str>, last: Option<&str>) -> Result<(), ParseError> {
    let first = first.map(str::trim);
    let last = last.map(str::trim);
    match (first, last) {
        (Some(f), Some(l)) if f == BEGIN_PATCH_MARKER && l == END_PATCH_MARKER => Ok(()),
        (Some(f), _) if f != BEGIN_PATCH_MARKER => Err(ParseError::InvalidPatchError(
            "The first line of the patch must be '*** Begin Patch'".into(),
        )),
        _ => Err(ParseError::InvalidPatchError(
            "The last line of the patch must be '*** End Patch'".into(),
        )),
    }
}

impl Parser {
    fn process_line(&mut self, line: &str, line_no: usize) -> Result<(), ParseError> {
        let trimmed = line.trim();
        match self.mode {
            Mode::NotStarted => {
                if trimmed == BEGIN_PATCH_MARKER {
                    self.mode = Mode::StartedPatch;
                    Ok(())
                } else {
                    Err(ParseError::InvalidPatchError(
                        "The first line of the patch must be '*** Begin Patch'".into(),
                    ))
                }
            }
            Mode::StartedPatch => {
                if self.try_hunk_header(trimmed, line_no)? {
                    return Ok(());
                }
                Err(self.invalid_hunk_header(trimmed, line_no))
            }
            Mode::AddFile => {
                if self.try_hunk_header(trimmed, line_no)? {
                    return Ok(());
                }
                if let Some(text) = line.strip_prefix('+') {
                    if let Some(Hunk::AddFile { contents, .. }) = self.hunks.last_mut() {
                        contents.push_str(text);
                        contents.push('\n');
                        return Ok(());
                    }
                }
                Err(self.invalid_hunk_header(trimmed, line_no))
            }
            Mode::DeleteFile => {
                if self.try_hunk_header(trimmed, line_no)? {
                    return Ok(());
                }
                Err(self.invalid_hunk_header(trimmed, line_no))
            }
            Mode::UpdateFile => {
                let update_line = line.trim_end();
                if self.try_hunk_header(update_line, line_no)? {
                    return Ok(());
                }
                self.process_update_body_line(line, update_line, line_no)
            }
            Mode::EndedPatch => {
                if trimmed.is_empty() {
                    Ok(())
                } else {
                    Err(ParseError::InvalidPatchError(
                        "The last line of the patch must be '*** End Patch'".into(),
                    ))
                }
            }
        }
    }

    fn invalid_hunk_header(&self, trimmed: &str, line_no: usize) -> ParseError {
        ParseError::InvalidHunkError {
            message: format!(
                "'{trimmed}' is not a valid hunk header. Valid hunk headers: \
                 '*** Add File: {{path}}', '*** Delete File: {{path}}', '*** Update File: {{path}}'"
            ),
            line_number: line_no,
        }
    }

    /// Try to match a hunk header or *** End Patch. Returns `Ok(true)` if the
    /// line was consumed as a header/boundary.
    fn try_hunk_header(&mut self, trimmed: &str, line_no: usize) -> Result<bool, ParseError> {
        // Environment ID (only valid in StartedPatch mode).
        if matches!(self.mode, Mode::StartedPatch)
            && let Some(env_id) = trimmed.strip_prefix(ENVIRONMENT_ID_MARKER)
        {
            if self.environment_id.is_some() {
                return Err(ParseError::InvalidPatchError(
                    "apply_patch environment_id cannot be specified more than once".into(),
                ));
            }
            let env_id = env_id.trim();
            if env_id.is_empty() {
                return Err(ParseError::InvalidPatchError(
                    "apply_patch environment_id cannot be empty".into(),
                ));
            }
            self.environment_id = Some(env_id.to_string());
            return Ok(true);
        }

        if trimmed == END_PATCH_MARKER {
            self.ensure_update_hunk_not_empty(trimmed, line_no)?;
            self.mode = Mode::EndedPatch;
            return Ok(true);
        }

        if let Some(path) = trimmed.strip_prefix(ADD_FILE_MARKER) {
            self.ensure_update_hunk_not_empty(trimmed, line_no)?;
            self.hunks.push(Hunk::AddFile {
                path: PathBuf::from(path),
                contents: String::new(),
            });
            self.mode = Mode::AddFile;
            return Ok(true);
        }

        if let Some(path) = trimmed.strip_prefix(DELETE_FILE_MARKER) {
            self.ensure_update_hunk_not_empty(trimmed, line_no)?;
            self.hunks.push(Hunk::DeleteFile {
                path: PathBuf::from(path),
            });
            self.mode = Mode::DeleteFile;
            return Ok(true);
        }

        if let Some(path) = trimmed.strip_prefix(UPDATE_FILE_MARKER) {
            self.ensure_update_hunk_not_empty(trimmed, line_no)?;
            self.update_hunk_line_no = line_no;
            self.hunks.push(Hunk::UpdateFile {
                path: PathBuf::from(path),
                move_path: None,
                chunks: Vec::new(),
            });
            self.mode = Mode::UpdateFile;
            return Ok(true);
        }

        Ok(false)
    }

    fn ensure_update_hunk_not_empty(&self, line: &str, line_no: usize) -> Result<(), ParseError> {
        if let Some(Hunk::UpdateFile { path, chunks, .. }) = self.hunks.last() {
            if chunks.is_empty() {
                return Err(ParseError::InvalidHunkError {
                    message: format!("Update file hunk for path '{}' is empty", path.display()),
                    line_number: self.update_hunk_line_no,
                });
            }
            if chunks
                .last()
                .is_some_and(|c| c.old_lines.is_empty() && c.new_lines.is_empty())
            {
                return Err(ParseError::InvalidHunkError {
                    message: if line == END_PATCH_MARKER {
                        "Update hunk does not contain any lines".to_string()
                    } else {
                        format!(
                            "Unexpected line found in update hunk: '{line}'. \
                             Every line should start with ' ' (context line), \
                             '+' (added line), or '-' (removed line)"
                        )
                    },
                    line_number: line_no,
                });
            }
        }
        Ok(())
    }

    fn process_update_body_line(
        &mut self,
        line: &str,
        update_line: &str,
        line_no: usize,
    ) -> Result<(), ParseError> {
        let Some(Hunk::UpdateFile {
            move_path, chunks, ..
        }) = self.hunks.last_mut()
        else {
            return Err(ParseError::InvalidHunkError {
                message: format!("Unexpected line found in update hunk: '{line}'"),
                line_number: line_no,
            });
        };

        // After *** End of File, only allow empty lines or new @@ markers.
        if chunks.last().is_some_and(|c| c.is_end_of_file) {
            if update_line.is_empty() {
                return Ok(());
            }
            if update_line != EMPTY_CHANGE_CONTEXT_MARKER
                && !update_line.starts_with(CHANGE_CONTEXT_MARKER)
            {
                return Err(ParseError::InvalidHunkError {
                    message: format!(
                        "Expected update hunk to start with a @@ context marker, got: '{line}'"
                    ),
                    line_number: line_no,
                });
            }
        }

        // *** Move to: <path>
        if chunks.is_empty()
            && move_path.is_none()
            && let Some(dest) = update_line.strip_prefix(MOVE_TO_MARKER)
        {
            *move_path = Some(PathBuf::from(dest));
            return Ok(());
        }

        // Reject consecutive @@ with no content between them.
        if (update_line == EMPTY_CHANGE_CONTEXT_MARKER
            || update_line.starts_with(CHANGE_CONTEXT_MARKER))
            && chunks
                .last()
                .is_some_and(|c| c.old_lines.is_empty() && c.new_lines.is_empty())
        {
            return Err(ParseError::InvalidHunkError {
                message: format!(
                    "Unexpected line found in update hunk: '{line}'. \
                     Every line should start with ' ' (context line), \
                     '+' (added line), or '-' (removed line)"
                ),
                line_number: line_no,
            });
        }

        // @@ (empty context marker)
        if update_line == EMPTY_CHANGE_CONTEXT_MARKER {
            chunks.push(UpdateFileChunk::default());
            return Ok(());
        }

        // @@ <context>
        if let Some(ctx) = update_line.strip_prefix(CHANGE_CONTEXT_MARKER) {
            chunks.push(UpdateFileChunk {
                change_context: Some(ctx.to_string()),
                ..Default::default()
            });
            return Ok(());
        }

        // *** End of File
        if update_line == EOF_MARKER {
            if chunks
                .last()
                .is_some_and(|c| c.old_lines.is_empty() && c.new_lines.is_empty())
            {
                return Err(ParseError::InvalidHunkError {
                    message: "Update hunk does not contain any lines".into(),
                    line_number: line_no,
                });
            }
            if let Some(chunk) = chunks.last_mut() {
                chunk.is_end_of_file = true;
            }
            return Ok(());
        }

        // Empty line → context line (preserved).
        if line.is_empty() {
            ensure_chunk(chunks);
            if let Some(chunk) = chunks.last_mut() {
                chunk.old_lines.push(String::new());
                chunk.new_lines.push(String::new());
            }
            return Ok(());
        }

        // ' ' prefix → context line (unchanged).
        if let Some(text) = line.strip_prefix(' ') {
            ensure_chunk(chunks);
            if let Some(chunk) = chunks.last_mut() {
                chunk.old_lines.push(text.to_string());
                chunk.new_lines.push(text.to_string());
            }
            return Ok(());
        }

        // '+' prefix → added line.
        if let Some(text) = line.strip_prefix('+') {
            ensure_chunk(chunks);
            if let Some(chunk) = chunks.last_mut() {
                chunk.new_lines.push(text.to_string());
            }
            return Ok(());
        }

        // '-' prefix → removed line.
        if let Some(text) = line.strip_prefix('-') {
            ensure_chunk(chunks);
            if let Some(chunk) = chunks.last_mut() {
                chunk.old_lines.push(text.to_string());
            }
            return Ok(());
        }

        // Anything else is an error.
        Err(ParseError::InvalidHunkError {
            message: format!(
                "Unexpected line found in update hunk: '{line}'. \
                 Every line should start with ' ' (context line), \
                 '+' (added line), or '-' (removed line)"
            ),
            line_number: line_no,
        })
    }
}

/// Ensure there is at least one chunk to push lines into.
fn ensure_chunk(chunks: &mut Vec<UpdateFileChunk>) {
    if chunks.is_empty() {
        chunks.push(UpdateFileChunk::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bad_input() {
        assert_eq!(
            parse_patch_text("bad", ParseMode::Strict),
            Err(ParseError::InvalidPatchError(
                "The first line of the patch must be '*** Begin Patch'".into()
            ))
        );
    }

    #[test]
    fn test_parse_add_file() {
        let r = parse_patch("*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch").unwrap();
        assert_eq!(
            r.hunks,
            vec![Hunk::AddFile {
                path: PathBuf::from("foo"),
                contents: "hi\n".into(),
            }]
        );
    }

    #[test]
    fn test_parse_update_with_move_and_context() {
        let r = parse_patch(
            "*** Begin Patch\n\
             *** Update File: test.py\n\
             *** Move to: test2.py\n\
             @@ def f():\n\
             -    pass\n\
             +    return 123\n\
             *** End Patch",
        )
        .unwrap();
        assert_eq!(
            r.hunks,
            vec![Hunk::UpdateFile {
                path: PathBuf::from("test.py"),
                move_path: Some(PathBuf::from("test2.py")),
                chunks: vec![UpdateFileChunk {
                    change_context: Some("def f():".into()),
                    old_lines: vec!["    pass".into()],
                    new_lines: vec!["    return 123".into()],
                    is_end_of_file: false,
                }],
            }]
        );
    }

    #[test]
    fn test_parse_empty_update_rejected() {
        assert_eq!(
            parse_patch_text(
                "*** Begin Patch\n*** Update File: test.py\n*** End Patch",
                ParseMode::Strict
            ),
            Err(ParseError::InvalidHunkError {
                message: "Update file hunk for path 'test.py' is empty".into(),
                line_number: 2,
            })
        );
    }

    #[test]
    fn test_parse_lenient_heredoc() {
        let inner = "*** Begin Patch\n*** Update File: f.py\n import\n+bar\n*** End Patch";
        let heredoc = format!("<<'EOF'\n{inner}\nEOF\n");
        assert!(parse_patch_text(&heredoc, ParseMode::Strict).is_err());
        assert!(parse_patch_text(&heredoc, ParseMode::Lenient).is_ok());
    }

    #[test]
    fn test_parse_multiple_hunks() {
        let r = parse_patch(
            "*** Begin Patch\n\
             *** Add File: a.txt\n+content\n\
             *** Delete File: b.txt\n\
             *** Update File: c.txt\n@@\n-old\n+new\n\
             *** End Patch",
        )
        .unwrap();
        assert_eq!(r.hunks.len(), 3);
    }

    #[test]
    fn test_parse_indented_marker_as_context() {
        let r = parse_patch(
            "*** Begin Patch\n\
             *** Update File: a.txt\n\
             @@\n\
             -old a\n\
             +new a\n\
             *** Update File: b.txt\n\
             @@\n\
             -old b\n\
             +new b\n\
             *** End Patch",
        )
        .unwrap();
        // Only one hunk — the second *** Update File is consumed by the first
        // hunk as a context line because it has no leading space.
        // Wait — actually the parser checks trimmed for hunk headers even in
        // UpdateFile mode. Let me verify: try_hunk_header is called with
        // update_line (trim_end), and "*** Update File: b.txt" will match
        // the UPDATE_FILE_MARKER. So it should be two hunks.
        assert_eq!(r.hunks.len(), 2);
    }

    #[test]
    fn test_parse_crlf() {
        let r = parse_patch(
            "*** Begin Patch\r\n*** Update File: f.txt\r\n@@\r\n-old\r\n+new\r\n*** End Patch\r\n",
        )
        .unwrap();
        assert_eq!(r.hunks.len(), 1);
        match &r.hunks[0] {
            Hunk::UpdateFile { chunks, .. } => {
                assert_eq!(chunks[0].old_lines, vec!["old".to_string()]);
                assert_eq!(chunks[0].new_lines, vec!["new".to_string()]);
            }
            _ => panic!("expected UpdateFile"),
        }
    }

    #[test]
    fn test_parse_eof_marker() {
        let r = parse_patch(
            "*** Begin Patch\n*** Update File: f.txt\n@@\n+quux\n*** End of File\n*** End Patch",
        )
        .unwrap();
        match &r.hunks[0] {
            Hunk::UpdateFile { chunks, .. } => assert!(chunks[0].is_end_of_file),
            _ => panic!("expected UpdateFile"),
        }
    }
}
