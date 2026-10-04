//! Grep and glob operations through OpenDAL.
//!
//! These functions enumerate files via `op.lister_with(recursive)` and
//! perform content matching entirely in Rust. No filesystem walking
//! outside the OpenDAL operator's virtual root.

use std::collections::{BTreeSet, HashSet};

use agentik_core::tools::truncation::{TruncationConfig, truncate_tool_output};
use agentik_core::tools::{ToolError, ToolResult};
use agentik_sdk::types::ToolResult as AgentToolResult;
use futures::StreamExt;
use regex::{Regex, RegexBuilder};

use crate::storage::OpendalFileStorage;

/// Maximum number of files grep will scan in a directory tree.
const GREP_MAX_FILES: usize = 200;
/// Maximum file size for per-file scan during directory grep (256 KB).
/// Larger files are skipped to bound memory when scanning many files.
const GREP_MAX_FILE_BYTES: u64 = 256 * 1024;
/// Maximum file size for explicit single-file grep (2 GB).
/// The caller has chosen this one file, so we allow a generous limit.
const GREP_MAX_SINGLE_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Maximum result lines grep will return.
const GREP_MAX_RESULT_LINES: usize = 250;
/// Maximum entries glob will return.
const GLOB_MAX_RESULTS: usize = 100;

// ══════════════════════════ types ══════════════════════════

/// How grep results are presented to the caller.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GrepOutputMode {
    /// Matching lines with optional context — the default.
    Content,
    /// Only the paths of files that contain at least one match.
    FilesWithMatches,
    /// Per-file match counts (`path:count`).
    Count,
}

impl GrepOutputMode {
    fn parse(s: Option<&str>) -> Result<GrepOutputMode, String> {
        match s.map(|s| s.trim()).filter(|s| !s.is_empty()) {
            None | Some("content") => Ok(GrepOutputMode::Content),
            Some("files_with_matches") => Ok(GrepOutputMode::FilesWithMatches),
            Some("count") => Ok(GrepOutputMode::Count),
            Some(other) => Err(format!(
                "Unknown output_mode '{other}'. Expected: content | files_with_matches | count"
            )),
        }
    }
}

/// Per-file search result.
struct FileResult {
    path: String,
    match_count: usize,
    /// Formatted output lines for Content mode (includes context).
    /// Empty for [`GrepOutputMode::FilesWithMatches`] / [`GrepOutputMode::Count`].
    output_lines: Vec<String>,
}

// ══════════════════════════ helpers ══════════════════════════

/// Build a regex with smart-case or explicit case sensitivity.
///
/// **Smart case** (ripgrep `-S`, the default): when `case_insensitive`
/// is `None` and the pattern contains no uppercase ASCII letters,
/// matching is automatically case-insensitive. When the pattern has at
/// least one uppercase letter, matching is case-sensitive.
///
/// `Some(true)`  → always case-insensitive.
/// `Some(false)` → always case-sensitive.
fn build_regex(pattern: &str, case_insensitive: Option<bool>) -> Result<Regex, String> {
    let ci = case_insensitive.unwrap_or_else(|| !pattern.chars().any(|c| c.is_ascii_uppercase()));
    RegexBuilder::new(pattern)
        .case_insensitive(ci)
        .build()
        .map_err(|e| format!("Invalid regex: {e}"))
}

/// Format matching lines with optional before/after context.
///
/// Uses ripgrep-style separators in the output:
/// - `path:N:content` — a matching line (`:` after the line number)
/// - `path:N-content` — a context line (`-` after the line number)
/// - `--` on its own line — separator between non-contiguous groups
///
/// Overlapping context windows from nearby matches are merged
/// automatically (no duplicated lines).
fn format_content(
    lines: &[&str],
    match_indices: &[usize],
    path: &str,
    before: usize,
    after: usize,
) -> Vec<String> {
    if before == 0 && after == 0 {
        // Fast path: no context, just emit matches directly.
        return match_indices
            .iter()
            .map(|&i| format!("{path}:{}:{}", i + 1, lines[i].trim_end()))
            .collect();
    }

    // Expand match indices into the full set of line indices to display
    // (each match ± context). BTreeSet keeps them sorted and de-duplicated.
    let match_set: HashSet<usize> = match_indices.iter().copied().collect();
    let mut show: BTreeSet<usize> = BTreeSet::new();
    for &m in match_indices {
        let start = m.saturating_sub(before);
        let end = (m + after).min(lines.len().saturating_sub(1));
        for i in start..=end {
            show.insert(i);
        }
    }

    let mut result = Vec::new();
    let mut prev: Option<usize> = None;
    for &i in &show {
        // Insert group separator between non-contiguous line ranges.
        if prev.is_some_and(|p| i > p + 1) {
            result.push("--".to_string());
        }
        let sep = if match_set.contains(&i) { ':' } else { '-' };
        result.push(format!("{path}:{}{}{}", i + 1, sep, lines[i].trim_end()));
        prev = Some(i);
    }
    result
}

