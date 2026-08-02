use std::cmp::Ordering;
use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;
use futures::StreamExt;

use crate::storage::OpendalFileStorage;

/// Default page size when no explicit `limit` is supplied. Matches
/// `file_read`'s `DEFAULT_MAX_LINES` so the two tools feel symmetric and
/// no single tool call can flood the context window with an unbounded
/// recursive listing.
const DEFAULT_LIST_LIMIT: usize = 200;
/// Hard ceiling on a single page. Even if the LLM asks for more, we cap
/// the response here to keep the tool result well under typical context
/// budgets (200 entries × ~80 chars ≈ 16 KB JSON, comfortably safe).
const MAX_LIST_LIMIT: usize = 1000;

#[derive(Debug)]
#[tool(
    name = "file_list",
    description = "List entries (files and directories) under a path. \
        Returns names, types (file/dir), and sizes. Results are sorted by \
        name for stable pagination. Use `offset` and `limit` to page through \
        large directories; the response includes `total`, `returned`, and \
        `has_more` so you can iterate without re-listing. Default page size \
        is 200; the maximum is 1000 per call. Set `recursive=false` to list \
        only direct children."
)]
pub struct FileListInput {
    #[desc = "Directory path to list. Defaults to \"/\"."]
    pub path: Option<String>,
    #[desc = "List recursively. Defaults to true."]
    pub recursive: Option<bool>,
    #[desc = "Number of entries to skip from the start of the sorted result \
        set (0-indexed). Use together with `limit` to page. Defaults to 0."]
    pub offset: Option<usize>,
    #[desc = "Maximum number of entries to return in this call. Defaults to \
        200; capped at 1000. Subsequent pages can be fetched by raising \
        `offset` by the value of `returned` from the previous call."]
    pub limit: Option<usize>,
}

pub struct FileListTool {
    pub(crate) storage: Arc<OpendalFileStorage>,
}

#[async_trait]
impl ToolFunction for FileListTool {
    type Input = FileListInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let op = &self.storage.op;
        let path = OpendalFileStorage::normalize_path(input.path.as_deref().unwrap_or("/"));
        let recursive = input.recursive.unwrap_or(true);

        // Resolve and clamp pagination parameters up front so the rest of the
        // function can treat `offset`/`limit` as resolved `usize` values.
        // `limit = 0` is allowed (gives a cheap "count only" call returning
        // `total` + empty `entries`), but is bumped to 1 to keep the response
        // shape uniform — callers can pass `offset` past `total` if they only
        // want the count.
        let offset = input.offset.unwrap_or(0);
        let limit_raw = input.limit.unwrap_or(DEFAULT_LIST_LIMIT);
        let limit = limit_raw.clamp(1, MAX_LIST_LIMIT);

        let mut items = if recursive {
            let mut lister = op
                .lister_with(&path)
                .recursive(true)
                .await
                .map_err(|e| e.to_string())?;
            let mut items = Vec::new();
            while let Some(entry) = lister.next().await {
                let entry = entry.map_err(|e| e.to_string())?;
                let entry_path = entry.path().to_string();
                let meta = entry.metadata();
                let is_dir = meta.is_dir();
                // list() may not return accurate content_length; stat each entry
                // to get the real file size.
                let size = if is_dir {
                    0
                } else {
                    op.stat(&entry_path)
                        .await
                        .ok()
                        .map(|m| m.content_length())
                        .unwrap_or(meta.content_length())
                };
                items.push(serde_json::json!({
                    "name": entry_path,
                    "is_dir": is_dir,
                    "size": size,
                }));
            }
            items
        } else {
            // opendal Fs requires a trailing '/' to list children of a directory.
            let list_path = if path.ends_with('/') {
                path.clone()
            } else {
                format!("{path}/")
            };
            let mut lister = op
                .lister_with(&list_path)
                .recursive(false)
                .await
                .map_err(|e| e.to_string())?;
            let mut items = Vec::new();
            while let Some(entry) = lister.next().await {
                let entry = entry.map_err(|e| e.to_string())?;
                let meta = entry.metadata();
                let is_dir = meta.is_dir();
                let entry_path = entry.path().to_string();
                let size = if is_dir {
                    0
                } else {
                    op.stat(&entry_path)
                        .await
                        .ok()
                        .map(|m| m.content_length())
                        .unwrap_or(meta.content_length())
                };
                items.push(serde_json::json!({
                    "name": entry_path,
                    "is_dir": is_dir,
                    "size": size,
                }));
            }
            items
        };

