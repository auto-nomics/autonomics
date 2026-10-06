//! VFS Bash — structured file operations through OpenDAL, no system shell.
//!
//! A single tool (`vfs`) dispatches on an `op` field to one of many pure-Rust
//! file-operation handlers. Every handler talks directly to the OpenDAL
//! [`Operator`]; no subprocess is ever spawned.

mod ops;
mod search;

use std::sync::Arc;

use agentik_core::tools::truncation::{DEFAULT_MAX_CHARS, TruncationConfig, truncate_tool_output};
use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::{
    ToolImageSource, ToolResult as AgentToolResult, ToolResultBlock, ToolResultContent,
};
use async_trait::async_trait;

use crate::storage::OpendalFileStorage;

/// Leave room for the outer ToolResult JSON and error/type metadata.
const VFS_OUTPUT_MARGIN: usize = 256;

// ────────────────────────── input ──────────────────────────

#[tool(
    name = "vfs",
    description = "Virtual filesystem operations through OpenDAL VFS. \
        All operations execute in pure Rust — no system shell is spawned. \
        Supported ops: read, cat, ls, cp, mv, rm, mkdir, stat, touch, \
        write, edit, patch, head, tail, wc, grep, glob, tree, mount_list. \
        Paths may be covered by VFS mounts (see mount_list). Read-only \
        mounts reject writes. \
        Unsupported (will error): chmod, chown, ln, pipes, redirects."
)]
pub struct VfsBashInput {
    #[desc = "Operation: read|cat|ls|cp|mv|rm|mkdir|stat|touch|write|edit|patch|head|tail|wc|grep|glob|tree|mount_list"]
    pub op: String,
    #[desc = "Primary path (file or directory)."]
    pub path: Option<String>,
    #[desc = "Source path for cp/mv."]
    pub src: Option<String>,
    #[desc = "Destination path for cp/mv."]
    pub dst: Option<String>,
    #[desc = "Content to write (for write op)."]
    pub content: Option<String>,
    #[desc = "Text to find (for edit op). Matched as whole lines, or as a \
        substring when it occurs within one long line, with fuzzy tolerance: \
        exact, whitespace-trim, and Unicode-normalised."]
    pub old_string: Option<String>,
    #[desc = "Replacement text (for edit op)."]
    pub new_string: Option<String>,
    #[desc = "Replace all occurrences (for edit op). Default false."]
    pub replace_all: Option<bool>,
    #[desc = "Codex-format patch text (for patch op). Multi-file add/delete/update \
        with fuzzy line matching. Format: '*** Begin Patch\\n*** Update File: path\\n@@\\n-old\\n+new\\n*** End Patch'"]
    pub patch: Option<String>,
    #[desc = "List recursively (for ls/tree). Default varies by op."]
    pub recursive: Option<bool>,
    #[desc = "Regex pattern (for grep, searches a directory tree or a single \
        file) or glob pattern (for glob op)."]
    pub pattern: Option<String>,
    #[desc = "Glob filter to narrow grep file candidates, e.g. '*.rs'."]
    pub glob: Option<String>,
    #[desc = "Grep output mode: 'content' (matching lines, default), \
        'files_with_matches' (just filenames with hits), or \
        'count' (per-file match counts)."]
    pub output_mode: Option<String>,
    #[desc = "Context lines before each match (grep -B). Content mode only."]
    pub before: Option<usize>,
    #[desc = "Context lines after each match (grep -A). Content mode only."]
    pub after: Option<usize>,
    #[desc = "Case-insensitive matching for grep. Default: smart-case — \
        auto-insensitive when pattern has no uppercase letters. \
        Set true to force insensitive, false to force sensitive."]
    pub case_insensitive: Option<bool>,
    #[desc = "Starting line number, 1-indexed (for cat/read/head/tail), \
        or number of entries to skip (for ls/tree pagination)."]
    pub offset: Option<usize>,
    #[desc = "Max lines/entries to return (for cat/read/head/tail/ls/tree). \
        ls defaults to 200 and tree defaults to 500; use with offset to page \
        through large listings. tree is capped at 1000 entries per response."]
    pub limit: Option<usize>,
    #[desc = "Maximum file size in bytes for cat. Defaults to 10 MiB; values \
        above 10 MiB are rejected so large files are never fully downloaded."]
    pub max_bytes: Option<usize>,
}

// ────────────────────────── tool ──────────────────────────

pub struct VfsBashTool {
    pub storage: Arc<OpendalFileStorage>,
}

#[async_trait]
impl ToolFunction for VfsBashTool {
    type Input = VfsBashInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let storage = &self.storage;

