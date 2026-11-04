//! Source resolution: enumerate fanout items through the VFS, uniformly.
//!
//! [`Source::Glob`] patterns address the engine's virtual filesystem — plain
//! mount-relative paths (`data/*.fastq`) or explicit `vfs://` URLs — and are
//! enumerated through [`vfs::OpendalFileStorage`], so mount policies,
//! permission scoping and (later) SSH data homes all apply identically. There
//! is deliberately no raw host-filesystem fallback: a flow never bypasses the
//! VFS to reach data.
//!
//! v1 pattern grammar: `*` and `?` in the **final path segment only**
//! (`data/*.fastq`, `data/sample_?.fastq`). A pattern without wildcards names
//! one exact file and must exist. Recursive (`**`) and mid-path wildcards are
//! rejected with an explicit error rather than silently mis-matching.

use vfs::OpendalFileStorage;

use crate::error::Error;
use crate::model::{Cardinality, FlowGraph, Source};
use crate::plan::SourceResolution;

/// Resolve every fanout source of the graph into a [`SourceResolution`],
/// ready for [`crate::plan::build_plan`]. `Items` sources pass through
/// untouched; `Glob` sources are enumerated through `storage`.
///
/// Fails when a glob source exists but no storage is configured, or when
/// enumeration itself fails (unlistable directory, missing exact file,
/// unsupported pattern).
pub async fn resolve_sources(
    graph: &FlowGraph,
    storage: Option<&OpendalFileStorage>,
) -> Result<SourceResolution, Error> {
    let mut resolution = SourceResolution::new();
    for process in graph.processes() {
        let Cardinality::Fanout { source, .. } = &process.cardinality else {
            continue;
        };
        let items = match source {
            Source::Items { values } => values.clone(),
            Source::Glob { pattern } => {
                let storage = storage.ok_or_else(|| {
                    Error::Execution(format!(
                        "fanout process `{}` uses glob `{pattern}` but no VFS storage is configured",
                        process.id
                    ))
                })?;
                enumerate_glob(pattern, storage).await?
            }
        };
        resolution.resolve(process.id.clone(), items);
    }
    Ok(resolution)
}

/// Enumerate one glob pattern into item values. Each item is the matched
/// file's virtual path as a JSON string (`vfs://…`), which `{{item}}`
/// templates render directly and [`crate::instance::item_key`] turns into a
/// stable per-file key. Results are sorted by path so plans and reports are
/// deterministic across runs.
pub async fn enumerate_glob(
    pattern: &str,
    storage: &OpendalFileStorage,
) -> Result<Vec<serde_json::Value>, Error> {
    let invalid = |reason: String| {
        Error::Execution(format!("glob pattern `{pattern}` is invalid: {reason}"))
    };
    let stripped = pattern.strip_prefix("vfs://").unwrap_or(pattern);
    let normalized = OpendalFileStorage::normalize_path(stripped);
    let (directory, leaf) = normalized
        .rsplit_once('/')
        .ok_or_else(|| invalid("pattern must name a directory".into()))?;
    // Root-level patterns keep an empty directory; normalize to "/".
    let directory = if directory.is_empty() { "/" } else { directory };

    if directory.contains('*') || directory.contains('?') || leaf.contains('/') {
        return Err(invalid(
            "wildcards are only supported in the final path segment (`**` is not available yet)"
                .into(),
        ));
    }
    let wildcard = leaf.contains('*') || leaf.contains('?');

    // Listing goes through the permission-scoped operator: denied paths are
    // filtered by the VFS layer, not by this function.
    storage
        .check_listable(directory)
        .map_err(|error| Error::Execution(format!("cannot list `{directory}` for glob `{pattern}`: {error}")))?;
    let operator = storage.resolve(directory);
    // opendal's listing is prefix-based: a directory needs its trailing
    // separator, with the root as the lone special case.
    let list_path = if directory == "/" { "/".to_string() } else { format!("{directory}/") };
    let entries = operator
        .list(&list_path)
        .await
        .map_err(|error| Error::Execution(format!("cannot list `{directory}` for glob `{pattern}`: {error}")))?;

    let mut paths = Vec::new();
    for entry in &entries {
        let entry_path = entry.path();
        // Directory members come back with a trailing separator on some
        // backends; a flow fans out over files only.
        if entry_path.ends_with('/') {
            continue;
        }
        let virtual_path = storage.remap_entry_to_virtual(directory, entry_path);
        let name = virtual_path.rsplit('/').next().unwrap_or_default();
        let matched = if wildcard {
            glob_match(leaf, name)
        } else {
            name == leaf
        };
        if matched {
            paths.push(format!("vfs://{virtual_path}"));
        }
    }

    if !wildcard && paths.is_empty() {
        // A wildcard matching nothing is a legitimate empty fanout; a fully
        // specified file that does not exist is an authoring error.
        return Err(Error::Execution(format!(
            "glob pattern `{pattern}` names a file that does not exist"
        )));
    }

    paths.sort();
    Ok(paths.into_iter().map(serde_json::Value::String).collect())
}