        // Sort the entire collected set by entry name so that `offset` is
        // deterministic across calls. opendal's lister order is not
        // guaranteed (it's whatever the underlying backend returns), and a
        // stable order is required for pagination to be useful: otherwise
        // page N+1 may re-show entries from page N.
        items.sort_by(|a, b| {
            let an = a["name"].as_str().unwrap_or("");
            let bn = b["name"].as_str().unwrap_or("");
            // Directories first (so `..` / subfolders don't get lost at the
            // tail of a page), then alphabetical within each group.
            match (a["is_dir"].as_bool(), b["is_dir"].as_bool()) {
                (Some(true), Some(false)) => Ordering::Less,
                (Some(false), Some(true)) => Ordering::Greater,
                _ => an.cmp(bn),
            }
        });

        let total = items.len();

        // Clamp `offset` past the end to a no-op rather than an error so
        // callers can safely iterate `while has_more { offset += returned }`.
        let page: Vec<serde_json::Value> = if offset >= total {
            Vec::new()
        } else {
            let end = (offset + limit).min(total);
            items[offset..end].to_vec()
        };
        let returned = page.len();
        let has_more = offset + returned < total;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "path": path,
            "entries": page,
            "total": total,
            "offset": offset,
            "limit": limit,
            "returned": returned,
            "has_more": has_more,
            "next_offset": if has_more { Some(offset + returned) } else { Option::<usize>::None },
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_sdk::types::ToolResultContent;

    /// Create a `FileListTool` backed by a temp directory pre-populated
    /// with the given `(path, content)` pairs.
    ///
    /// **Note:** opendal 0.57 Fs backend returns paths like `hello.txt`,
    /// `sub/world.txt`, `sub/` (no leading `/`).  The root is returned as `/`.
    async fn setup_tool(layout: Vec<(&str, &str)>) -> FileListTool {
        let storage = OpendalFileStorage::new_temp();
        for (path, content) in &layout {
            let bytes: Vec<u8> = content.as_bytes().to_vec();
            storage
                .op
                .write(path, opendal::Buffer::from(bytes))
                .await
                .unwrap();
        }
        FileListTool {
            storage: Arc::new(storage),
        }
    }

    /// Extract the JSON value from a successful `ToolResult`.
    fn result_json(result: AgentToolResult) -> serde_json::Value {
        match result.content {
            ToolResultContent::Json(v) => v,
            other => panic!("expected JSON content, got: {other:?}"),
        }
    }