        let result = match input.op.as_str() {
            // ── introspection ──
            "mount_list" => ops::op_mount_list(storage).await,

            // ── reading ──
            "read" => ops::op_read(storage, input.path.as_deref(), input.offset, input.limit).await,
            "cat" => {
                ops::op_cat(
                    storage,
                    input.path.as_deref(),
                    input.offset,
                    input.limit,
                    input.max_bytes,
                )
                .await
            }
            "head" => ops::op_head(storage, input.path.as_deref(), input.offset, input.limit).await,
            "tail" => ops::op_tail(storage, input.path.as_deref(), input.offset, input.limit).await,

            // ── writing ──
            "write" => {
                ops::op_write(storage, input.path.as_deref(), input.content.as_deref()).await
            }
            "edit" => {
                ops::op_edit(
                    storage,
                    input.path.as_deref(),
                    input.old_string.as_deref(),
                    input.new_string.as_deref(),
                    input.replace_all,
                )
                .await
            }
            "touch" => ops::op_touch(storage, input.path.as_deref()).await,
            "patch" => ops::op_patch(storage, input.patch.as_deref()).await,

            // ── filesystem ──
            "ls" => {
                ops::op_ls(
                    storage,
                    input.path.as_deref(),
                    input.recursive,
                    input.limit,
                    input.offset,
                )
                .await
            }
            "stat" => ops::op_stat(storage, input.path.as_deref()).await,
            "mkdir" => ops::op_mkdir(storage, input.path.as_deref()).await,
            "rm" => ops::op_rm(storage, input.path.as_deref(), input.recursive).await,
            "cp" => ops::op_cp(storage, input.src.as_deref(), input.dst.as_deref()).await,
            "mv" => ops::op_mv(storage, input.src.as_deref(), input.dst.as_deref()).await,
            "wc" => ops::op_wc(storage, input.path.as_deref()).await,
            "tree" => ops::op_tree(storage, input.path.as_deref(), input.limit, input.offset).await,

            // ── search ──
            "grep" => {
                search::op_grep(
                    storage,
                    input.path.as_deref(),
                    input.pattern.as_deref(),
                    input.glob.as_deref(),
                    input.output_mode.as_deref(),
                    input.before,
                    input.after,
                    input.case_insensitive,
                )
                .await
            }
            "glob" => {
                search::op_glob(storage, input.path.as_deref(), input.pattern.as_deref()).await
            }

            // ── unsupported ──
            other => Ok(AgentToolResult::error(format!(
                "Unknown or unsupported operation '{other}'. \
                 Supported: read cat ls cp mv rm mkdir stat touch write edit patch \
                 head tail wc grep glob tree mount_list."
            ))),
        };

        result
            .map(bound_vfs_output)
            .map_err(|error| bound_tool_error(&error))
    }
}

// ────────────────────────── registration ──────────────────────────