/// Read and search a single file.
///
/// Returns `(Option<FileResult>, files_scanned_delta, files_skipped_delta)`.
/// `Some(FileResult)` is returned only when the file has ≥1 match.
///
/// `max_bytes` controls the size guard: files larger than this are skipped.
/// Use [`GREP_MAX_FILE_BYTES`] for directory scans (conservative) or
/// [`GREP_MAX_SINGLE_FILE_BYTES`] for explicit single-file grep (generous).
async fn search_file(
    op: &opendal::Operator,
    entry_path: &str,
    display_path: &str,
    re: &Regex,
    output_mode: GrepOutputMode,
    before: usize,
    after: usize,
    max_bytes: u64,
) -> (Option<FileResult>, usize, usize) {
    // Size guard.
    let size = match op.stat(entry_path).await {
        Ok(m) => m.content_length(),
        Err(_) => return (None, 0, 0),
    };
    if size > max_bytes {
        return (None, 0, 1);
    }

    // Read content.
    let buf = match op.read(entry_path).await {
        Ok(b) => b,
        Err(_) => return (None, 0, 0),
    };
    let data = buf.to_vec();

    // Skip binary files — searching them as text yields garbled matches
    // (mirrors ripgrep's default behaviour).
    if super::ops::is_binary(&data) {
        return (None, 0, 1);
    }

    let content = String::from_utf8_lossy(&data);
    let lines: Vec<&str> = content.lines().collect();

    // Find all matching line indices.
    let match_indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| re.is_match(line))
        .map(|(i, _)| i)
        .collect();

    if match_indices.is_empty() {
        return (None, 1, 0);
    }

    let output_lines = match output_mode {
        GrepOutputMode::Content => {
            format_content(&lines, &match_indices, display_path, before, after)
        }
        // Other modes don't need per-line output.
        _ => Vec::new(),
    };

    (
        Some(FileResult {
            path: display_path.to_string(),
            match_count: match_indices.len(),
            output_lines,
        }),
        1,
        0,
    )
}

/// Collect and format all grep results into a JSON tool result.
fn format_grep_result(
    file_results: &[FileResult],
    output_mode: GrepOutputMode,
    files_scanned: usize,
    files_skipped: usize,
    hit_limit: bool,
) -> AgentToolResult {
    // Only files with matches are interesting.
    let matched: Vec<&FileResult> = file_results.iter().filter(|f| f.match_count > 0).collect();
    let total_matches: usize = matched.iter().map(|f| f.match_count).sum();

    // Build output lines according to the mode.
    let mut lines: Vec<String> = Vec::new();
    for fr in &matched {
        match output_mode {
            GrepOutputMode::Content => lines.extend(fr.output_lines.iter().cloned()),
            GrepOutputMode::FilesWithMatches => lines.push(fr.path.clone()),
            GrepOutputMode::Count => lines.push(format!("{}:{}", fr.path, fr.match_count)),
        }
        if lines.len() >= GREP_MAX_RESULT_LINES {
            lines.truncate(GREP_MAX_RESULT_LINES);
            break;
        }
    }

    // Truncated if output exceeded the line limit OR the scan loop broke early.
    let total_possible: usize = match output_mode {
        GrepOutputMode::Content => matched.iter().map(|f| f.output_lines.len()).sum(),
        _ => matched.len(),
    };
    let result_limited = total_possible > GREP_MAX_RESULT_LINES || hit_limit;

    let mut out = lines.join("\n");
    if result_limited {
        out.push_str(&format!(
            "\n\n(results limited to {GREP_MAX_RESULT_LINES} lines)"
        ));
    }
    let bounded_output = truncate_tool_output(&out, &TruncationConfig::default());
    out = bounded_output.content;
    let truncated = result_limited || bounded_output.truncated;
    if out.is_empty() {
        out = "(no matches)".to_string();
    }

    AgentToolResult::success_json(serde_json::json!({
        "matches": out,
        "files_scanned": files_scanned,
        "files_skipped": files_skipped,
        "total_matches": total_matches,
        "truncated": truncated,
    }))
}