    /// Collect the `name` field from every entry as a sorted `Vec<String>`.
    fn entry_names(entries: &serde_json::Value) -> Vec<String> {
        let mut names: Vec<String> = entries
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["name"].as_str().map(|s| s.to_string()))
            .collect();
        names.sort();
        names
    }

    /// Helper: assert that `entries` contains an entry whose `name` contains
    /// the given substring.
    fn assert_has_entry(entries: &serde_json::Value, substr: &str) {
        let names = entry_names(entries);
        assert!(
            names.iter().any(|n| n.contains(substr)),
            "expected entry containing '{substr}' in {names:?}"
        );
    }

    /// Helper: assert that `entries` does NOT contain an entry whose `name`
    /// contains the given substring.
    fn assert_no_entry(entries: &serde_json::Value, substr: &str) {
        let names = entry_names(entries);
        assert!(
            !names.iter().any(|n| n.contains(substr)),
            "did not expect entry containing '{substr}' in {names:?}"
        );
    }

    // ---------------------------------------------------------------
    // Recursive listing (default behaviour)
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn list_recursive_default() {
        let tool = setup_tool(vec![
            ("hello.txt", "hello"),
            ("sub/world.txt", "world"),
            ("sub/deep/nested.txt", "nested"),
        ])
        .await;

        let result = tool
            .run(FileListInput {
                path: None,
                recursive: None, // defaults to true
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        let json = result_json(result);

        // All three files must appear.
        assert_has_entry(&json["entries"], "hello.txt");
        assert_has_entry(&json["entries"], "sub/world.txt");
        assert_has_entry(&json["entries"], "sub/deep/nested.txt");
    }

    #[tokio::test]
    async fn list_recursive_explicit_true() {
        let tool = setup_tool(vec![("a.txt", "a"), ("dir/b.txt", "b")]).await;

        let result = tool
            .run(FileListInput {
                path: None,
                recursive: Some(true),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        let json = result_json(result);

        assert_has_entry(&json["entries"], "a.txt");
        assert_has_entry(&json["entries"], "dir/b.txt");
    }

    #[tokio::test]
    async fn list_recursive_specific_subdir() {
        let tool = setup_tool(vec![
            ("root/file1.txt", "f1"),
            ("root/sub/file2.txt", "f2"),
            ("other.txt", "other"),
        ])
        .await;

        let result = tool
            .run(FileListInput {
                path: Some("/root".into()),
                recursive: Some(true),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        let json = result_json(result);
        assert_eq!(json["path"], "/root");

        assert_has_entry(&json["entries"], "file1.txt");
        assert_has_entry(&json["entries"], "file2.txt");
        // other.txt must not appear — it's outside /root.
        assert_no_entry(&json["entries"], "other.txt");
    }

    // ---------------------------------------------------------------
    // Non-recursive listing
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn list_non_recursive() {
        let tool = setup_tool(vec![
            ("top/file1.txt", "f1"),
            ("top/file2.txt", "f2"),
            ("top/sub/deep.txt", "deep"),
        ])
        .await;

        let result = tool
            .run(FileListInput {
                path: Some("/top".into()),
                recursive: Some(false),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        let json = result_json(result);

        // Direct children appear.
        assert_has_entry(&json["entries"], "file1.txt");
        assert_has_entry(&json["entries"], "file2.txt");
        // The deeply nested file must NOT appear.
        assert_no_entry(&json["entries"], "deep.txt");
    }

    // ---------------------------------------------------------------
    // Empty / fresh directory
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn list_fresh_temp_dir() {
        let storage = OpendalFileStorage::new_temp();
        let tool = FileListTool {
            storage: Arc::new(storage),
        };

        let result = tool
            .run(FileListInput {
                path: None,
                recursive: None,
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        let json = result_json(result);
        let entries = json["entries"].as_array().unwrap();

        // opendal Fs returns only the root "/" entry for an empty dir.
        assert!(
            entries.len() <= 1,
            "expected at most root entry, got: {entries:?}"
        );
        if !entries.is_empty() {
            assert_eq!(entries[0]["name"], "/");
            assert_eq!(entries[0]["is_dir"], true);
        }
    }

    // ---------------------------------------------------------------
    // File size accuracy
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn list_reports_file_size() {
        let tool = setup_tool(vec![("sized.txt", "hello world")]).await;

        let result = tool
            .run(FileListInput {
                path: None,
                recursive: Some(true),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        let json = result_json(result);

        let file_entry = json["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"].as_str().is_some_and(|n| n.contains("sized.txt")))
            .expect("expected sized.txt entry");
        assert_eq!(file_entry["size"].as_u64().unwrap(), 11); // "hello world"
    }

    // ---------------------------------------------------------------
    // is_dir field
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn list_reports_is_dir() {
        let tool = setup_tool(vec![("with_dir/file.txt", "f"), ("other.txt", "o")]).await;

        let result = tool
            .run(FileListInput {
                path: None,
                recursive: Some(true),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        let json = result_json(result);
        let entries = json["entries"].as_array().unwrap();

        // File entry — is_dir = false
        let file = entries
            .iter()
            .find(|e| e["name"].as_str().is_some_and(|n| n.contains("file.txt")))
            .expect("expected file.txt entry");
        assert!(!file["is_dir"].as_bool().unwrap());

        // At least one directory entry (e.g. "with_dir/") — is_dir = true, size = 0.
        let dir = entries
            .iter()
            .find(|e| e["is_dir"].as_bool() == Some(true))
            .expect("expected at least one directory entry");
        assert_eq!(dir["size"].as_u64().unwrap(), 0);
    }

    // ---------------------------------------------------------------
    // Path normalisation
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn list_path_normalisation() {
        let tool = setup_tool(vec![("foo.txt", "foo")]).await;

        // Relative path → normalised with leading "/".
        let result = tool
            .run(FileListInput {
                path: Some("foo.txt".into()),
                recursive: Some(true),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        assert_eq!(result_json(result)["path"], "/foo.txt");

        // "./" relative path → leading "./" stripped, then normalised.
        let result = tool
            .run(FileListInput {
                path: Some("./foo.txt".into()),
                recursive: Some(true),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        assert_eq!(result_json(result)["path"], "/foo.txt");

        // Bare "/" stays as "/".
        let result = tool
            .run(FileListInput {
                path: Some("/".into()),
                recursive: Some(true),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        assert_eq!(result_json(result)["path"], "/");
    }

    // ---------------------------------------------------------------
    // Non-existent path — opendal returns empty (no error)
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn list_nonexistent_path_returns_empty() {
        let storage = OpendalFileStorage::new_temp();
        let tool = FileListTool {
            storage: Arc::new(storage),
        };

        let result = tool
            .run(FileListInput {
                path: Some("/does_not_exist".into()),
                recursive: Some(false),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        let json = result_json(result);
        let entries = json["entries"].as_array().unwrap();
        assert!(
            entries.is_empty(),
            "expected empty entries for non-existent path, got: {entries:?}"
        );
    }

    // ---------------------------------------------------------------
    // Pagination — offset / limit / window
    // ---------------------------------------------------------------

    /// Create a tool whose recursive listing contains exactly `count` files,
    /// named `file000.txt` … `file{count-1}.txt`. Used to validate slicing
    /// arithmetic without coupling to opendal's traversal quirks.
    async fn setup_tool_with_n_files(count: usize) -> FileListTool {
        let mut layout = Vec::with_capacity(count);
        for i in 0..count {
            layout.push((format!("file{i:03}.txt"), format!("{i}")));
        }
        let refs: Vec<(&str, &str)> = layout
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        setup_tool(refs).await
    }

    #[tokio::test]
    async fn list_default_limit_is_200() {
        // 250 files > DEFAULT_LIST_LIMIT(200). A bare call (no limit) must
        // return at most 200 entries, with `total` reflecting all entries
        // (opendal Fs also emits the "/" root self-entry, so total = 251).
        let tool = setup_tool_with_n_files(250).await;

        let result = tool
            .run(FileListInput {
                path: None,
                recursive: Some(true),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        let json = result_json(result);

        assert_eq!(json["total"], 251);
        assert_eq!(json["limit"], 200);
        assert_eq!(json["returned"], 200);
        assert_eq!(json["offset"], 0);
        assert_eq!(json["has_more"], true);
        assert_eq!(json["next_offset"], 200);
        assert_eq!(json["entries"].as_array().unwrap().len(), 200);
    }

    #[tokio::test]
    async fn list_offset_and_limit_returns_slice() {
        // 250 files, ask for the second page (limit=100). After sorting
        // (directories first, then files alphabetically), the layout is:
        //   index 0       → "/"
        //   index 1..251  → file000.txt .. file249.txt
        // So `offset=201, limit=100` returns exactly file200..file249
        // (50 entries), with the trailing "/" still on disk but out of
        // window.
        let tool = setup_tool_with_n_files(250).await;

        let result = tool
            .run(FileListInput {
                path: None,
                recursive: Some(true),
                offset: Some(201),
                limit: Some(100),
            })
            .await
            .unwrap();
        let json = result_json(result);

        assert_eq!(json["total"], 251);
        assert_eq!(json["offset"], 201);
        assert_eq!(json["limit"], 100);
        assert_eq!(json["returned"], 50); // last 50 file entries
        assert_eq!(json["has_more"], false);
        assert!(json["next_offset"].is_null());
        assert_eq!(json["entries"].as_array().unwrap().len(), 50);

        // The first and last names in the slice must be file200.txt and
        // file249.txt respectively.
        let entries = json["entries"].as_array().unwrap();
        assert_eq!(entries[0]["name"], "file200.txt");
        assert_eq!(entries[entries.len() - 1]["name"], "file249.txt");
    }

    #[tokio::test]
    async fn list_offset_past_total_returns_empty_with_metadata() {
        // Walking past the end must NOT error — return empty entries but
        // accurate `total` so callers can detect over-shoot.
        let tool = setup_tool_with_n_files(10).await;

        let result = tool
            .run(FileListInput {
                path: None,
                recursive: Some(true),
                offset: Some(500),
                limit: Some(10),
            })
            .await
            .unwrap();
        let json = result_json(result);

        assert_eq!(json["total"], 11); // 10 files + "/" self-entry
        assert_eq!(json["offset"], 500);
        assert_eq!(json["returned"], 0);
        assert_eq!(json["has_more"], false);
        assert!(json["next_offset"].is_null());
        assert!(json["entries"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_limit_capped_at_max() {
        // LLM asks for 9999 — tool must clamp to MAX_LIST_LIMIT (1000) rather
        // than honouring the request (which would dump the whole storage).
        let tool = setup_tool_with_n_files(50).await;

        let result = tool
            .run(FileListInput {
                path: None,
                recursive: Some(true),
                offset: None,
                limit: Some(9999),
            })
            .await
            .unwrap();
        let json = result_json(result);

        // Only 51 entries exist in total, so the clamp is moot here — but
        // the echoed `limit` must reflect the cap, not the requested 9999.
        assert_eq!(json["total"], 51);
        assert_eq!(json["limit"], 1000);
        assert_eq!(json["returned"], 51);
        assert_eq!(json["has_more"], false);
    }

    #[tokio::test]
    async fn list_limit_zero_clamped_to_one() {
        // limit=0 is meaningless for paging; clamp to 1 so the response
        // shape is uniform (entries is always an array of ≥0 items).
        let tool = setup_tool_with_n_files(5).await;

        let result = tool
            .run(FileListInput {
                path: None,
                recursive: Some(true),
                offset: None,
                limit: Some(0),
            })
            .await
            .unwrap();
        let json = result_json(result);

        assert_eq!(json["limit"], 1);
        assert_eq!(json["returned"], 1);
        assert_eq!(json["has_more"], true);
    }

    #[tokio::test]
    async fn list_iterate_to_end_via_offset() {
        // Smoke-test the recommended iteration pattern: while has_more,
        // raise offset by returned. After the loop every entry — files
        // and the "/" self-entry — must have been visited exactly once,
        // in deterministic alphabetical order.
        let tool = setup_tool_with_n_files(7).await;
        let mut offset: usize = 0;
        let limit: usize = 3;
        let mut seen_names: Vec<String> = Vec::new();

        loop {
            let result = tool
                .run(FileListInput {
                    path: None,
                    recursive: Some(true),
                    offset: Some(offset),
                    limit: Some(limit),
                })
                .await
                .unwrap();
            let json = result_json(result);

            for entry in json["entries"].as_array().unwrap() {
                seen_names.push(entry["name"].as_str().unwrap().to_string());
            }

            if !json["has_more"].as_bool().unwrap() {
                break;
            }
            offset = json["next_offset"].as_u64().unwrap() as usize;
        }

        // 7 files + the "/" self-entry, all sorted by name (directories
        // come first, then files alphabetically).
        let file_only: Vec<String> =
            seen_names.iter().filter(|n| n.as_str() != "/").cloned().collect();
        let expected_files: Vec<String> =
            (0..7).map(|i| format!("file{i:03}.txt")).collect();
        assert_eq!(file_only, expected_files);
        // "/" was visited exactly once across the whole iteration.
        assert_eq!(seen_names.iter().filter(|n| n.as_str() == "/").count(), 1);
    }
}