/// Minimal matcher for the v1 grammar: `*` matches any (possibly empty)
/// sequence, `?` exactly one character, everything else literal.
fn glob_match(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    // Classic two-row DP over (pattern prefix, name prefix).
    let mut previous = vec![true; name.len() + 1];
    for token in pattern.iter() {
        let mut current = vec![false; name.len() + 1];
        if *token == '*' {
            // A leading star can match the whole prefix: current[0] inherits
            // previous[0]; beyond that, `*` absorbs one more character.
            current[0] = previous[0];
        }
        for position in 0..=name.len() {
            if !current[position] && previous[position] {
                match token {
                    '*' => current[position] = true,
                    '?' if position < name.len() => current[position + 1] = true,
                    literal if position < name.len() && name[position] == *literal => {
                        current[position + 1] = true
                    }
                    _ => {}
                }
            }
            if *token == '*' && current[position] && position < name.len() {
                current[position + 1] = true;
            }
        }
        previous = current;
    }
    previous[name.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FlowGraphBuilder, Process};

    async fn storage_with_files() -> OpendalFileStorage {
        let storage = OpendalFileStorage::new_temp();
        for name in ["a.fastq", "b.fastq", "c.txt", "sample_1.fastq"] {
            storage
                .write_bytes(&format!("data/{name}"), b"payload".to_vec())
                .await
                .unwrap();
        }
        storage
    }

    #[tokio::test]
    async fn star_glob_lists_matching_files_sorted() {
        let storage = storage_with_files().await;
        let items = enumerate_glob("data/*.fastq", &storage).await.unwrap();
        let paths: Vec<&str> = items
            .iter()
            .filter_map(|value| value.as_str())
            .collect();
        assert_eq!(
            paths,
            vec!["vfs://data/a.fastq", "vfs://data/b.fastq", "vfs://data/sample_1.fastq"]
        );
    }

    #[tokio::test]
    async fn vfs_scheme_and_question_mark_work() {
        let storage = storage_with_files().await;
        let items = enumerate_glob("vfs://data/sample_?.fastq", &storage)
            .await
            .unwrap();
        assert_eq!(items, vec![serde_json::json!("vfs://data/sample_1.fastq")]);
    }

    #[tokio::test]
    async fn exact_file_pattern_must_exist() {
        let storage = storage_with_files().await;
        assert!(enumerate_glob("data/c.txt", &storage).await.is_ok());
        let error = enumerate_glob("data/missing.txt", &storage)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("does not exist"), "{error}");
    }

    #[tokio::test]
    async fn empty_match_is_an_empty_fanout_not_an_error() {
        let storage = storage_with_files().await;
        let items = enumerate_glob("data/*.csv", &storage).await.unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn wildcard_in_directory_segment_is_rejected() {
        let storage = storage_with_files().await;
        let error = enumerate_glob("data/*/x.fastq", &storage).await.unwrap_err();
        assert!(error.to_string().contains("final path segment"), "{error}");
    }

    #[tokio::test]
    async fn resolve_sources_fills_glob_and_items_uniformly() {
        let storage = storage_with_files().await;
        let graph = FlowGraphBuilder::default()
            .process(Process::fanout(
                "qc",
                "fastqc",
                serde_json::json!({ "input": "{{item}}" }),
                Source::glob("data/*.fastq"),
                "sample",
            ))
            .process(Process::fanout(
                "manual",
                "fastqc",
                serde_json::json!({ "input": "{{item}}" }),
                Source::items(vec![serde_json::json!("x.fastq")]),
                "sample",
            ))
            .build()
            .unwrap();
        let resolution = resolve_sources(&graph, Some(&storage)).await.unwrap();
        // Glob enumerated through the VFS; items passed through as declared.
        let plan = crate::plan::build_plan(
            &graph,
            &resolution,
        )
        .unwrap();
        assert_eq!(plan.processes["qc"].instances.len(), 3);
        assert_eq!(plan.processes["manual"].instances.len(), 1);
    }

    #[tokio::test]
    async fn glob_without_storage_is_rejected() {
        let graph = FlowGraphBuilder::default()
            .process(Process::fanout(
                "qc",
                "fastqc",
                serde_json::json!({}),
                Source::glob("data/*.fastq"),
                "sample",
            ))
            .build()
            .unwrap();
        let error = resolve_sources(&graph, None).await.unwrap_err();
        assert!(error.to_string().contains("no VFS storage"), "{error}");
    }

    #[test]
    fn glob_matcher_covers_star_and_question() {
        assert!(glob_match("*.fastq", "a.fastq"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("*.fastq", "c.txt"));
        assert!(glob_match("sample_?", "sample_1"));
        assert!(!glob_match("sample_?", "sample_12"));
        assert!(glob_match("a*b*c", "aXbYc"));
        assert!(!glob_match("a*b*c", "aXc"));
    }
}
