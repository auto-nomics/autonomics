//! Grep and glob operations through OpenDAL.
//!
//! These functions enumerate files via `op.lister_with(recursive)` and
//! perform content matching entirely in Rust. No filesystem walking
//! outside the OpenDAL operator's virtual root.

use agentik_core::tools::{ToolError, ToolResult};
use agentik_sdk::types::ToolResult as AgentToolResult;
use futures::StreamExt;
use regex::Regex;

use crate::storage::OpendalFileStorage;

/// Maximum number of files grep will scan.
const GREP_MAX_FILES: usize = 200;
/// Maximum file size grep will read (256 KB). Larger files are skipped.
const GREP_MAX_FILE_BYTES: u64 = 256 * 1024;
/// Maximum result lines grep will return.
const GREP_MAX_RESULT_LINES: usize = 250;
/// Maximum entries glob will return.
const GLOB_MAX_RESULTS: usize = 100;

/// `grep` — recursive content search.
///
/// Walks the directory tree via OpenDAL lister, reads each file's content,
/// and matches lines against the regex pattern.
pub async fn op_grep(
    op: &opendal::Operator,
    path: Option<&str>,
    pattern: Option<&str>,
    glob_filter: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let vpath = OpendalFileStorage::normalize_path(path.unwrap_or("/"));
    let pattern = pattern.ok_or("missing 'pattern' for grep")?;

    // Verify the search root exists and is a directory before walking.
    // Without this, OpenDAL's recursive lister on a missing path returns
    // an empty stream and the op would silently report "(no matches)".
    let meta = match op.stat(&vpath).await {
        Ok(m) => m,
        Err(e) if matches!(e.kind(), opendal::ErrorKind::NotFound) => {
            return Ok(AgentToolResult::error(format!(
                "grep: '{vpath}': No such file or directory"
            )));
        }
        Err(e) => return Ok(AgentToolResult::error(format!("grep: '{vpath}': {e}"))),
    };
    if !meta.is_dir() {
        return Ok(AgentToolResult::error(format!(
            "grep: '{vpath}' is not a directory"
        )));
    }

    let re = Regex::new(pattern).map_err(|e| format!("Invalid regex: {e}"))?;

    let glob_pat = match glob_filter {
        Some(g) => Some(glob::Pattern::new(g).map_err(|e| format!("Invalid glob: {e}"))?),
        None => None,
    };

    let mut lister = op
        .lister_with(&vpath)
        .recursive(true)
        .await
        .map_err(|e| e.to_string())?;

    let mut results: Vec<String> = Vec::new();
    let mut files_scanned = 0usize;
    let mut files_skipped = 0usize;

    while let Some(entry) = lister.next().await {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.metadata().is_file() {
            continue;
        }

        if files_scanned >= GREP_MAX_FILES {
            break;
        }

        let entry_path = entry.path().to_string();

        // Apply glob filename filter.
        if let Some(ref gp) = glob_pat {
            let name = entry_path.rsplit('/').next().unwrap_or(&entry_path);
            if !gp.matches(name) {
                continue;
            }
        }

        // Size guard: skip oversized files.
        let size = op
            .stat(&entry_path)
            .await
            .ok()
            .map(|m| m.content_length())
            .unwrap_or(0);
        if size > GREP_MAX_FILE_BYTES {
            files_skipped += 1;
            continue;
        }

        files_scanned += 1;

        // Read content and match.
        let buf = match op.read(&entry_path).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let data = buf.to_vec();
        // Skip binary files — searching them as text yields garbled matches
        // (mirrors ripgrep's default behaviour).
        if super::ops::is_binary(&data) {
            files_skipped += 1;
            continue;
        }
        let content = String::from_utf8_lossy(&data);

        for (i, line) in content.lines().enumerate() {
            if re.is_match(line) {
                results.push(format!("{entry_path}:{}:{}", i + 1, line.trim_end()));
                if results.len() >= GREP_MAX_RESULT_LINES {
                    break;
                }
            }
        }
        if results.len() >= GREP_MAX_RESULT_LINES {
            break;
        }
    }

    let truncated = results.len() >= GREP_MAX_RESULT_LINES;
    let mut out = results.join("\n");
    if truncated {
        out.push_str(&format!(
            "\n\n(results limited to {GREP_MAX_RESULT_LINES} lines)"
        ));
    }
    if out.is_empty() {
        out = "(no matches)".to_string();
    }

    Ok(AgentToolResult::success_json(serde_json::json!({
        "matches": out,
        "files_scanned": files_scanned,
        "files_skipped": files_skipped,
    })))
}