// ══════════════════════════ grep ══════════════════════════

/// `grep` — content search across a directory tree or a single file.
///
/// When `path` is a directory, walks the tree via OpenDAL lister and
/// searches every file. When `path` is a regular file, searches that
/// file directly.
///
/// # Parameters
/// - `output_mode`: `content` (default) | `files_with_matches` | `count`
/// - `before` / `after`: context lines (content mode only)
/// - `case_insensitive`: `Some(true)` = always, `Some(false)` = never,
///   `None` = smart-case (auto-insensitive when pattern is all-lowercase)
pub async fn op_grep(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    pattern: Option<&str>,
    glob_filter: Option<&str>,
    output_mode: Option<&str>,
    before: Option<usize>,
    after: Option<usize>,
    case_insensitive: Option<bool>,
) -> Result<AgentToolResult, ToolError> {
    let vpath = OpendalFileStorage::normalize_path(path.unwrap_or("/"));
    let pattern = pattern.ok_or("missing 'pattern' for grep")?;
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    // Verify the search root exists.
    // Without this, OpenDAL's recursive lister on a missing path returns
    // an empty stream and the op would silently report "(no matches)".
    let meta = match op.stat(&remote).await {
        Ok(m) => m,
        Err(e) if matches!(e.kind(), opendal::ErrorKind::NotFound) => {
            return Ok(AgentToolResult::error(format!(
                "grep: '{vpath}': No such file or directory"
            )));
        }
        Err(e) => return Ok(AgentToolResult::error(format!("grep: '{vpath}': {e}"))),
    };

    let output_mode = GrepOutputMode::parse(output_mode)?;
    let re = build_regex(pattern, case_insensitive)?;
    let before = before.unwrap_or(0);
    let after = after.unwrap_or(0);

    let glob_pat = match glob_filter {
        Some(g) => Some(glob::Pattern::new(g).map_err(|e| format!("Invalid glob: {e}"))?),
        None => None,
    };

    let mut file_results: Vec<FileResult> = Vec::new();
    let mut files_scanned = 0usize;
    let mut files_skipped = 0usize;
    let mut hit_limit = false;

    // ── Single-file grep ──
    // When the path is a regular file, search it directly instead of
    // requiring the caller to pass a parent directory.
    if meta.is_file() {
        // Apply glob filename filter.
        if let Some(ref gp) = glob_pat {
            let name = vpath.rsplit('/').next().unwrap_or(&vpath);
            if !gp.matches(name) {
                return Ok(format_grep_result(&[], output_mode, 0, 0, false));
            }
        }
        let (fr, scanned, skipped) = search_file(
            &op,
            &remote,
            &vpath,
            &re,
            output_mode,
            before,
            after,
            GREP_MAX_SINGLE_FILE_BYTES,
        )
        .await;
        files_scanned += scanned;
        files_skipped += skipped;
        if let Some(fr) = fr {
            file_results.push(fr);
        }
        return Ok(format_grep_result(
            &file_results,
            output_mode,
            files_scanned,
            files_skipped,
            false,
        ));
    }

    // ── Directory grep ──
    let mut lister = op
        .lister_with(&remote)
        .recursive(true)
        .await
        .map_err(|e| e.to_string())?;

    let mut total_output_lines = 0usize;

    while let Some(entry) = lister.next().await {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.metadata().is_file() {
            continue;
        }

        if files_scanned + files_skipped >= GREP_MAX_FILES {
            hit_limit = true;
            break;
        }

        let entry_path = entry.path().to_string();
        let display_path = storage.remap_entry_to_virtual(&vpath, &entry_path);

        // Apply glob filename filter.
        if let Some(ref gp) = glob_pat {
            let name = entry_path.rsplit('/').next().unwrap_or(&entry_path);
            if !gp.matches(name) {
                continue;
            }
        }

        let (fr, scanned, skipped) = search_file(
            &op,
            &entry_path,
            &display_path,
            &re,
            output_mode,
            before,
            after,
            GREP_MAX_FILE_BYTES,
        )
        .await;
        files_scanned += scanned;
        files_skipped += skipped;

        if let Some(fr) = fr {
            // Track how many output lines this file would contribute.
            let lines_added = match output_mode {
                GrepOutputMode::Content => fr.output_lines.len(),
                _ => 1,
            };
            file_results.push(fr);
            total_output_lines += lines_added;
            if total_output_lines >= GREP_MAX_RESULT_LINES {
                hit_limit = true;
                break;
            }
        }
    }

    Ok(format_grep_result(
        &file_results,
        output_mode,
        files_scanned,
        files_skipped,
        hit_limit,
    ))
}

