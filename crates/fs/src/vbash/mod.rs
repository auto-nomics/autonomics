//! VFS Bash — structured file operations through OpenDAL, no system shell.
//!
//! A single tool (`vfs`) dispatches on an `op` field to one of many pure-Rust
//! file-operation handlers. Every handler talks directly to the OpenDAL
//! [`Operator`]; no subprocess is ever spawned.

mod ops;
mod search;

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::storage::OpendalFileStorage;

// ────────────────────────── input ──────────────────────────

#[tool(
    name = "vfs",
    description = "Virtual filesystem operations through OpenDAL VFS. \
        All operations execute in pure Rust — no system shell is spawned. \
        Supported ops: read, cat, ls, cp, mv, rm, mkdir, stat, touch, \
        write, edit, head, tail, wc, grep, glob, tree. \
        Unsupported (will error): chmod, chown, ln, pipes, redirects."
)]
pub struct VfsBashInput {
    #[desc = "Operation: read|cat|ls|cp|mv|rm|mkdir|stat|touch|write|edit|head|tail|wc|grep|glob|tree"]
    pub op: String,
    #[desc = "Primary path (file or directory)."]
    pub path: Option<String>,
    #[desc = "Source path for cp/mv."]
    pub src: Option<String>,
    #[desc = "Destination path for cp/mv."]
    pub dst: Option<String>,
    #[desc = "Content to write (for write op)."]
    pub content: Option<String>,
    #[desc = "Text to find (for edit op)."]
    pub old_string: Option<String>,
    #[desc = "Replacement text (for edit op)."]
    pub new_string: Option<String>,
    #[desc = "Replace all occurrences (for edit op). Default false."]
    pub replace_all: Option<bool>,
    #[desc = "List recursively (for ls/tree). Default varies by op."]
    pub recursive: Option<bool>,
    #[desc = "Regex pattern (for grep) or glob pattern (for glob op)."]
    pub pattern: Option<String>,
    #[desc = "Glob filter to narrow grep file candidates, e.g. '*.rs'."]
    pub glob: Option<String>,
    #[desc = "Starting line number, 1-indexed (for cat/read/head/tail), \
        or number of entries to skip (for ls pagination)."]
    pub offset: Option<usize>,
    #[desc = "Max lines/entries to return (for cat/read/head/tail/ls). \
        ls defaults to 200; use with offset to page through large listings."]
    pub limit: Option<usize>,
}

// ────────────────────────── tool ──────────────────────────

pub struct VfsBashTool {
    pub storage: Arc<OpendalFileStorage>,
}

#[async_trait]
impl ToolFunction for VfsBashTool {
    type Input = VfsBashInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let op = &self.storage.op;

        match input.op.as_str() {
            // ── reading ──
            "read" => ops::op_read(op, input.path.as_deref(), input.offset, input.limit).await,
            "cat" => ops::op_cat(op, input.path.as_deref(), input.offset, input.limit).await,
            "head" => ops::op_head(op, input.path.as_deref(), input.offset, input.limit).await,
            "tail" => ops::op_tail(op, input.path.as_deref(), input.offset, input.limit).await,

            // ── writing ──
            "write" => ops::op_write(op, input.path.as_deref(), input.content.as_deref()).await,
            "edit" => {
                ops::op_edit(
                    op,
                    input.path.as_deref(),
                    input.old_string.as_deref(),
                    input.new_string.as_deref(),
                    input.replace_all,
                )
                .await
            }
            "touch" => ops::op_touch(op, input.path.as_deref()).await,

            // ── filesystem ──
            "ls" => {
                ops::op_ls(
                    op,
                    input.path.as_deref(),
                    input.recursive,
                    input.limit,
                    input.offset,
                )
                .await
            }
            "stat" => ops::op_stat(op, input.path.as_deref()).await,
            "mkdir" => ops::op_mkdir(op, input.path.as_deref()).await,
            "rm" => ops::op_rm(op, input.path.as_deref(), input.recursive).await,
            "cp" => ops::op_cp(op, input.src.as_deref(), input.dst.as_deref()).await,
            "mv" => ops::op_mv(op, input.src.as_deref(), input.dst.as_deref()).await,
            "wc" => ops::op_wc(op, input.path.as_deref()).await,
            "tree" => ops::op_tree(op, input.path.as_deref(), input.limit).await,

            // ── search ──
            "grep" => {
                search::op_grep(
                    op,
                    input.path.as_deref(),
                    input.pattern.as_deref(),
                    input.glob.as_deref(),
                )
                .await
            }
            "glob" => search::op_glob(op, input.path.as_deref(), input.pattern.as_deref()).await,

            // ── unsupported ──
            other => Ok(AgentToolResult::error(format!(
                "Unknown or unsupported operation '{other}'. \
                 Supported: read cat ls cp mv rm mkdir stat touch write edit \
                 head tail wc grep glob tree."
            ))),
        }
    }
}