/// `glob` — find files whose path matches a glob pattern.
///
/// Uses OpenDAL lister to enumerate, then matches each entry path
/// against the user-supplied pattern via the `glob` crate.
pub async fn op_glob(
    op: &opendal::Operator,
    path: Option<&str>,
    pattern: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let vpath = OpendalFileStorage::normalize_path(path.unwrap_or("/"));
    let pattern = pattern.ok_or("missing 'pattern' for glob")?;

    // Build a glob pattern. We match against the path relative to the
    // search root so that `**/*.rs` works regardless of where we root.
    let pat = glob::Pattern::new(pattern).map_err(|e| format!("Invalid glob pattern: {e}"))?;

    let mut lister = op
        .lister_with(&vpath)
        .recursive(true)
        .await
        .map_err(|e| e.to_string())?;

    let prefix = vpath.trim_end_matches('/');
    let mut matches: Vec<String> = Vec::new();

    while let Some(entry) = lister.next().await {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.metadata().is_file() {
            continue;
        }

        let p = entry.path().to_string();
        // Strip the search prefix so the pattern matches relative paths.
        let rel = p.strip_prefix(prefix).unwrap_or(&p).trim_start_matches('/');

        if pat.matches(rel) || pat.matches(&p) {
            matches.push(format!("/{p}"));
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

    fn make_op() -> opendal::Operator {
        Arc::new(OpendalFileStorage::new_temp()).op.clone()
    }

    async fn write_file(op: &opendal::Operator, path: &str, content: &str) {
        op.write(path, content.to_string()).await.unwrap();
    }

    fn json_val(result: AgentToolResult) -> serde_json::Value {
        match result.content {
            ToolResultContent::Json(v) => v,
            other => panic!("expected JSON, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn grep_nonexistent_directory_errors() {
        let op = make_op();
        let result = op_grep(&op, Some("/no_such_dir_xyz"), Some("anything"), None)
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
    async fn grep_path_is_file_errors() {
        let op = make_op();
        write_file(&op, "a.txt", "hello\n").await;
        let result = op_grep(&op, Some("/a.txt"), Some("hello"), None)
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let s = format!("{:?}", result.content);
        assert!(
            s.contains("not a directory"),
            "expected not-a-directory error, got: {s}"
        );
    }

    #[tokio::test]
    async fn grep_finds_matches() {
        let op = make_op();
        write_file(&op, "a.txt", "hello world\nfoo bar\n").await;
        write_file(&op, "b.txt", "no match here\n").await;

        let result = op_grep(&op, Some("/"), Some("hello"), None).await.unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("a.txt"));
        assert!(matches.contains("hello world"));
        assert!(!matches.contains("b.txt"));
    }

    #[tokio::test]
    async fn grep_with_glob_filter() {
        let op = make_op();
        write_file(&op, "code.rs", "fn main() {}\n").await;
        write_file(&op, "doc.md", "fn not_code()\n").await;

        let result = op_grep(&op, Some("/"), Some("fn"), Some("*.rs"))
            .await
            .unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("code.rs"));
        assert!(!matches.contains("doc.md"));
    }

    #[tokio::test]
    async fn grep_no_matches() {
        let op = make_op();
        write_file(&op, "x.txt", "nothing interesting\n").await;

        let result = op_grep(&op, Some("/"), Some("zzz_absent"), None)
            .await
            .unwrap();
        let json = json_val(result);
        assert_eq!(json["matches"].as_str().unwrap(), "(no matches)");
    }

    #[tokio::test]
    async fn grep_invalid_regex() {
        let op = make_op();
        let result = op_grep(&op, Some("/"), Some("[unclosed"), None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn glob_finds_files() {
        let op = make_op();
        write_file(&op, "src/main.rs", "").await;
        write_file(&op, "src/util.rs", "").await;
        write_file(&op, "README.md", "").await;

        let result = op_glob(&op, Some("/"), Some("**/*.rs")).await.unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("main.rs"));
        assert!(matches.contains("util.rs"));
        assert!(!matches.contains("README.md"));
    }

    #[tokio::test]
    async fn glob_no_matches() {
        let op = make_op();
        write_file(&op, "a.txt", "").await;

        let result = op_glob(&op, Some("/"), Some("*.xyz")).await.unwrap();
        let json = json_val(result);
        assert_eq!(json["matches"].as_str().unwrap(), "(no matches)");
    }

    #[tokio::test]
    async fn grep_skips_large_file() {
        let op = make_op();
        // Create a file larger than GREP_MAX_FILE_BYTES (256 KB)
        let big = "x".repeat(300_000);
        write_file(&op, "big.txt", &big).await;
        write_file(&op, "small.txt", "target line\n").await;

        let result = op_grep(&op, Some("/"), Some("target"), None).await.unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("small.txt"));
        assert_eq!(json["files_skipped"], 1);
    }

    #[tokio::test]
    async fn grep_skips_binary_file() {
        let op = make_op();
        // Binary file containing the pattern as a literal substring but
        // with NUL bytes present — should be skipped, not matched.
        op.write("blob.bin", b"\x00target\x00".to_vec())
            .await
            .unwrap();
        write_file(&op, "real.txt", "target line\n").await;

        let result = op_grep(&op, Some("/"), Some("target"), None).await.unwrap();
        let json = json_val(result);
        let matches = json["matches"].as_str().unwrap();
        assert!(matches.contains("real.txt"));
        assert!(!matches.contains("blob.bin"));
        assert_eq!(json["files_skipped"], 1);
    }
}