/// Apply the system-wide character budget to every VFS operation.
///
/// JSON that already fits is returned unchanged. Oversized JSON is represented
/// as a valid JSON object containing a truncated rendering of the original
/// payload, keeping the outer tool result structured while guaranteeing that
/// the serialized response stays within `DEFAULT_MAX_CHARS`. Oversized image
/// blocks are omitted with an explicit marker rather than sent as base64.
fn bound_vfs_output(mut result: AgentToolResult) -> AgentToolResult {
    let config = output_config();
    match &mut result.content {
        ToolResultContent::Text(content) => {
            *content = truncate_tool_output(content, &config).content;
        }
        ToolResultContent::Json(value) => {
            if let Ok(serialized) = serde_json::to_string(value) {
                if serialized.chars().count() > DEFAULT_MAX_CHARS - VFS_OUTPUT_MARGIN {
                    *value = truncated_json_payload(&serialized, &config);
                }
            }
        }
        ToolResultContent::Blocks(blocks) => {
            if blocks.len() == 1
                && matches!(blocks[0], ToolResultBlock::Image { .. })
                && image_data_chars(&blocks[0]) > DEFAULT_MAX_CHARS - VFS_OUTPUT_MARGIN
            {
                let chars = image_data_chars(&blocks[0]);
                let omitted = format!(
                    "[image omitted: base64 expansion was {chars} characters; \
                     limit is {DEFAULT_MAX_CHARS}]"
                );
                result.content = ToolResultContent::Text(omitted);
            } else {
                let joined = blocks
                    .iter()
                    .map(|block| match block {
                        ToolResultBlock::Text { text } => text.as_str(),
                        ToolResultBlock::Image { .. } => "[image omitted]",
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let bounded = truncate_tool_output(&joined, &config);
                blocks.clear();
                blocks.push(ToolResultBlock::text(bounded.content));
            }
        }
    }
    result
}

fn truncated_json_payload(serialized: &str, config: &TruncationConfig) -> serde_json::Value {
    let mut content = truncate_tool_output(serialized, config).content;
    let mut payload = serde_json::json!({
        "output_truncated": true,
        "content": content,
    });

    // JSON escaping can make nested text longer than its raw character count.
    // Shrink it until the complete serialized wrapper also fits.
    while serde_json::to_string(&payload)
        .map(|bounded| bounded.chars().count() > DEFAULT_MAX_CHARS - VFS_OUTPUT_MARGIN)
        .unwrap_or(true)
    {
        let cut = content.chars().count().saturating_sub(128).max(1);
        if cut == content.chars().count() {
            content.clear();
        } else {
            content = content.chars().take(cut).collect();
        }
        payload = serde_json::json!({
            "output_truncated": true,
            "content": content,
        });
        if content.is_empty() {
            break;
        }
    }
    payload
}

fn image_data_chars(block: &ToolResultBlock) -> usize {
    let ToolResultBlock::Image { source } = block else {
        return 0;
    };
    let ToolImageSource::Base64 { data, .. } = source;
    data.chars().count()
}

fn bound_tool_error(error: &ToolError) -> ToolError {
    let config = output_config();
    let bounded = |value: &str| truncate_tool_output(value, &config).content;

    match error {
        ToolError::NotFound { name } => ToolError::NotFound {
            name: bounded(name),
        },
        ToolError::ValidationFailed { message } => ToolError::ValidationFailed {
            message: bounded(message),
        },
        ToolError::ExecutionFailed { source } => ToolError::ExecutionFailed {
            source: Box::new(BoundedToolError(bounded(&source.to_string()))),
        },
        ToolError::Timeout { seconds } => ToolError::Timeout { seconds: *seconds },
        ToolError::Cancel => ToolError::Cancel,
        ToolError::RegistryError { message } => ToolError::RegistryError {
            message: bounded(message),
        },
    }
}

fn output_config() -> TruncationConfig {
    TruncationConfig {
        max_chars: DEFAULT_MAX_CHARS - VFS_OUTPUT_MARGIN,
        ..TruncationConfig::default()
    }
}

#[derive(Debug)]
struct BoundedToolError(String);

impl std::fmt::Display for BoundedToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BoundedToolError {}

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
    use crate::permission::{
        MountPermissions, VfsAccess, VfsMode, VfsOwnership, VfsPathRule, VfsPrincipal,
    };
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
            patch: None,
            recursive: None,
            pattern: None,
            glob: None,
            output_mode: None,
            before: None,
            after: None,
            case_insensitive: None,
            offset: None,
            limit: None,
            max_bytes: None,
        }
    }

    #[tokio::test]
    async fn recursive_tools_hide_denied_paths() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".git")).unwrap();
        std::fs::write(root.path().join(".git").join("HEAD"), "needle\n").unwrap();
        std::fs::write(root.path().join("README.md"), "safe needle\n").unwrap();
        let deny_all = |path: &str| VfsPathRule::Deny {
            path: path.to_string(),
            access: vec![VfsAccess::Read, VfsAccess::Write, VfsAccess::Execute],
        };
        let manifest = crate::VfsManifest {
            backend: vec![crate::BackendDefinition {
                id: "secure".into(),
                config: crate::BackendConfig::local(root.path().to_string_lossy().into_owned()),
            }],
            mount: vec![crate::MountDefinition {
                path: "/secure".into(),
                backend: "secure".into(),
                source: "/".into(),
                read_only: false,
                permissions: MountPermissions::unix(
                    VfsOwnership { uid: 0, gid: 100 },
                    VfsMode::from_bits(0o777),
                    VfsMode::from_bits(0o666),
                    VfsMode::from_bits(0o777),
                )
                .with_rules(vec![
                    deny_all(".git"),
                    deny_all(".git/**"),
                    deny_all("**/.git"),
                    deny_all("**/.git/**"),
                ]),
            }],
        };
        let mounts = Arc::new(crate::MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = OpendalFileStorage::with_mounts(root.path(), mounts)
            .with_principal(VfsPrincipal::plugin_developer(10_000));
        let tool = VfsBashTool {
            storage: Arc::new(storage),
        };

        let mut ls = input("ls");
        ls.path = Some("/secure".into());
        ls.recursive = Some(true);
        let rendered = result_json(tool.run(ls).await.unwrap()).to_string();
        assert!(
            !rendered.contains(".git"),
            "ls leaked denied path: {rendered}"
        );

        let mut grep = input("grep");
        grep.path = Some("/secure".into());
        grep.pattern = Some("needle".into());
        let rendered = result_json(tool.run(grep).await.unwrap()).to_string();
        assert!(
            rendered.contains("README.md"),
            "expected safe match: {rendered}"
        );
        assert!(
            !rendered.contains(".git"),
            "grep leaked denied path: {rendered}"
        );

        let mut tree = input("tree");
        tree.path = Some("/secure".into());
        let rendered = result_json(tool.run(tree).await.unwrap()).to_string();
        assert!(
            !rendered.contains(".git"),
            "tree leaked denied path: {rendered}"
        );
    }

    /// Helper: extract JSON from a successful tool result.
    fn result_json(result: AgentToolResult) -> serde_json::Value {
        match result.content {
            ToolResultContent::Json(v) => v,
            other => panic!("expected JSON content, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn json_operations_enforce_serialized_character_limit() {
        let tool = make_tool();
        let mut write = input("write");
        write.path = Some("/large.txt".into());
        write.content = Some("x".repeat(60_000));
        tool.run(write).await.unwrap();

        let mut cat = input("cat");
        cat.path = Some("/large.txt".into());
        let result = tool.run(cat).await.unwrap();
        let serialized = serde_json::to_string(&result).unwrap();
        let json = result_json(result);

        assert_eq!(json["output_truncated"], serde_json::json!(true));
        assert!(
            serialized.chars().count() <= DEFAULT_MAX_CHARS,
            "serialized JSON has {} characters",
            serialized.chars().count()
        );
        assert!(json["content"].as_str().unwrap().contains("x"));
    }

    #[tokio::test]
    async fn oversized_image_is_omitted_instead_of_expanding_context() {
        let tool = make_tool();
        let mut write = input("write");
        write.path = Some("/large.png".into());
        write.content = Some("x".repeat(60_000));
        tool.run(write).await.unwrap();

        let mut read = input("read");
        read.path = Some("/large.png".into());
        let result = tool.run(read).await.unwrap();

        match result.content {
            ToolResultContent::Text(text) => {
                assert!(text.contains("[image omitted: base64 expansion was"));
                assert!(text.contains("limit is 50000]"));
            }
            other => panic!("expected bounded text content, got: {other:?}"),
        }
    }

    #[test]
    fn tool_error_messages_enforce_character_limit() {
        let error = ToolError::ValidationFailed {
            message: "x".repeat(60_000),
        };
        let bounded = bound_tool_error(&error);
        let serialized = serde_json::to_string(&bounded.to_string()).unwrap();

        assert!(serialized.chars().count() <= DEFAULT_MAX_CHARS);
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
        w.content = Some("foo bar baz\n".into());
        tool.run(w).await.unwrap();

        // Line-based edit: old_string must match complete line(s).
        let mut e = input("edit");
        e.path = Some("/t.txt".into());
        e.old_string = Some("foo bar baz".into());
        e.new_string = Some("foo QUX baz".into());
        let result = tool.run(e).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["replacements"], 1);
        assert_eq!(json["fuzzy"], false);

        let mut c = input("cat");
        c.path = Some("/t.txt".into());
        let result = tool.run(c).await.unwrap();
        let json = result_json(result);
        assert!(json["content"].as_str().unwrap().contains("QUX"));
    }

    #[tokio::test]
    async fn edit_fuzzy_whitespace_tolerance() {
        let tool = make_tool();
        // File line has trailing spaces; old_string does not.
        let mut w = input("write");
        w.path = Some("/fw.txt".into());
        w.content = Some("foo  \nbar\n".into());
        tool.run(w).await.unwrap();

        let mut e = input("edit");
        e.path = Some("/fw.txt".into());
        e.old_string = Some("foo".into());
        e.new_string = Some("FOO".into());
        let result = tool.run(e).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["replacements"], 1);
        assert_eq!(json["fuzzy"], true);

        let mut c = input("cat");
        c.path = Some("/fw.txt".into());
        let result = tool.run(c).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["content"].as_str().unwrap(), "FOO\nbar");
    }

    #[tokio::test]
    async fn edit_fuzzy_indentation_tolerance() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("/fi.txt".into());
        w.content = Some("fn main() {\n    println!(\"hi\");\n}\n".into());
        tool.run(w).await.unwrap();

        // old_string omits leading indentation — trim pass should catch it.
        let mut e = input("edit");
        e.path = Some("/fi.txt".into());
        e.old_string = Some("println!(\"hi\");".into());
        e.new_string = Some("    println!(\"bye\");".into());
        let result = tool.run(e).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["replacements"], 1);
        assert_eq!(json["fuzzy"], true);

        let mut c = input("cat");
        c.path = Some("/fi.txt".into());
        let result = tool.run(c).await.unwrap();
        let json = result_json(result);
        assert!(json["content"].as_str().unwrap().contains("bye"));
    }

    #[tokio::test]
    async fn edit_not_found_error() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("/nf.txt".into());
        w.content = Some("hello\nworld\n".into());
        tool.run(w).await.unwrap();

        let mut e = input("edit");
        e.path = Some("/nf.txt".into());
        e.old_string = Some("nonexistent".into());
        e.new_string = Some("X".into());
        let result = tool.run(e).await.unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn edit_replace_all() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("/ra.txt".into());
        w.content = Some("foo\nfoo\nbar\n".into());
        tool.run(w).await.unwrap();

        let mut e = input("edit");
        e.path = Some("/ra.txt".into());
        e.old_string = Some("foo".into());
        e.new_string = Some("X".into());
        e.replace_all = Some(true);
        let result = tool.run(e).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["replacements"], 2);

        let mut c = input("cat");
        c.path = Some("/ra.txt".into());
        let result = tool.run(c).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["content"].as_str().unwrap(), "X\nX\nbar");
    }

    #[tokio::test]
    async fn edit_multi_line_old_string() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("/ml.txt".into());
        w.content = Some("a\nb\nc\nd\n".into());
        tool.run(w).await.unwrap();

        let mut e = input("edit");
        e.path = Some("/ml.txt".into());
        e.old_string = Some("b\nc".into());
        e.new_string = Some("X\nY".into());
        let result = tool.run(e).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["replacements"], 1);

        let mut c = input("cat");
        c.path = Some("/ml.txt".into());
        let result = tool.run(c).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["content"].as_str().unwrap(), "a\nX\nY\nd");
    }

    #[tokio::test]
    async fn patch_update_file() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("/p.txt".into());
        w.content = Some("foo\nbar\nbaz\n".into());
        tool.run(w).await.unwrap();

        let mut p = input("patch");
        p.patch = Some(
            "*** Begin Patch\n\
             *** Update File: /p.txt\n\
             @@\n\
             -bar\n\
             +BAR\n\
             *** End Patch"
                .into(),
        );
        let result = tool.run(p).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["changes"].as_array().unwrap().len(), 1);

        let mut c = input("cat");
        c.path = Some("/p.txt".into());
        let result = tool.run(c).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["content"].as_str().unwrap(), "foo\nBAR\nbaz");
    }

    #[tokio::test]
    async fn patch_add_and_delete_file() {
        let tool = make_tool();

        // Pre-create a file to delete.
        let mut w = input("write");
        w.path = Some("/old.txt".into());
        w.content = Some("bye\n".into());
        tool.run(w).await.unwrap();

        let mut p = input("patch");
        p.patch = Some(
            "*** Begin Patch\n\
             *** Add File: /new.txt\n\
             +hello world\n\
             *** Delete File: /old.txt\n\
             *** End Patch"
                .into(),
        );
        let result = tool.run(p).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["changes"].as_array().unwrap().len(), 2);

        // new.txt exists, old.txt gone.
        let mut c = input("cat");
        c.path = Some("/new.txt".into());
        let result = tool.run(c).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["content"].as_str().unwrap(), "hello world");

        let mut s = input("stat");
        s.path = Some("/old.txt".into());
        let result = tool.run(s).await.unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn patch_move_file() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("/src.txt".into());
        w.content = Some("data\n".into());
        tool.run(w).await.unwrap();

        let mut p = input("patch");
        p.patch = Some(
            "*** Begin Patch\n\
             *** Update File: /src.txt\n\
             *** Move to: /dst.txt\n\
             @@\n\
             -data\n\
             +DATA\n\
             *** End Patch"
                .into(),
        );
        let result = tool.run(p).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["changes"][0]["action"], "move");

        // src removed, dst has new content.
        let mut c = input("cat");
        c.path = Some("/dst.txt".into());
        let result = tool.run(c).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["content"].as_str().unwrap(), "DATA");

        let mut s = input("stat");
        s.path = Some("/src.txt".into());
        let result = tool.run(s).await.unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn patch_fuzzy_matching() {
        let tool = make_tool();
        // File has trailing spaces; patch doesn't include them.
        let mut w = input("write");
        w.path = Some("/fz.txt".into());
        w.content = Some("foo  \nbar\n".into());
        tool.run(w).await.unwrap();

        let mut p = input("patch");
        p.patch = Some(
            "*** Begin Patch\n\
             *** Update File: /fz.txt\n\
             @@\n\
             -foo\n\
             +FOO\n\
             *** End Patch"
                .into(),
        );
        let result = tool.run(p).await.unwrap();
        assert_eq!(result.is_error, None);

        let mut c = input("cat");
        c.path = Some("/fz.txt".into());
        let result = tool.run(c).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["content"].as_str().unwrap(), "FOO\nbar");
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
        assert_eq!(json["next_offset"].as_u64().unwrap(), 10);
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
    async fn ls_offset_returns_final_entry() {
        let tool = make_tool();
        for name in ["one.txt", "two.txt", "three.txt"] {
            let mut w = input("write");
            w.path = Some(format!("/{name}"));
            w.content = Some(String::new());
            tool.run(w).await.unwrap();
        }

        let mut first = input("ls");
        first.path = Some("/".into());
        first.recursive = Some(false);
        first.limit = Some(2);
        let first = result_json(tool.run(first).await.unwrap());
        assert_eq!(first["returned"].as_u64().unwrap(), 2);
        assert_eq!(first["truncated"], true);
        assert_eq!(first["next_offset"].as_u64().unwrap(), 2);

        let mut second = input("ls");
        second.path = Some("/".into());
        second.recursive = Some(false);
        second.limit = Some(2);
        second.offset = Some(2);
        let second = result_json(tool.run(second).await.unwrap());
        assert_eq!(second["returned"].as_u64().unwrap(), 1);
        assert_eq!(second["truncated"], false);
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
        assert!(result.is_err(), "expected error for whitespace-only path");
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

    #[tokio::test]
    async fn cat_rejects_files_over_requested_byte_limit() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("/too-large.txt".into());
        w.content = Some("123456789".into());
        tool.run(w).await.unwrap();

        let mut c = input("cat");
        c.path = Some("/too-large.txt".into());
        c.max_bytes = Some(8);
        let result = tool.run(c).await.unwrap();
        assert_eq!(result.is_error, Some(true));
        let output = format!("{:?}", result.content);
        assert!(
            output.contains("9 bytes (max_bytes: 8)"),
            "expected size-limit error, got: {output}"
        );
    }

    #[tokio::test]
    async fn cat_allows_file_exactly_at_byte_limit() {
        let tool = make_tool();
        let mut w = input("write");
        w.path = Some("/at-limit.txt".into());
        w.content = Some("123456789".into());
        tool.run(w).await.unwrap();

        let mut c = input("cat");
        c.path = Some("/at-limit.txt".into());
        c.max_bytes = Some(9);
        let result = tool.run(c).await.unwrap();
        assert_eq!(result.is_error, None);
        assert_eq!(result_json(result)["content"], "123456789");
    }

    #[tokio::test]
    async fn cat_rejects_invalid_byte_limit() {
        let tool = make_tool();
        let mut c = input("cat");
        c.path = Some("/file.txt".into());
        c.max_bytes = Some(0);
        let err = tool.run(c).await.unwrap_err();
        assert!(
            matches!(err, ToolError::ValidationFailed { .. }),
            "expected validation error, got: {err:?}"
        );
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
    async fn tree_truncates_and_reports_truncated() {
        let tool = make_tool();
        for i in 0..12 {
            let mut w = input("write");
            w.path = Some(format!("/t{i:02}.txt"));
            w.content = Some(String::new());
            tool.run(w).await.unwrap();
        }

        let mut first = input("tree");
        first.path = Some("/".into());
        first.limit = Some(10);
        let first = result_json(tool.run(first).await.unwrap());
        assert_eq!(first["returned"].as_u64().unwrap(), 10);
        assert_eq!(first["truncated"], true);
        assert_eq!(first["next_offset"].as_u64().unwrap(), 10);
        assert!(
            first["content"]
                .as_str()
                .unwrap()
                .contains("next_offset=10")
        );

        let mut second = input("tree");
        second.path = Some("/".into());
        second.limit = Some(10);
        second.offset = Some(10);
        let second = result_json(tool.run(second).await.unwrap());
        assert_eq!(second["returned"].as_u64().unwrap(), 2);
        assert_eq!(second["truncated"], false);
        assert!(second.get("next_offset").is_none());
    }

    #[tokio::test]
    async fn tree_pagination_keeps_ancestor_context() {
        let tool = make_tool();
        for path in [
            "/alpha/beta/one.txt",
            "/alpha/beta/two.txt",
            "/alpha/gamma.txt",
            "/delta.txt",
        ] {
            let mut w = input("write");
            w.path = Some(path.into());
            w.content = Some(String::new());
            tool.run(w).await.unwrap();
        }

        let mut page = input("tree");
        page.path = Some("/".into());
        page.limit = Some(2);
        page.offset = Some(2);
        let page = result_json(tool.run(page).await.unwrap());
        let content = page["content"].as_str().unwrap();
        assert_eq!(page["returned"].as_u64().unwrap(), 2);
        assert_eq!(page["truncated"], true);
        assert_eq!(page["next_offset"].as_u64().unwrap(), 4);
        assert!(
            content.contains("alpha/") && content.contains("beta/") && content.contains("two.txt"),
            "expected ancestor context in tree page: {content}"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn tree_and_size_queries_follow_local_symlink_directories() {
        let root = tempfile::TempDir::new().unwrap();
        let real = root.path().join("real");
        std::fs::create_dir_all(real.join("nested")).unwrap();
        std::fs::write(real.join("nested").join("data.txt"), b"12345").unwrap();
        std::os::unix::fs::symlink(&real, root.path().join("link")).unwrap();

        let tool = VfsBashTool {
            storage: Arc::new(OpendalFileStorage::new(root.path())),
        };

        let mut stat = input("stat");
        stat.path = Some("/link/nested/data.txt".into());
        let result = tool.run(stat).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["is_dir"], false);
        assert_eq!(json["size"], 5, "stat should follow the symlink: {json}");

        let mut ls = input("ls");
        ls.path = Some("/".into());
        let result = tool.run(ls).await.unwrap();
        let json = result_json(result);
        let link = json["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == "link")
            .cloned()
            .unwrap_or_else(|| panic!("expected /link in listing: {json}"));
        assert_eq!(
            link["is_dir"], true,
            "symlink directory should be listed as a directory"
        );
        assert_eq!(
            link["size"], 0,
            "directory size should not expose the symlink target path length"
        );

        let mut tree = input("tree");
        tree.path = Some("/".into());
        let result = tool.run(tree).await.unwrap();
        let json = result_json(result);
        let content = json["content"].as_str().unwrap();
        assert!(
            content.contains("link/")
                && content.contains("nested/")
                && content.contains("data.txt"),
            "tree should descend into symlink directory: {content}"
        );

        let mut linked_tree = input("tree");
        linked_tree.path = Some("/link".into());
        let result = tool.run(linked_tree).await.unwrap();
        let json = result_json(result);
        let content = json["content"].as_str().unwrap();
        assert!(
            content.contains("nested/") && content.contains("data.txt"),
            "tree rooted at a symlink directory should list its target: {json}"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn tree_follows_a_local_mount_source_symlink() {
        use crate::{BackendConfig, BackendDefinition, MountDefinition, VfsManifest};

        let backend_root = tempfile::TempDir::new().unwrap();
        let target = backend_root.path().join("target");
        std::fs::create_dir_all(target.join("nested")).unwrap();
        std::fs::write(target.join("nested").join("data.txt"), b"12345").unwrap();
        let source_link = backend_root.path().join("source-link");
        std::os::unix::fs::symlink(&target, &source_link).unwrap();

        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "local".into(),
                config: BackendConfig::local(backend_root.path().to_string_lossy().to_string()),
            }],
            mount: vec![MountDefinition {
                path: "/mounted".into(),
                backend: "local".into(),
                source: source_link.to_string_lossy().to_string(),
                read_only: true,
                permissions: MountPermissions::default(),
            }],
        };
        let mounts = Arc::new(crate::MountedObjectStore::from_manifest(&manifest).unwrap());
        let tool = VfsBashTool {
            storage: Arc::new(OpendalFileStorage::with_mounts(
                tempfile::tempdir().unwrap().path(),
                mounts,
            )),
        };

        let mut tree = input("tree");
        tree.path = Some("/mounted".into());
        let result = tool.run(tree).await.unwrap();
        let json = result_json(result);
        let content = json["content"].as_str().unwrap();
        assert!(
            content.contains("nested/") && content.contains("data.txt"),
            "tree should follow a mount source symlink: {content}"
        );

        let mut stat = input("stat");
        stat.path = Some("/mounted/nested/data.txt".into());
        let result = tool.run(stat).await.unwrap();
        let json = result_json(result);
        assert_eq!(json["size"], 5, "stat through mount symlink: {json}");
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

    // ── mounted storage regression (non-empty mount source) ──────────

    /// Build a tool whose root mount maps `/` to a subdirectory of the
    /// backend root — the layout real `vfs.toml` files use. Writes and
    /// reads must land in the mount source, not the process root.
    fn make_mounted_tool() -> (VfsBashTool, tempfile::TempDir, std::path::PathBuf) {
        use crate::{BackendConfig, BackendDefinition, MountDefinition, VfsManifest};

        let backend_root = tempfile::tempdir().unwrap();
        let source_dir = backend_root.path().join("ws");
        std::fs::create_dir_all(&source_dir).unwrap();
        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "default".into(),
                config: BackendConfig::local(backend_root.path().to_string_lossy().to_string()),
            }],
            mount: vec![MountDefinition {
                path: "/".into(),
                backend: "default".into(),
                source: source_dir.to_string_lossy().to_string(),
                read_only: false,
                permissions: MountPermissions::default(),
            }],
        };
        let vfs = Arc::new(crate::MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = Arc::new(OpendalFileStorage::with_mounts(
            tempfile::tempdir().unwrap().path(),
            vfs,
        ));
        let tool = VfsBashTool { storage };
        (tool, backend_root, source_dir)
    }

    /// Build a tool with a writable root mount plus a deeper read-only mount,
    /// matching the common `vfs.toml` layout.
    fn make_data_mounted_tool() -> (VfsBashTool, tempfile::TempDir, std::path::PathBuf) {
        use crate::{BackendConfig, BackendDefinition, MountDefinition, VfsManifest};

        let backend_root = tempfile::tempdir().unwrap();
        let source_dir = backend_root.path().join("source");
        std::fs::create_dir_all(source_dir.join("nested")).unwrap();
        std::fs::write(source_dir.join("panel.parquet"), b"panel").unwrap();
        std::fs::write(source_dir.join("nested/data.csv"), b"data").unwrap();

        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "default".into(),
                config: BackendConfig::local(backend_root.path().to_string_lossy().to_string()),
            }],
            mount: vec![
                MountDefinition {
                    path: "/".into(),
                    backend: "default".into(),
                    source: "/".into(),
                    read_only: false,
                    permissions: MountPermissions::default(),
                },
                MountDefinition {
                    path: "/data/ldsc".into(),
                    backend: "default".into(),
                    source: "source".into(),
                    read_only: true,
                    permissions: MountPermissions::default(),
                },
            ],
        };
        let vfs = Arc::new(crate::MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = Arc::new(OpendalFileStorage::with_mounts(
            tempfile::tempdir().unwrap().path(),
            vfs,
        ));
        (VfsBashTool { storage }, backend_root, source_dir)
    }

    #[tokio::test]
    async fn write_read_ls_through_mount_with_nonempty_source() {
        let (tool, _backend_root, source_dir) = make_mounted_tool();

        let mut w = input("write");
        w.path = Some("/hello.txt".into());
        w.content = Some("hello world".into());
        let res = tool.run(w).await.unwrap();
        assert_ne!(res.is_error, Some(true), "write failed: {res:?}");

        // Bytes must land inside the mount's source directory.
        let on_disk = std::fs::read(source_dir.join("hello.txt")).unwrap();
        assert_eq!(on_disk, b"hello world");

        let mut r = input("cat");
        r.path = Some("/hello.txt".into());
        let result = tool.run(r).await.unwrap();
        let json = result_json(result);
        assert!(json["content"].as_str().unwrap().contains("hello world"));

        let mut ls = input("ls");
        ls.path = Some("/".into());
        let result = tool.run(ls).await.unwrap();
        let json = result_json(result);
        let names: Vec<&str> = json["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(
            names.iter().any(|n| n.ends_with("hello.txt")),
            "expected hello.txt in listing, got: {names:?}"
        );
    }

    #[tokio::test]
    async fn patch_through_mount_preserves_repeated_path_component() {
        let (tool, _backend_root, source_dir) = make_mounted_tool();
        let virtual_path = "/ic_cvd_ml/outputs/patched.md";

        let mut w = input("write");
        w.path = Some(virtual_path.into());
        w.content = Some("old\n".into());
        let result = tool.run(w).await.unwrap();
        assert_ne!(result.is_error, Some(true), "write failed: {result:?}");

        let mut p = input("patch");
        p.patch = Some(
            "*** Begin Patch\n\
             *** Update File: /ic_cvd_ml/outputs/patched.md\n\
             @@\n\
             -old\n\
             +new\n\
             *** End Patch"
                .into(),
        );
        let result = tool.run(p).await.unwrap();
        assert_ne!(result.is_error, Some(true), "patch failed: {result:?}");

        let on_disk = source_dir.join("ic_cvd_ml/outputs/patched.md");
        assert_eq!(std::fs::read_to_string(on_disk).unwrap(), "new\n");
    }

    #[tokio::test]
    async fn ls_parent_of_mount_lists_mount_point_and_contents() {
        let (tool, _backend_root, _source_dir) = make_data_mounted_tool();

        let mut root = input("ls");
        root.path = Some("/".into());
        let result = tool.run(root).await.unwrap();
        let json = result_json(result);
        let names: Vec<&str> = json["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&"/data"),
            "expected synthetic ancestor for deep mount, got: {names:?}"
        );

        let mut ls = input("ls");
        ls.path = Some("/data".into());
        let result = tool.run(ls).await.unwrap();
        let json = result_json(result);
        let names: Vec<&str> = json["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&"/data/ldsc"),
            "expected child mount point, got: {names:?}"
        );

        let mut mounted = input("ls");
        mounted.path = Some("/data/ldsc".into());
        let result = tool.run(mounted).await.unwrap();
        let json = result_json(result);
        let names: Vec<&str> = json["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&"/data/ldsc/panel.parquet"),
            "expected nested mounted file, got: {names:?}"
        );
    }

    #[tokio::test]
    async fn ls_prioritizes_mount_points_when_truncated() {
        let (tool, _backend_root, _source_dir) = make_data_mounted_tool();

        let mut ls = input("ls");
        ls.path = Some("/".into());
        ls.recursive = Some(false);
        ls.limit = Some(1);
        let result = tool.run(ls).await.unwrap();
        let json = result_json(result);
        let entries = json["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["name"], "/data");
        assert_eq!(json["truncated"], true);
    }

    #[tokio::test]
    async fn mkdir_and_rm_through_mount() {
        let (tool, _backend_root, source_dir) = make_mounted_tool();

        let mut m = input("mkdir");
        m.path = Some("/sub".into());
        let res = tool.run(m).await.unwrap();
        assert_ne!(res.is_error, Some(true), "mkdir failed: {res:?}");
        assert!(source_dir.join("sub").is_dir());

        let mut w = input("write");
        w.path = Some("/sub/file.txt".into());
        w.content = Some("data".into());
        let res = tool.run(w).await.unwrap();
        assert_ne!(res.is_error, Some(true), "write failed: {res:?}");

        let mut rm = input("rm");
        rm.path = Some("/sub".into());
        rm.recursive = Some(true);
        let res = tool.run(rm).await.unwrap();
        assert_ne!(res.is_error, Some(true), "rm failed: {res:?}");
        assert!(!source_dir.join("sub").exists());
    }
}