// ────────────────────────── registration ──────────────────────────

/// Build the [`ToolRegistration`] for the unified VFS bash tool.
///
/// Pass a shared [`OpendalFileStorage`] so the tool reuses the same
/// OpenDAL operator as the rest of the system.
pub fn vbash_registrations(storage: Arc<OpendalFileStorage>) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(VfsBashTool { storage })]
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_sdk::types::ToolResultContent;

    /// Helper: create a `VfsBashTool` backed by a temp directory.
    fn make_tool() -> VfsBashTool {
        VfsBashTool {
            storage: Arc::new(OpendalFileStorage::new_temp()),
        }
    }

    /// Helper: create a `VfsBashInput` with only `op` set, rest `None`.
    fn input(op: &str) -> VfsBashInput {
        VfsBashInput {
            op: op.into(),
            path: None,
            src: None,
            dst: None,
            content: None,
            old_string: None,
            new_string: None,
            replace_all: None,
            recursive: None,
            pattern: None,
            glob: None,
            offset: None,
            limit: None,
        }
    }

    /// Helper: extract JSON from a successful tool result.
    fn result_json(result: AgentToolResult) -> serde_json::Value {
        match result.content {
            ToolResultContent::Json(v) => v,
            other => panic!("expected JSON content, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn write_then_read() {
        let tool = make_tool();
        // write
        let mut w = input("write");
        w.path = Some("/hello.txt".into());
        w.content = Some("hello world".into());
        tool.run(w).await.unwrap();

        // read
        let mut r = input("read");
        r.path = Some("/hello.txt".into());
        let result = tool.run(r).await.unwrap();
        let json = result_json(result);
        let content = json["content"].as_str().unwrap();
        assert!(content.contains("hello world"));
    }

    #[tokio::test]
    async fn write_then_cat() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("/f.txt".into());
        w.content = Some("line1\nline2\n".into());
        tool.run(w).await.unwrap();

        let mut c = input("cat");
        c.path = Some("/f.txt".into());
        let result = tool.run(c).await.unwrap();
        let json = result_json(result);
        let content = json["content"].as_str().unwrap();
        assert!(content.contains("line1"));
        assert!(!content.contains("\tline1")); // no line-number prefix
    }

    #[tokio::test]
    async fn cp_and_mv() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("/a.txt".into());
        w.content = Some("data".into());
        tool.run(w).await.unwrap();

        // cp
        let mut cp = input("cp");
        cp.src = Some("/a.txt".into());
        cp.dst = Some("/b.txt".into());
        tool.run(cp).await.unwrap();

        // mv b → c
        let mut mv = input("mv");
        mv.src = Some("/b.txt".into());
        mv.dst = Some("/c.txt".into());
        tool.run(mv).await.unwrap();

        // ls should show a.txt and c.txt
        let result = tool.run(input("ls")).await.unwrap();
        let json = result_json(result);
        let names: Vec<&str> = json["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["name"].as_str())
            .collect();
        assert!(names.iter().any(|n| n.contains("a.txt")));
        assert!(names.iter().any(|n| n.contains("c.txt")));
        assert!(!names.iter().any(|n| n.contains("b.txt")));
    }

    #[tokio::test]
    async fn mkdir_stat_rm() {
        let tool = make_tool();

        let mut m = input("mkdir");
        m.path = Some("/mydir".into());
        tool.run(m).await.unwrap();

        let mut s = input("stat");
        s.path = Some("/mydir".into());
        let result = tool.run(s).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["is_dir"], true);

        let mut r = input("rm");
        r.path = Some("/mydir".into());
        tool.run(r).await.unwrap();

        // stat should fail
        let mut s2 = input("stat");
        s2.path = Some("/mydir".into());
        let result = tool.run(s2).await.unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn touch_and_edit() {
        let tool = make_tool();
        let mut t = input("touch");
        t.path = Some("/t.txt".into());
        tool.run(t).await.unwrap();

        let mut w = input("write");
        w.path = Some("/t.txt".into());
        w.content = Some("foo bar baz".into());
        tool.run(w).await.unwrap();

        let mut e = input("edit");
        e.path = Some("/t.txt".into());
        e.old_string = Some("bar".into());
        e.new_string = Some("QUX".into());
        let result = tool.run(e).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["replacements"], 1);

        let mut c = input("cat");
        c.path = Some("/t.txt".into());
        let result = tool.run(c).await.unwrap();
        let json = result_json(result);
        assert!(json["content"].as_str().unwrap().contains("QUX"));
    }

    #[tokio::test]
    async fn head_tail_wc() {
        let tool = make_tool();
        let lines: String = (1..=10).map(|i| format!("line{i}\n")).collect();
        let mut w = input("write");
        w.path = Some("/nums.txt".into());
        w.content = Some(lines);
        tool.run(w).await.unwrap();

        // head 3
        let mut h = input("head");
        h.path = Some("/nums.txt".into());
        h.limit = Some(3);
        let result = tool.run(h).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["lines_returned"], 3);

        // tail 2
        let mut t = input("tail");
        t.path = Some("/nums.txt".into());
        t.limit = Some(2);
        let result = tool.run(t).await.unwrap();
        let json = result_json(result);
        let content = json["content"].as_str().unwrap();
        assert!(content.contains("line10"));
        assert!(!content.contains("line8"));

        // wc
        let mut wc = input("wc");
        wc.path = Some("/nums.txt".into());
        let result = tool.run(wc).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["lines"], 10);
    }

    #[tokio::test]
    async fn grep_finds_content() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("/search.txt".into());
        w.content = Some("hello world\nfoo bar\n".into());
        tool.run(w).await.unwrap();

        let mut g = input("grep");
        g.path = Some("/".into());
        g.pattern = Some("hello".into());
        let result = tool.run(g).await.unwrap();
        let json = result_json(result);
        let matches = json["matches"].as_str().unwrap_or("");
        assert!(matches.contains("search.txt"));
        assert!(matches.contains("hello world"));
    }

    #[tokio::test]
    async fn glob_finds_paths() {
        let tool = make_tool();

        let mut w1 = input("write");
        w1.path = Some("/a.rs".into());
        w1.content = Some(String::new());
        tool.run(w1).await.unwrap();

        let mut w2 = input("write");
        w2.path = Some("/b.txt".into());
        w2.content = Some(String::new());
        tool.run(w2).await.unwrap();

        let mut g = input("glob");
        g.path = Some("/".into());
        g.pattern = Some("*.rs".into());
        let result = tool.run(g).await.unwrap();
        let json = result_json(result);
        let paths: Vec<&str> = json["matches"].as_str().unwrap_or("").lines().collect();
        assert!(paths.iter().any(|p| p.contains("a.rs")));
        assert!(!paths.iter().any(|p| p.contains("b.txt")));
    }

    #[tokio::test]
    async fn unknown_op_errors() {
        let tool = make_tool();
        let mut i = input("chmod");
        i.path = Some("/x".into());
        let result = tool.run(i).await.unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn path_traversal_clamped() {
        let tool = make_tool();
        // /../../../etc/passwd normalizes to /etc/passwd within virtual root
        // — should NOT find real /etc/passwd
        let mut s = input("stat");
        s.path = Some("/../../../etc/passwd".into());
        let result = tool.run(s).await.unwrap();
        // Will error because /etc/passwd doesn't exist in the temp dir
        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn ls_truncates_and_reports_truncated() {
        let tool = make_tool();
        // Create 50 files — more than the explicit limit of 10.
        for i in 0..50 {
            let mut w = input("write");
            w.path = Some(format!("/f{i:02}.txt"));
            w.content = Some(String::new());
            tool.run(w).await.unwrap();
        }

        let mut ls = input("ls");
        ls.path = Some("/".into());
        ls.recursive = Some(false);
        ls.limit = Some(10);
        let result = tool.run(ls).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["returned"].as_u64().unwrap(), 10);
        assert_eq!(json["truncated"], true);
        assert_eq!(json["next_offset"].as_u64().unwrap(), 11);
        let entries = json["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 10);
    }

    #[tokio::test]
    async fn ls_full_page_reports_not_truncated() {
        let tool = make_tool();
        // Create exactly 10 files and ask for 10 — should NOT be truncated.
        for i in 0..10 {
            let mut w = input("write");
            w.path = Some(format!("/g{i:02}.txt"));
            w.content = Some(String::new());
            tool.run(w).await.unwrap();
        }

        let mut ls = input("ls");
        ls.path = Some("/".into());
        ls.recursive = Some(false);
        ls.limit = Some(10);
        let result = tool.run(ls).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["returned"].as_u64().unwrap(), 10);
        assert_eq!(json["truncated"], false);
        assert!(json.get("next_offset").is_none());
    }

    #[tokio::test]
    async fn ls_offset_pagination() {
        let tool = make_tool();
        for i in 0..30 {
            let mut w = input("write");
            w.path = Some(format!("/p{i:02}.txt"));
            w.content = Some(String::new());
            tool.run(w).await.unwrap();
        }

        // Page 1: offset 0, limit 10
        let mut ls = input("ls");
        ls.path = Some("/".into());
        ls.recursive = Some(false);
        ls.limit = Some(10);
        ls.offset = Some(0);
        let page1 = result_json(tool.run(ls).await.unwrap());
        assert_eq!(page1["returned"].as_u64().unwrap(), 10);
        assert_eq!(page1["truncated"], true);

        // Page 2: offset 10, limit 10
        let mut ls = input("ls");
        ls.path = Some("/".into());
        ls.recursive = Some(false);
        ls.limit = Some(10);
        ls.offset = Some(10);
        let page2 = result_json(tool.run(ls).await.unwrap());
        assert_eq!(page2["returned"].as_u64().unwrap(), 10);

        // Pages should not overlap: first entry name of page2 differs from
        // last entry name of page1.
        let p1_last = page1["entries"].as_array().unwrap().last().unwrap()["name"]
            .as_str()
            .unwrap();
        let p2_first = page2["entries"].as_array().unwrap().first().unwrap()["name"]
            .as_str()
            .unwrap();
        assert_ne!(p1_last, p2_first);

        // Last page: offset 20, limit 10 — only 10 remain, not truncated.
        let mut ls = input("ls");
        ls.path = Some("/".into());
        ls.recursive = Some(false);
        ls.limit = Some(10);
        ls.offset = Some(20);
        let page3 = result_json(tool.run(ls).await.unwrap());
        assert_eq!(page3["returned"].as_u64().unwrap(), 10);
        assert_eq!(page3["truncated"], false);
    }

    #[tokio::test]
    async fn read_refuses_binary_nul_bytes() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("/blob.bin".into());
        // NUL bytes are a strong binary signal
        w.content = Some("AB\x00CD\x00EF".into());
        tool.run(w).await.unwrap();

        for op_name in ["read", "cat", "head", "tail"] {
            let mut r = input(op_name);
            r.path = Some("/blob.bin".into());
            let result = tool.run(r).await.unwrap();
            assert_eq!(
                result.is_error,
                Some(true),
                "{op_name} should refuse binary"
            );
        }
    }

    #[tokio::test]
    async fn read_refuses_binary_control_heavy() {
        let tool = make_tool();
        // Build bytes that are mostly non-text control chars (no NUL),
        // exceeding the 30 % threshold but below 100 % to make the
        // ratio-based path decisive.
        let mut bytes = Vec::new();
        for _ in 0..100 {
            bytes.push(0x01); // SOH — control byte
        }
        for _ in 0..50 {
            bytes.push(b'A'); // printable
        }

        let mut w = input("write");
        w.path = Some("/ctrl.bin".into());
        w.content = Some(String::from_utf8_lossy(&bytes).into_owned());
        tool.run(w).await.unwrap();

        let mut r = input("read");
        r.path = Some("/ctrl.bin".into());
        let result = tool.run(r).await.unwrap();
        assert_eq!(result.is_error, Some(true));
    }
    // ─── rm safety contract (Bug 3 + Bug 4) ────────────────────────

    #[tokio::test]
    async fn rm_nonexistent_returns_error() {
        let tool = make_tool();
        let mut r = input("rm");
        r.path = Some("/does_not_exist.txt".into());
        let result = tool.run(r).await.unwrap();
        assert_eq!(result.is_error, Some(true));
        let s = format!("{:?}", result.content);
        assert!(
            s.contains("No such file"),
            "expected 'No such file' in error, got: {s}"
        );
    }

    #[tokio::test]
    async fn rm_root_refused() {
        let tool = make_tool();
        let mut r = input("rm");
        r.path = Some("/".into());
        r.recursive = Some(true);
        let result = tool.run(r).await.unwrap();
        assert_eq!(result.is_error, Some(true));
        let s = format!("{:?}", result.content);
        assert!(
            s.contains("virtual filesystem root") || s.to_lowercase().contains("root"),
            "expected root-refusal message, got: {s}"
        );
    }

    /// An empty-string path must be rejected at the validation layer
    /// before it reaches `normalize_path`, which would otherwise turn it
    /// into `/` and attempt a destructive delete on the virtual root.
    #[tokio::test]
    async fn write_empty_path_is_rejected() {
        let tool = make_tool();

        // `path: Some("")` — the exact input that caused the
        // "failed to clear / => directory not empty" bug.
        let mut w = input("write");
        w.path = Some(String::new());
        w.content = Some("data".into());
        let result = tool.run(w).await;
        assert!(result.is_err(), "expected error for empty path");
        let s = format!("{result:?}");
        assert!(
            s.contains("missing or empty"),
            "expected 'missing or empty' in error, got: {s}"
        );
    }

    #[tokio::test]
    async fn write_whitespace_only_path_is_rejected() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("   ".into());
        w.content = Some("data".into());
        let result = tool.run(w).await;
        assert!(
            result.is_err(),
            "expected error for whitespace-only path"
        );
    }

    #[tokio::test]
    async fn cp_empty_dst_is_rejected() {
        let tool = make_tool();
        let mut c = input("cp");
        c.src = Some("/src.txt".into());
        c.dst = Some(String::new());
        let result = tool.run(c).await;
        assert!(result.is_err(), "expected error for empty dst");
    }

    #[tokio::test]
    async fn edit_empty_path_is_rejected() {
        let tool = make_tool();
        let mut e = input("edit");
        e.path = Some(String::new());
        e.old_string = Some("a".into());
        e.new_string = Some("b".into());
        let result = tool.run(e).await.unwrap();
        assert_eq!(result.is_error, Some(true));
        let s = format!("{:?}", result.content);
        assert!(
            s.contains("missing or empty"),
            "expected 'missing or empty' in error, got: {s}"
        );
    }

    #[tokio::test]
    async fn rm_empty_dir_succeeds_without_flag() {
        let tool = make_tool();
        let mut m = input("mkdir");
        m.path = Some("/empty".into());
        tool.run(m).await.unwrap();

        let mut r = input("rm");
        r.path = Some("/empty".into());
        // no recursive flag
        let result = tool.run(r).await.unwrap();
        assert_eq!(result.is_error, None, "empty-dir rm must succeed");
        let json = result_json(result);
        assert_eq!(json["deleted"], true);
        assert_eq!(json["recursive"], false);
    }

    #[tokio::test]
    async fn rm_nonempty_dir_fails_without_recursive() {
        let tool = make_tool();
        let mut m = input("mkdir");
        m.path = Some("/full".into());
        tool.run(m).await.unwrap();
        let mut w = input("write");
        w.path = Some("/full/a.txt".into());
        w.content = Some("data".into());
        tool.run(w).await.unwrap();

        let mut r = input("rm");
        r.path = Some("/full".into());
        // no recursive flag — must NOT wipe the subtree
        let result = tool.run(r).await.unwrap();
        assert_eq!(result.is_error, Some(true));

        // /full/a.txt must still exist
        let mut s = input("stat");
        s.path = Some("/full/a.txt".into());
        let result = tool.run(s).await.unwrap();
        assert_eq!(
            result.is_error, None,
            "non-empty-dir rm must not delete contents"
        );
    }

    #[tokio::test]
    async fn rm_nonempty_dir_succeeds_with_recursive_true() {
        let tool = make_tool();
        let mut m = input("mkdir");
        m.path = Some("/full2".into());
        tool.run(m).await.unwrap();
        let mut w = input("write");
        w.path = Some("/full2/a.txt".into());
        w.content = Some("data".into());
        tool.run(w).await.unwrap();

        let mut r = input("rm");
        r.path = Some("/full2".into());
        r.recursive = Some(true);
        let result = tool.run(r).await.unwrap();
        assert_eq!(result.is_error, None);
        let json = result_json(result);
        assert_eq!(json["deleted"], true);
        assert_eq!(json["recursive"], true);
    }

    #[tokio::test]
    async fn rm_file_succeeds() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("/f.txt".into());
        w.content = Some("x".into());
        tool.run(w).await.unwrap();

        let mut r = input("rm");
        r.path = Some("/f.txt".into());
        let result = tool.run(r).await.unwrap();
        assert_eq!(result.is_error, None);

        // subsequent rm on the same file must now error
        let mut r2 = input("rm");
        r2.path = Some("/f.txt".into());
        let result = tool.run(r2).await.unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    // ─── cat/read error handling (Bug 1 + Bug 2) ───────────────────

    #[tokio::test]
    async fn cat_nonexistent_returns_error() {
        let tool = make_tool();
        let mut c = input("cat");
        c.path = Some("/no_such_file_xyz".into());
        let result = tool.run(c).await.unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn cat_directory_returns_error() {
        let tool = make_tool();
        let mut m = input("mkdir");
        m.path = Some("/d".into());
        tool.run(m).await.unwrap();

        let mut c = input("cat");
        c.path = Some("/d".into());
        let result = tool.run(c).await.unwrap();
        assert_eq!(result.is_error, Some(true));
        let s = format!("{:?}", result.content);
        assert!(
            s.contains("is a directory"),
            "expected 'is a directory', got: {s}"
        );
    }

    #[tokio::test]
    async fn read_nonexistent_returns_error() {
        let tool = make_tool();
        let mut r = input("read");
        r.path = Some("/nope_xyz".into());
        let result = tool.run(r).await.unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn cat_empty_file_returns_empty_marker() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("/empty.txt".into());
        w.content = Some(String::new());
        tool.run(w).await.unwrap();

        let mut c = input("cat");
        c.path = Some("/empty.txt".into());
        let result = tool.run(c).await.unwrap();
        assert_eq!(result.is_error, None);
        let json = result_json(result);
        assert_eq!(json["content"], "(file is empty)");
        assert_eq!(json["total_size"], 0);
    }

    // ─── write atomicity (Bug 5 + Bug 6) ───────────────────────────

    #[tokio::test]
    async fn write_overwrite_growing_payloads_consistent() {
        let tool = make_tool();
        // 10 successive overwrites, each longer than the previous one.
        // Old code could sporadically fail with "writer got too little data".
        for i in 1..=10 {
            let payload = "x".repeat(i * 16);
            let mut w = input("write");
            w.path = Some("/grow.txt".into());
            w.content = Some(payload.clone());
            let result = tool.run(w).await.unwrap();
            assert_eq!(
                result.is_error,
                None,
                "write #{i} of {} bytes failed",
                payload.len()
            );

            let mut c = input("cat");
            c.path = Some("/grow.txt".into());
            let result = tool.run(c).await.unwrap();
            let json = result_json(result);
            assert_eq!(
                json["content"].as_str().unwrap().len(),
                payload.len(),
                "content length mismatch on write #{i}"
            );
        }
    }

    // ─── ls default non-recursive (问题 8) ───────────────────────────

    #[tokio::test]
    async fn ls_default_is_non_recursive() {
        let tool = make_tool();
        // Set up: top-level file + nested file
        let mut w1 = input("write");
        w1.path = Some("/top.txt".into());
        w1.content = Some("top".into());
        tool.run(w1).await.unwrap();

        let mut w2 = input("write");
        w2.path = Some("/dir/nested.txt".into());
        w2.content = Some("nested".into());
        tool.run(w2).await.unwrap();

        // Default (no recursive flag) must NOT see the nested entry
        let mut ls = input("ls");
        ls.path = Some("/".into());
        let result = tool.run(ls).await.unwrap();
        let json = result_json(result);
        let names: Vec<String> = json["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["name"].as_str().map(String::from))
            .collect();
        assert!(
            names.iter().any(|n| n.ends_with("top.txt")),
            "expected top.txt at depth 0, got: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.ends_with("nested.txt")),
            "default ls must not descend into subdirs, got: {names:?}"
        );
    }

    #[tokio::test]
    async fn ls_recursive_true_descends() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("/dir/nested.txt".into());
        w.content = Some("nested".into());
        tool.run(w).await.unwrap();

        let mut ls = input("ls");
        ls.path = Some("/".into());
        ls.recursive = Some(true);
        let result = tool.run(ls).await.unwrap();
        let json = result_json(result);
        let names: Vec<String> = json["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["name"].as_str().map(String::from))
            .collect();
        assert!(
            names.iter().any(|n| n.ends_with("nested.txt")),
            "recursive ls should include nested files, got: {names:?}"
        );
    }

    // ─── tree standard tree characters (问题 9) ─────────────────────

    #[tokio::test]
    async fn tree_uses_standard_tree_chars() {
        let tool = make_tool();
        let mut m = input("mkdir");
        m.path = Some("/d".into());
        tool.run(m).await.unwrap();
        let mut w = input("write");
        w.path = Some("/d/a.txt".into());
        w.content = Some("a".into());
        tool.run(w).await.unwrap();
        let mut w = input("write");
        w.path = Some("/d/b.txt".into());
        w.content = Some("b".into());
        tool.run(w).await.unwrap();

        let mut t = input("tree");
        t.path = Some("/".into());
        let result = tool.run(t).await.unwrap();
        let json = result_json(result);
        let content = json["content"].as_str().unwrap();
        assert!(
            content.contains("├── ") || content.contains("└── "),
            "expected tree connectors in: {content}"
        );
        assert!(content.contains("d/"), "expected 'd/' marker in: {content}");
        assert!(content.contains("a.txt"), "expected a.txt in: {content}");
        assert!(content.contains("b.txt"), "expected b.txt in: {content}");
    }

    #[tokio::test]
    async fn tree_root_not_duplicated() {
        let tool = make_tool();
        let mut t = input("tree");
        t.path = Some("/".into());
        let result = tool.run(t).await.unwrap();
        let json = result_json(result);
        let content = json["content"].as_str().unwrap();
        // Root must appear exactly once (as the header), not as a child
        let count = content
            .matches(
                "
/",
            )
            .count();
        assert!(
            count <= 1,
            "root '/' should not be duplicated as child, got content:\n{content}"
        );
    }

    // ─── head/tail offset semantics (问题 10) ────────────────────────

    #[tokio::test]
    async fn head_offset_skips_lines() {
        let tool = make_tool();
        let lines: String = (1..=10).map(|i| format!("line{i}\n")).collect();
        let mut w = input("write");
        w.path = Some("/nums.txt".into());
        w.content = Some(lines);
        tool.run(w).await.unwrap();

        let mut h = input("head");
        h.path = Some("/nums.txt".into());
        h.offset = Some(4);
        h.limit = Some(3);
        let result = tool.run(h).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["start_line"], 4);
        assert_eq!(json["lines_returned"], 3);
        let content = json["content"].as_str().unwrap();
        assert!(content.contains("line4"));
        assert!(content.contains("line6"));
        assert!(!content.contains("line3"));
    }

    #[tokio::test]
    async fn tail_offset_is_ignored() {
        let tool = make_tool();
        let lines: String = (1..=10).map(|i| format!("line{i}\n")).collect();
        let mut w = input("write");
        w.path = Some("/nums.txt".into());
        w.content = Some(lines);
        tool.run(w).await.unwrap();

        let mut t = input("tail");
        t.path = Some("/nums.txt".into());
        t.offset = Some(5); // should be ignored
        t.limit = Some(2);
        let result = tool.run(t).await.unwrap();
        let json = result_json(result);
        assert_eq!(
            json["offset_ignored"], true,
            "tail must flag offset_ignored when caller passed one"
        );
        let content = json["content"].as_str().unwrap();
        // tail 2 = last 2 lines = line9 and line10
        assert!(content.contains("line10"));
        assert!(content.contains("line9"));
        assert!(!content.contains("line8"));
    }

    #[tokio::test]
    async fn head_nonexistent_returns_error() {
        let tool = make_tool();
        let mut h = input("head");
        h.path = Some("/nope_head".into());
        let result = tool.run(h).await.unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn tail_nonexistent_returns_error() {
        let tool = make_tool();
        let mut t = input("tail");
        t.path = Some("/nope_tail".into());
        let result = tool.run(t).await.unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn read_allows_cjk_and_emoji() {
        let tool = make_tool();
        // High bytes from valid UTF-8 (CJK + emoji) must not trip the
        // binary detector.
        let text = "你好，世界！😀🎉\nline two\n";
        let mut w = input("write");
        w.path = Some("/cjk.txt".into());
        w.content = Some(text.into());
        tool.run(w).await.unwrap();

        let mut r = input("cat");
        r.path = Some("/cjk.txt".into());
        let result = tool.run(r).await.unwrap();
        let json = result_json(result);
        assert!(json["content"].as_str().unwrap().contains("你好"));
    }
}