// ══════════════════════════ glob ══════════════════════════

/// `glob` — find files whose path matches a glob pattern.
///
/// Uses OpenDAL lister to enumerate, then matches each entry path
/// against the user-supplied pattern via the `glob` crate.
pub async fn op_glob(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    pattern: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let vpath = OpendalFileStorage::normalize_path(path.unwrap_or("/"));
    let pattern = pattern.ok_or("missing 'pattern' for glob")?;
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    // Build a glob pattern. We match against the path relative to the
    // search root so that `**/*.rs` works regardless of where we root.
    let pat = glob::Pattern::new(pattern).map_err(|e| format!("Invalid glob pattern: {e}"))?;

    let mut lister = op
        .lister_with(&remote)
        .recursive(true)
        .await
        .map_err(|e| e.to_string())?;

    let prefix = remote.trim_end_matches('/');
    let mut matches: Vec<String> = Vec::new();

    while let Some(entry) = lister.next().await {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.metadata().is_file() {
            continue;
        }

        let p = entry.path().to_string();
        // Strip the search prefix so the pattern matches relative paths.
        let rel = p.strip_prefix(prefix).unwrap_or(&p).trim_start_matches('/');
        let display = storage.remap_entry_to_virtual(&vpath, &p);

        if pat.matches(rel) || pat.matches(&display) {
            let rendered = if display.starts_with('/') {
                display
            } else {
                format!("/{display}")
            };
            matches.push(rendered);
            if matches.len() >= GLOB_MAX_RESULTS {
                break;
            }
        }
    }

    matches.sort();
    let truncated = matches.len() >= GLOB_MAX_RESULTS;
    let mut out = matches.join("\n");
    if truncated {
        out.push_str(&format!("\n\n(results limited to {GLOB_MAX_RESULTS})"));
    }
    if out.is_empty() {
        out = "(no matches)".to_string();
    }

    Ok(AgentToolResult::success_json(serde_json::json!({
        "matches": out,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_sdk::types::ToolResultContent;
    use std::sync::Arc;

    fn make_op() -> Arc<OpendalFileStorage> {
        Arc::new(OpendalFileStorage::new_temp())
    }

    async fn write_file(storage: &OpendalFileStorage, path: &str, content: &str) {
        storage
            .resolve(path)
            .write(path, content.to_string())
            .await
            .unwrap();
    }

    fn json_val(result: AgentToolResult) -> serde_json::Value {
        match result.content {
            ToolResultContent::Json(v) => v,
            other => panic!("expected JSON, got: {other:?}"),
        }
    }

    // ── test helpers for the new multi-parameter op_grep ──

    /// Basic grep: path + pattern only, all advanced options default.
    async fn grep(storage: &OpendalFileStorage, path: &str, pattern: &str) -> AgentToolResult {
        op_grep(
            storage,
            Some(path),
            Some(pattern),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap()
    }

    /// Grep with a glob filter.
    async fn grep_glob(
        storage: &OpendalFileStorage,
        path: &str,
        pattern: &str,
        glob: &str,
    ) -> AgentToolResult {
        op_grep(
            storage,
            Some(path),
            Some(pattern),
            Some(glob),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap()
    }

    // ═══════════ existing tests (updated signatures) ═══════════

    #[tokio::test]
    async fn grep_nonexistent_directory_errors() {
        let storage = make_op();
        let result = op_grep(
            &storage,
            Some("/no_such_dir_xyz"),
            Some("anything"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.is_error, Some(true));
        let s = format!("{:?}", result.content);
        assert!(
            s.contains("No such file") || s.contains("not found"),
            "expected not-found error, got: {s}"
        );
    }

    #[tokio::test]
    async fn grep_single_file_finds_matches() {
        let storage = make_op();
        write_file(&storage, "a.txt", "hello world\nfoo bar\nnope\n").await;

        let result = grep(&storage, "/a.txt", "hello").await;
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("a.txt"));
        assert!(matches.contains("hello world"));
        assert!(!matches.contains("nope"));
        assert_eq!(json["files_scanned"], 1);
        assert_eq!(json["files_skipped"], 0);
    }

    #[tokio::test]
    async fn grep_single_file_no_matches() {
        let storage = make_op();
        write_file(&storage, "a.txt", "hello\n").await;

        let result = grep(&storage, "/a.txt", "zzz_absent").await;
        let json = json_val(result);
        assert_eq!(json["matches"].as_str().unwrap(), "(no matches)");
        assert_eq!(json["files_scanned"], 1);
    }

    #[tokio::test]
    async fn grep_single_file_with_glob_filter_match() {
        let storage = make_op();
        write_file(&storage, "code.rs", "fn main() {}\n").await;

        let result = grep_glob(&storage, "/code.rs", "fn", "*.rs").await;
        let json = json_val(result);
        assert!(json["matches"].as_str().unwrap().contains("fn main"));
    }

    #[tokio::test]
    async fn grep_single_file_with_glob_filter_no_match() {
        let storage = make_op();
        write_file(&storage, "code.rs", "fn main() {}\n").await;

        let result = grep_glob(&storage, "/code.rs", "fn", "*.txt").await;
        let json = json_val(result);
        assert_eq!(json["matches"].as_str().unwrap(), "(no matches)");
        assert_eq!(json["files_scanned"], 0);
    }

    #[tokio::test]
    async fn grep_single_file_allows_large_file() {
        let storage = make_op();
        // 300 KB — exceeds the 256 KB directory-scan limit but is well
        // within the 2 GB single-file limit.
        let big = "target\n".repeat(50_000); // ~350 KB
        write_file(&storage, "big.txt", &big).await;

        let result = grep(&storage, "/big.txt", "target").await;
        let json = json_val(result);
        // Single-file grep should read and search the file successfully.
        assert_eq!(json["files_scanned"], 1);
        assert_eq!(json["files_skipped"], 0);
        assert_eq!(json["total_matches"], 50_000);
    }

    #[tokio::test]
    async fn grep_single_file_forcibly_bounds_large_output() {
        let storage = make_op();
        let long_line = format!("target {}\n", "世".repeat(60_000));
        write_file(&storage, "long.txt", &long_line).await;

        let result = grep(&storage, "/long.txt", "target").await;
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("[output truncated"));
        assert!(json["truncated"].as_bool().unwrap());
        assert_eq!(matches.chars().count(), 50_000);
    }

    #[tokio::test]
    async fn grep_single_file_skips_binary() {
        let storage = make_op();
        storage
            .resolve("/")
            .write("blob.bin", b"\x00target\x00".to_vec())
            .await
            .unwrap();

        let result = grep(&storage, "/blob.bin", "target").await;
        let json = json_val(result);
        assert_eq!(json["matches"].as_str().unwrap(), "(no matches)");
        assert_eq!(json["files_skipped"], 1);
        assert_eq!(json["files_scanned"], 0);
    }

    #[tokio::test]
    async fn grep_finds_matches() {
        let storage = make_op();
        write_file(&storage, "a.txt", "hello world\nfoo bar\n").await;
        write_file(&storage, "b.txt", "no match here\n").await;

        let result = grep(&storage, "/", "hello").await;
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("a.txt"));
        assert!(matches.contains("hello world"));
        assert!(!matches.contains("b.txt"));
    }

    #[tokio::test]
    async fn grep_with_glob_filter() {
        let storage = make_op();
        write_file(&storage, "code.rs", "fn main() {}\n").await;
        write_file(&storage, "doc.md", "fn not_code()\n").await;

        let result = grep_glob(&storage, "/", "fn", "*.rs").await;
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("code.rs"));
        assert!(!matches.contains("doc.md"));
    }

    #[tokio::test]
    async fn grep_no_matches() {
        let storage = make_op();
        write_file(&storage, "x.txt", "nothing interesting\n").await;

        let result = grep(&storage, "/", "zzz_absent").await;
        let json = json_val(result);
        assert_eq!(json["matches"].as_str().unwrap(), "(no matches)");
    }

    #[tokio::test]
    async fn grep_invalid_regex() {
        let storage = make_op();
        let result = op_grep(
            &storage,
            Some("/"),
            Some("[unclosed"),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn glob_finds_files() {
        let storage = make_op();
        write_file(&storage, "src/main.rs", "").await;
        write_file(&storage, "src/util.rs", "").await;
        write_file(&storage, "README.md", "").await;

        let result = op_glob(&storage, Some("/"), Some("**/*.rs")).await.unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("main.rs"));
        assert!(matches.contains("util.rs"));
        assert!(!matches.contains("README.md"));
    }

    #[tokio::test]
    async fn glob_no_matches() {
        let storage = make_op();
        write_file(&storage, "a.txt", "").await;

        let result = op_glob(&storage, Some("/"), Some("*.xyz")).await.unwrap();
        let json = json_val(result);
        assert_eq!(json["matches"].as_str().unwrap(), "(no matches)");
    }

    #[tokio::test]
    async fn grep_skips_large_file() {
        let storage = make_op();
        let big = "x".repeat(300_000);
        write_file(&storage, "big.txt", &big).await;
        write_file(&storage, "small.txt", "target line\n").await;

        let result = grep(&storage, "/", "target").await;
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("small.txt"));
        assert_eq!(json["files_skipped"], 1);
    }

    #[tokio::test]
    async fn grep_skips_binary_file() {
        let storage = make_op();
        storage
            .resolve("/")
            .write("blob.bin", b"\x00target\x00".to_vec())
            .await
            .unwrap();
        write_file(&storage, "real.txt", "target line\n").await;

        let result = grep(&storage, "/", "target").await;
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("real.txt"));
        assert!(!matches.contains("blob.bin"));
        assert_eq!(json["files_skipped"], 1);
    }

    // ═══════════ output_mode tests ═══════════

    #[tokio::test]
    async fn output_mode_files_with_matches() {
        let storage = make_op();
        write_file(&storage, "a.rs", "match here\nnope\nmatch again\n").await;
        write_file(&storage, "b.rs", "match too\n").await;
        write_file(&storage, "c.txt", "nothing\n").await;

        let result = op_grep(
            &storage,
            Some("/"),
            Some("match"),
            None,
            Some("files_with_matches"),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        // Should list only filenames, not content.
        assert!(matches.contains("a.rs"));
        assert!(matches.contains("b.rs"));
        assert!(!matches.contains("c.txt"));
        assert!(!matches.contains("match here"));
        assert_eq!(json["total_matches"], 3); // 2 in a.rs + 1 in b.rs
    }

    #[tokio::test]
    async fn output_mode_count() {
        let storage = make_op();
        write_file(&storage, "a.rs", "target\nx\ntarget\ntarget\n").await;
        write_file(&storage, "b.rs", "target\n").await;
        write_file(&storage, "c.txt", "nope\n").await;

        let result = op_grep(
            &storage,
            Some("/"),
            Some("target"),
            None,
            Some("count"),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        // Should show per-file counts, not content.
        assert!(matches.contains("a.rs:3"));
        assert!(matches.contains("b.rs:1"));
        assert!(!matches.contains("c.txt"));
        assert_eq!(json["total_matches"], 4);
    }

    #[tokio::test]
    async fn output_mode_content_is_default() {
        let storage = make_op();
        write_file(&storage, "a.txt", "hello\n").await;

        // No output_mode → defaults to content.
        let result = grep(&storage, "/", "hello").await;
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("hello"));
        // Content mode shows line content, not just filename.
        assert!(matches.contains("a.txt"));
    }

    #[tokio::test]
    async fn output_mode_invalid_errors() {
        let storage = make_op();
        write_file(&storage, "a.txt", "hello\n").await;
        let result = op_grep(
            &storage,
            Some("/"),
            Some("hello"),
            None,
            Some("bogus_mode"),
            None,
            None,
            None,
        )
        .await;
        assert!(result.is_err());
    }

    // ═══════════ context line tests ═══════════

    #[tokio::test]
    async fn context_after_shows_following_lines() {
        let storage = make_op();
        write_file(&storage, "code.rs", "line1\nline2\nMATCH\nline4\nline5\n").await;

        let result = op_grep(
            &storage,
            Some("/code.rs"),
            Some("MATCH"),
            None,
            None,
            None,
            Some(2),
            None, // after = 2
        )
        .await
        .unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        // Match line uses ':'
        assert!(matches.contains(":3:MATCH"));
        // Context lines use '-'
        assert!(matches.contains("4-line4"));
        assert!(matches.contains("5-line5"));
        // Lines before the match are NOT shown (before = 0).
        assert!(!matches.contains("line1"));
        assert!(!matches.contains("line2"));
    }

    #[tokio::test]
    async fn context_before_shows_preceding_lines() {
        let storage = make_op();
        write_file(&storage, "code.rs", "line1\nline2\nMATCH\nline4\nline5\n").await;

        let result = op_grep(
            &storage,
            Some("/code.rs"),
            Some("MATCH"),
            None,
            None,
            Some(2),
            None,
            None, // before = 2
        )
        .await
        .unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("1-line1"));
        assert!(matches.contains("2-line2"));
        assert!(matches.contains(":3:MATCH"));
        // Lines after the match are NOT shown (after = 0).
        assert!(!matches.contains("line4"));
        assert!(!matches.contains("line5"));
    }

    #[tokio::test]
    async fn context_both_before_and_after() {
        let storage = make_op();
        write_file(&storage, "f.txt", "a\nb\nMATCH\nc\nd\n").await;

        let result = op_grep(
            &storage,
            Some("/f.txt"),
            Some("MATCH"),
            None,
            None,
            Some(1),
            Some(1),
            None, // before=1, after=1
        )
        .await
        .unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        // b is context before, MATCH is the hit, c is context after.
        assert!(matches.contains("2-b"));
        assert!(matches.contains(":3:MATCH"));
        assert!(matches.contains("4-c"));
        assert!(!matches.contains("1-a"));
        assert!(!matches.contains("5-d"));
    }

    #[tokio::test]
    async fn context_separator_between_noncontiguous_groups() {
        let storage = make_op();
        // Two matches far apart, with before=1 after=1.
        write_file(&storage, "f.txt", "M1\nx\nx\nx\nx\nx\nM2\n").await;

        let result = op_grep(
            &storage,
            Some("/f.txt"),
            Some("M[12]"),
            None,
            None,
            Some(1),
            Some(1),
            None,
        )
        .await
        .unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        // There should be a `--` separator between the two groups.
        assert!(matches.contains("--"));
        // Group 1: line 1 (match) + line 2 (after context)
        assert!(matches.contains(":1:M1"));
        assert!(matches.contains("2-x"));
        // Group 2: line 6 (before context) + line 7 (match)
        assert!(matches.contains("6-x"));
        assert!(matches.contains(":7:M2"));
    }

    #[tokio::test]
    async fn context_overlapping_windows_merge() {
        let storage = make_op();
        // Two matches only 2 lines apart — with before=2, after=2
        // their context windows overlap and should merge into one group.
        write_file(&storage, "f.txt", "a\nb\nM1\nc\nM2\nd\n").await;

        let result = op_grep(
            &storage,
            Some("/f.txt"),
            Some("M[12]"),
            None,
            None,
            Some(2),
            Some(2),
            None,
        )
        .await
        .unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        // No separator because windows overlap.
        assert!(!matches.contains("--"));
        // All 6 lines should be present (a=before, b=before, M1=match,
        // c=between, M2=match, d=after).
        assert!(matches.contains("1-a"));
        assert!(matches.contains(":3:M1"));
        assert!(matches.contains(":5:M2"));
        assert!(matches.contains("6-d"));
    }

    #[tokio::test]
    async fn context_at_file_boundaries() {
        let storage = make_op();
        // Match at first and last lines — context should clamp.
        write_file(&storage, "f.txt", "M1\nb\nc\nd\nM2\n").await;

        let result = op_grep(
            &storage,
            Some("/f.txt"),
            Some("M[12]"),
            None,
            None,
            Some(5),
            Some(5),
            None, // generous context
        )
        .await
        .unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        // No out-of-range line numbers.
        assert!(matches.contains(":1:M1"));
        assert!(matches.contains(":5:M2"));
        // All lines present (no separator since context windows overlap).
        assert!(!matches.contains("--"));
    }

    #[tokio::test]
    async fn context_only_affects_content_mode() {
        let storage = make_op();
        write_file(&storage, "data.rs", "alpha\nMATCH\nbeta\n").await;

        // files_with_matches + context → context is ignored.
        let result = op_grep(
            &storage,
            Some("/data.rs"),
            Some("MATCH"),
            None,
            Some("files_with_matches"),
            Some(10),
            Some(10),
            None,
        )
        .await
        .unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        // Just the filename, no context lines from the file content.
        assert!(!matches.contains("alpha"));
        assert!(!matches.contains("beta"));
        assert!(matches.contains("data.rs"));
    }

    // ═══════════ case sensitivity tests ═══════════

    #[tokio::test]
    async fn smart_case_all_lowercase_matches_case_insensitive() {
        let storage = make_op();
        write_file(&storage, "f.txt", "Hello World\nHELLO\nhello\n").await;

        // Pattern is all-lowercase → smart-case makes it insensitive.
        let result = op_grep(
            &storage,
            Some("/f.txt"),
            Some("hello"),
            None,
            None,
            None,
            None,
            None, // case_insensitive = None → smart
        )
        .await
        .unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("Hello World"));
        assert!(matches.contains("HELLO"));
        assert!(matches.contains("hello"));
        assert_eq!(json["total_matches"], 3);
    }

    #[tokio::test]
    async fn smart_case_pattern_with_uppercase_is_case_sensitive() {
        let storage = make_op();
        write_file(&storage, "f.txt", "Hello\nhello\nHELLO\n").await;

        // Pattern has uppercase 'H' → smart-case keeps it sensitive.
        let result = op_grep(
            &storage,
            Some("/f.txt"),
            Some("Hello"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("Hello"));
        assert!(!matches.contains("hello"));
        assert!(!matches.contains("HELLO"));
        assert_eq!(json["total_matches"], 1);
    }

    #[tokio::test]
    async fn explicit_case_insensitive_true() {
        let storage = make_op();
        write_file(&storage, "f.txt", "Foo\nfoo\nFOO\n").await;

        // Explicit case_insensitive=true overrides smart-case.
        let result = op_grep(
            &storage,
            Some("/f.txt"),
            Some("Foo"),
            None,
            None,
            None,
            None,
            Some(true),
        )
        .await
        .unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("Foo"));
        assert!(matches.contains("foo"));
        assert!(matches.contains("FOO"));
        assert_eq!(json["total_matches"], 3);
    }

    #[tokio::test]
    async fn explicit_case_sensitive_false() {
        let storage = make_op();
        write_file(&storage, "f.txt", "Foo\nfoo\nFOO\n").await;

        // Explicit case_insensitive=false forces sensitive even for
        // all-lowercase patterns.
        let result = op_grep(
            &storage,
            Some("/f.txt"),
            Some("foo"),
            None,
            None,
            None,
            None,
            Some(false),
        )
        .await
        .unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("foo"));
        assert!(!matches.contains("Foo"));
        assert!(!matches.contains("FOO"));
        assert_eq!(json["total_matches"], 1);
    }
}
