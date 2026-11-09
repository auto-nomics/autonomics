//! Source resolution: enumerate fanout items through the VFS, uniformly.
//!
//! [`SourceSpec::Glob`] patterns address the engine's virtual filesystem — plain
//! mount-relative paths (`data/*.fastq`) or explicit `vfs://` URLs — and are
//! enumerated through [`vfs::OpendalFileStorage`], so mount policies,
//! permission scoping and (later) SSH data homes all apply identically. There
//! is deliberately no raw host-filesystem fallback: a flow never bypasses the
//! VFS to reach data.
//!
//! Globs support wildcard directory segments, recursive `**`, and character
//! classes and brace alternatives. Enumeration uses the VFS object-store view to honor mounts and
//! per-entry access checks.

use datafusion::object_store::{ObjectStore, path::Path};
use futures::{FutureExt, TryStreamExt};
use std::{collections::BTreeMap, sync::Arc};
use tokio::{sync::mpsc, task::JoinSet};
use vfs::OpendalFileStorage;

use crate::workflow::error::Error;
use crate::workflow::model::{FlowGraph, PathType, SourceSpec};
use crate::workflow::plan::SourceResolution;

/// Bounded source readers. Dropping this value cancels outstanding enumerations.
pub struct SourceStreams {
    pub(crate) readers: BTreeMap<String, mpsc::Receiver<Result<serde_json::Value, Error>>>,
    _tasks: JoinSet<()>,
}

async fn guard_source(
    name: String,
    tx: mpsc::Sender<Result<serde_json::Value, Error>>,
    work: impl std::future::Future<Output = ()>,
) {
    if std::panic::AssertUnwindSafe(work)
        .catch_unwind()
        .await
        .is_err()
    {
        let _ = tx
            .send(Err(Error::Execution(format!("source `{name}` panicked"))))
            .await;
    }
}

impl SourceStreams {
    #[cfg(test)]
    pub(crate) fn from_test_reader(
        name: &str,
        reader: mpsc::Receiver<Result<serde_json::Value, Error>>,
    ) -> Self {
        Self {
            readers: BTreeMap::from([(name.to_owned(), reader)]),
            _tasks: JoinSet::new(),
        }
    }

    pub(crate) fn from_resolution(
        graph: &FlowGraph,
        resolution: &SourceResolution,
    ) -> Result<Self, Error> {
        let mut readers = BTreeMap::new();
        let mut tasks = JoinSet::new();
        for source in graph.sources() {
            let values = resolution
                .get(&source.id)
                .ok_or_else(|| Error::Execution(format!("source `{}` is unresolved", source.id)))?
                .clone();
            let (tx, rx) = mpsc::channel(16);
            readers.insert(source.id.clone(), rx);
            tasks.spawn(async move {
                for value in values {
                    if tx.send(Ok(value)).await.is_err() {
                        break;
                    }
                }
            });
        }
        Ok(Self {
            readers,
            _tasks: tasks,
        })
    }
}

/// Start independent, bounded source producers for a single workflow run.
pub fn stream_sources(
    graph: &FlowGraph,
    storage: Option<Arc<OpendalFileStorage>>,
) -> Result<SourceStreams, Error> {
    stream_sources_with_functions(graph, storage, None)
}

pub fn stream_sources_with_functions(
    graph: &FlowGraph,
    storage: Option<Arc<OpendalFileStorage>>,
    channel_function: Option<crate::workflow::ChannelFunction>,
) -> Result<SourceStreams, Error> {
    let mut readers = BTreeMap::new();
    let mut tasks = JoinSet::new();
    for source in graph.sources() {
        let spec = source.spec.clone();
        if matches!(spec, SourceSpec::Glob { .. } | SourceSpec::FilePairs { .. })
            && storage.is_none()
        {
            return Err(Error::Execution(format!(
                "source `{}` uses glob but no VFS storage is configured",
                source.id
            )));
        }
        if matches!(
            &spec,
            SourceSpec::FilePairs {
                grouping: Some(_),
                ..
            }
        ) && channel_function.is_none()
        {
            return Err(Error::Execution(format!(
                "source `{}` requires a channel function evaluator",
                source.id
            )));
        }
        let (tx, rx) = mpsc::channel(16);
        readers.insert(source.id.clone(), rx);
        let storage = storage.clone();
        let channel_function = channel_function.clone();
        let name = source.id.clone();
        let error_tx = tx.clone();
        tasks.spawn(async move {
            guard_source(name, error_tx, async move {
                match spec {
                    SourceSpec::Items { values } => {
                        for value in values {
                            if tx.send(Ok(value)).await.is_err() {
                                break;
                            }
                        }
                    }
                    SourceSpec::Value { value } => {
                        let _ = tx.send(Ok(value)).await;
                    }
                    SourceSpec::FilePairs {
                        pattern,
                        path_type,
                        hidden,
                        check_if_exists,
                        glob,
                        max_depth,
                        size,
                        flat,
                        grouping,
                    } => {
                        if let Err(error) = stream_file_pairs(
                            pattern.as_slice(),
                            storage.expect("checked storage"),
                            FilePairOptions {
                                glob: GlobOptions {
                                    path_type,
                                    hidden,
                                    check_if_exists,
                                    glob,
                                    max_depth,
                                },
                                size,
                                flat,
                                grouping,
                                evaluator: channel_function,
                            },
                            &tx,
                        )
                        .await
                        {
                            let _ = tx.send(Err(error)).await;
                        }
                    }
                    SourceSpec::Glob {
                        pattern,
                        path_type,
                        hidden,
                        check_if_exists,
                        glob,
                        max_depth,
                    } => {
                        let options = GlobOptions {
                            path_type,
                            hidden,
                            check_if_exists,
                            glob,
                            max_depth,
                        };
                        for pattern in pattern.as_slice() {
                            if let Err(error) = stream_glob(
                                pattern,
                                storage.as_deref().expect("checked storage"),
                                options,
                                &tx,
                            )
                            .await
                            {
                                let _ = tx.send(Err(error)).await;
                                break;
                            }
                            if tx.is_closed() {
                                break;
                            }
                        }
                    }
                }
            })
            .await;
        });
    }
    Ok(SourceStreams {
        readers,
        _tasks: tasks,
    })
}

/// Match Nextflow's `Channel.readPrefix` against a matched filename.
fn pair_prefix(path: &str, pattern: &str) -> Result<Option<String>, Error> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let pattern = pattern.rsplit('/').next().unwrap_or(pattern);
    if !pattern.contains(['*', '?', '{', '[']) {
        return Ok((name == pattern).then(|| {
            std::path::Path::new(name)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        }));
    }
    let mut regex = String::from("^");
    let mut wildcard_groups = 0usize;
    let mut first_bracket = None;
    let mut chars = pattern.chars().peekable();
    let mut position = 0usize;
    while let Some(ch) = chars.next() {
        match ch {
            '*' | '?' => {
                if first_bracket.is_none() {
                    wildcard_groups += 1;
                }
                regex.push_str(if ch == '*' { "(.*)" } else { "(.?)" });
            }
            '{' => {
                first_bracket.get_or_insert(position);
                regex.push_str("(?:");
            }
            '}' => regex.push(')'),
            ',' if first_bracket.is_some() => regex.push('|'),
            '[' => {
                first_bracket.get_or_insert(position);
                regex.push('[');
                for inner in chars.by_ref() {
                    regex.push(inner);
                    position += inner.len_utf8();
                    if inner == ']' {
                        break;
                    }
                }
            }
            _ => regex.push_str(&regex::escape(&ch.to_string())),
        }
        position += ch.len_utf8();
    }
    regex.push('$');
    let regex = regex::Regex::new(&regex)
        .map_err(|e| Error::Execution(format!("invalid file pair pattern `{pattern}`: {e}")))?;
    let Some(captures) = regex.captures(name) else {
        return Ok(None);
    };
    let end = if wildcard_groups > 0 {
        captures
            .get(wildcard_groups)
            .map(|m| m.end())
            .unwrap_or(name.len())
    } else {
        first_bracket.unwrap_or(name.len()).min(name.len())
    };
    let prefix = name
        .get(..end)
        .ok_or_else(|| Error::Execution("invalid UTF-8 boundary in file pair prefix".into()))?
        .trim_end_matches(['-', '_', '.']);
    Ok(Some(prefix.to_string()))
}

struct FilePairOptions {
    glob: GlobOptions,
    size: Option<i64>,
    flat: bool,
    grouping: Option<serde_json::Value>,
    evaluator: Option<crate::workflow::ChannelFunction>,
}

async fn stream_file_pairs(
    patterns: &[String],
    storage: Arc<OpendalFileStorage>,
    options: FilePairOptions,
    output: &mpsc::Sender<Result<serde_json::Value, Error>>,
) -> Result<(), Error> {
    use dag_core::dag::execution::ChannelTransportValue as T;
    let FilePairOptions {
        glob: options,
        size,
        flat,
        grouping,
        evaluator,
    } = options;
    let size = size.filter(|size| *size != 0).unwrap_or_else(|| {
        if patterns
            .iter()
            .any(|pattern| pattern.contains(['*', '?', '{', '[']))
        {
            2
        } else {
            1
        }
    });
    let mut groups = BTreeMap::<String, (T, Vec<String>)>::new();
    let mut group_order = Vec::new();
    for pattern in patterns {
        let (tx, mut rx) = mpsc::channel(16);
        let pattern_copy = pattern.clone();
        let storage = Arc::clone(&storage);
        let task =
            tokio::spawn(async move { stream_glob(&pattern_copy, &storage, options, &tx).await });
        while let Some(value) = rx.recv().await {
            let value = value?;
            let path = value
                .as_str()
                .ok_or_else(|| Error::Execution("file source returned non-path".into()))?
                .to_string();
            let key = if let Some(function) = &grouping {
                let evaluator = evaluator.clone().ok_or_else(|| {
                    Error::Execution(
                        "file pair grouping requires a channel function evaluator".into(),
                    )
                })?;
                let function = function.clone();
                let input =
                    dag_core::value::NodeValue::File(dag_core::value::FileRef::new(&path, None));
                let result =
                    tokio::task::spawn_blocking(move || evaluator(&function, Some(&input)))
                        .await
                        .map_err(|e| Error::Execution(format!("file pair grouping failed: {e}")))?
                        .map_err(Error::Execution)?;
                crate::workflow::reactive::node_transport_value(result)?
            } else {
                let Some(prefix) = pair_prefix(&path, pattern)? else {
                    continue;
                };
                T::Json(serde_json::Value::String(prefix))
            };
            let identity = serde_json::to_string(&crate::workflow::reactive::group_key(&key))
                .map_err(|e| Error::Execution(e.to_string()))?;
            if size < 0 && !groups.contains_key(&identity) {
                group_order.push(identity.clone());
            }
            let group = groups
                .entry(identity.clone())
                .or_insert_with(|| (key, Vec::new()));
            group.1.push(path);
            if size > 0 && i64::try_from(group.1.len()).ok() == Some(size) {
                let (key, paths) = groups.remove(&identity).expect("group exists");
                let item = file_pair_value(key, paths, flat)?;
                if output.send(Ok(item)).await.is_err() {
                    task.abort();
                    return Ok(());
                }
            }
        }
        task.await
            .map_err(|e| Error::Execution(format!("file pair enumeration failed: {e}")))??;
    }
    if size < 0 {
        for identity in group_order {
            let (key, paths) = groups.remove(&identity).expect("group exists");
            if output
                .send(Ok(file_pair_value(key, paths, flat)?))
                .await
                .is_err()
            {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn file_pair_value(
    key: dag_core::dag::execution::ChannelTransportValue,
    mut paths: Vec<String>,
    flat: bool,
) -> Result<serde_json::Value, Error> {
    paths.sort();
    let key = serde_json::to_value(key).map_err(|e| Error::Execution(e.to_string()))?;
    if flat {
        let mut values = vec![key];
        values.extend(paths.into_iter().map(serde_json::Value::String));
        Ok(serde_json::Value::Array(values))
    } else {
        Ok(serde_json::json!([key, paths]))
    }
}

#[derive(Clone, Copy)]
struct GlobOptions {
    path_type: PathType,
    hidden: bool,
    check_if_exists: bool,
    glob: bool,
    max_depth: Option<i32>,
}

impl Default for GlobOptions {
    fn default() -> Self {
        Self {
            path_type: PathType::File,
            hidden: false,
            check_if_exists: false,
            glob: true,
            max_depth: None,
        }
    }
}

async fn stream_glob(
    pattern: &str,
    storage: &OpendalFileStorage,
    opts: GlobOptions,
    tx: &mpsc::Sender<Result<serde_json::Value, Error>>,
) -> Result<(), Error> {
    let normalized =
        OpendalFileStorage::normalize_path(pattern.strip_prefix("vfs://").unwrap_or(pattern));
    let normalized = normalized.trim_start_matches('/');
    if !opts.glob || !normalized.contains(['*', '?', '[', '{']) {
        storage
            .check_readable(normalized)
            .map_err(|e| Error::Execution(format!("cannot access `{normalized}`: {e}")))?;
        if opts.check_if_exists {
            storage.content_length(normalized).await.map_err(|e| {
                Error::Execution(format!(
                    "path `{pattern}` does not exist or cannot be accessed: {e}"
                ))
            })?;
        }
        let _ = tx
            .send(Ok(serde_json::json!(format!("vfs://{normalized}"))))
            .await;
        return Ok(());
    }
    let matchers = expand_braces(normalized)?
        .iter()
        .map(|p| glob::Pattern::new(p))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| Error::Execution(format!("invalid glob pattern `{pattern}`: {error}")))?;
    let meta = normalized
        .find(['*', '?', '[', '{'])
        .unwrap_or(normalized.len());
    let prefix = normalized[..meta]
        .rsplit_once('/')
        .map(|(dir, _)| dir)
        .unwrap_or("");
    storage.check_listable(prefix).map_err(|e| {
        Error::Execution(format!("cannot list `{prefix}` for glob `{pattern}`: {e}"))
    })?;
    let path = if prefix.is_empty() {
        None
    } else {
        Some(Path::parse(prefix).map_err(|e| Error::Execution(e.to_string()))?)
    };
    let options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: !opts.hidden,
    };
    if opts.max_depth.is_some() || opts.path_type != PathType::File {
        let max_depth = opts.max_depth.unwrap_or(i32::MAX);
        let mut directories = Vec::new();
        if max_depth >= 0 {
            directories.push((path.clone(), 0_i32));
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut visited_directories = std::collections::BTreeSet::new();
        while let Some((directory, depth)) = directories.pop() {
            let current = directory.as_ref().map(|p| p.as_ref()).unwrap_or("");
            if !visited_directories.insert(current.to_owned()) {
                continue;
            }
            let result = match storage.list_with_delimiter(directory.as_ref()).await {
                Ok(result) => result,
                Err(datafusion::object_store::Error::NotFound { .. }) => continue,
                Err(e) => {
                    return Err(Error::Execution(format!(
                        "cannot list `{prefix}` for glob `{pattern}`: {e}"
                    )));
                }
            };
            for entry in result.objects {
                let path = entry.location.as_ref();
                if opts.path_type != PathType::Dir
                    && matchers
                        .iter()
                        .any(|matcher| matcher.matches_with(path, options))
                    && seen.insert(path.to_owned())
                    && tx
                        .send(Ok(serde_json::json!(format!("vfs://{path}"))))
                        .await
                        .is_err()
                {
                    return Ok(());
                }
            }
            for child in result.common_prefixes {
                let child_path = child.as_ref().trim_end_matches('/');
                if child_path == current || visited_directories.contains(child_path) {
                    continue;
                }
                if opts.path_type != PathType::File
                    && matchers
                        .iter()
                        .any(|matcher| matcher.matches_with(child_path, options))
                    && seen.insert(child_path.to_owned())
                    && tx
                        .send(Ok(serde_json::json!(format!("vfs://{child_path}"))))
                        .await
                        .is_err()
                {
                    return Ok(());
                }
                if depth < max_depth {
                    directories.push((Some(child), depth + 1));
                }
            }
        }
        if opts.check_if_exists && seen.is_empty() {
            return Err(Error::Execution(format!(
                "no files match pattern `{pattern}`"
            )));
        }
        return Ok(());
    }
    let mut entries = storage.list(path.as_ref());
    let mut seen = std::collections::BTreeSet::new();
    loop {
        match entries.try_next().await {
            Ok(Some(entry)) => {
                let path = entry.location.as_ref();
                if matchers
                    .iter()
                    .any(|matcher| matcher.matches_with(path, options))
                    && seen.insert(path.to_owned())
                    && tx
                        .send(Ok(serde_json::json!(format!("vfs://{path}"))))
                        .await
                        .is_err()
                {
                    return Ok(());
                }
            }
            Ok(None) | Err(datafusion::object_store::Error::NotFound { .. }) => {
                if opts.check_if_exists && seen.is_empty() {
                    return Err(Error::Execution(format!(
                        "no files match pattern `{pattern}`"
                    )));
                }
                return Ok(());
            }
            Err(e) => {
                return Err(Error::Execution(format!(
                    "cannot list `{prefix}` for glob `{pattern}`: {e}"
                )));
            }
        }
    }
}

/// Resolve every fanout source of the graph into a [`SourceResolution`],
/// ready for [`crate::workflow::plan::build_plan`]. `Items` sources pass through
/// untouched; `Glob` sources are enumerated through `storage`.
///
/// Fails when a glob source exists but no storage is configured, or when
/// enumeration itself fails (unlistable directory or unsupported pattern).
/// Literal paths are emitted without an existence check, matching fromPath defaults.
pub async fn resolve_sources(
    graph: &FlowGraph,
    storage: Option<&OpendalFileStorage>,
) -> Result<SourceResolution, Error> {
    let mut resolution = SourceResolution::new();
    for source in graph.sources() {
        let items = match &source.spec {
            SourceSpec::Items { values } => values.clone(),
            SourceSpec::Value { value } => vec![value.clone()],
            SourceSpec::FilePairs {
                pattern,
                path_type,
                hidden,
                check_if_exists,
                glob,
                max_depth,
                size,
                flat,
                grouping,
            } => {
                if grouping.is_some() {
                    return Err(Error::Execution("file pair grouping requires a channel function evaluator; use stream_sources_with_functions".into()));
                }
                let storage = Arc::new(
                    storage
                        .ok_or_else(|| {
                            Error::Execution(format!("source `{}` requires VFS storage", source.id))
                        })?
                        .clone(),
                );
                let patterns = pattern.as_slice().to_vec();
                let (tx, mut rx) = mpsc::channel(16);
                let options = GlobOptions {
                    path_type: *path_type,
                    hidden: *hidden,
                    check_if_exists: *check_if_exists,
                    glob: *glob,
                    max_depth: *max_depth,
                };
                let (size, flat) = (*size, *flat);
                let task = tokio::spawn(async move {
                    stream_file_pairs(
                        &patterns,
                        storage,
                        FilePairOptions {
                            glob: options,
                            size,
                            flat,
                            grouping: None,
                            evaluator: None,
                        },
                        &tx,
                    )
                    .await
                });
                let mut values = Vec::new();
                while let Some(value) = rx.recv().await {
                    values.push(value?);
                }
                task.await.map_err(|e| {
                    Error::Execution(format!("file pair enumeration failed: {e}"))
                })??;
                values
            }
            SourceSpec::Glob {
                pattern,
                path_type,
                hidden,
                check_if_exists,
                glob,
                max_depth,
            } => {
                let storage = storage.ok_or_else(|| {
                    Error::Execution(format!(
                        "source `{}` uses glob `{pattern:?}` but no VFS storage is configured",
                        source.id
                    ))
                })?;
                let options = GlobOptions {
                    path_type: *path_type,
                    hidden: *hidden,
                    check_if_exists: *check_if_exists,
                    glob: *glob,
                    max_depth: *max_depth,
                };
                let mut items = Vec::new();
                for pattern in pattern.as_slice() {
                    items.extend(enumerate_glob_with_options(pattern, storage, options).await?);
                }
                items
            }
        };
        resolution.resolve(source.id.clone(), items);
    }
    Ok(resolution)
}

/// Enumerate one glob pattern into item values. Each item is the matched
/// file's virtual path as a JSON string (`vfs://…`), which `{{item}}`
/// templates render directly and [`crate::workflow::instance::item_key`] turns into a
/// stable per-file key. Results are sorted by path so plans and reports are
/// deterministic across runs.
pub async fn enumerate_glob(
    pattern: &str,
    storage: &OpendalFileStorage,
) -> Result<Vec<serde_json::Value>, Error> {
    enumerate_glob_with_options(pattern, storage, GlobOptions::default()).await
}

async fn enumerate_glob_with_options(
    pattern: &str,
    storage: &OpendalFileStorage,
    options: GlobOptions,
) -> Result<Vec<serde_json::Value>, Error> {
    let (tx, mut rx) = mpsc::channel(16);
    let producer = async move {
        let result = stream_glob(pattern, storage, options, &tx).await;
        drop(tx);
        result
    };
    let collect = async move {
        let mut values = Vec::new();
        while let Some(value) = rx.recv().await {
            values.push(value?);
        }
        Ok::<_, Error>(values)
    };
    let (result, values) = futures::join!(producer, collect);
    result?;
    let mut values = values?;
    values.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    Ok(values)
}

// Java's glob syntax, used by Nextflow, supports comma-separated alternatives.
fn expand_braces(pattern: &str) -> Result<Vec<String>, Error> {
    let Some(start) = pattern.find('{') else {
        if pattern.contains('}') {
            return Err(Error::Execution(format!(
                "invalid glob pattern `{pattern}`: unmatched brace"
            )));
        }
        return Ok(vec![pattern.to_owned()]);
    };
    let mut depth = 0usize;
    let mut end = None;
    let mut separators = Vec::new();
    for (offset, ch) in pattern[start..].char_indices() {
        match ch {
            '{' => {
                depth += 1;
                if depth > 1 {
                    return Err(Error::Execution(format!(
                        "invalid glob pattern `{pattern}`: nested braces"
                    )));
                }
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + offset);
                    break;
                }
            }
            ',' if depth == 1 => separators.push(start + offset),
            _ => {}
        }
    }
    let end = end.ok_or_else(|| {
        Error::Execution(format!("invalid glob pattern `{pattern}`: unmatched brace"))
    })?;
    let mut result = Vec::new();
    let mut part = start + 1;
    for boundary in separators.into_iter().chain(std::iter::once(end)) {
        let expanded = format!(
            "{}{}{}",
            &pattern[..start],
            &pattern[part..boundary],
            &pattern[end + 1..]
        );
        result.extend(expand_braces(&expanded)?);
        part = boundary + 1;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_pair_prefix_matches_nextflow_examples() {
        use super::pair_prefix;
        assert_eq!(
            pair_prefix("vfs://data/sample_R1.fastq", "data/*_R{1,2}.fastq")
                .unwrap()
                .as_deref(),
            Some("sample")
        );
        assert_eq!(
            pair_prefix("vfs://data/sample_R2.fastq", "data/*_R{1,2}.fastq")
                .unwrap()
                .as_deref(),
            Some("sample")
        );
        assert_eq!(
            pair_prefix("vfs://data/file_alpha_1.fa", "data/file*_1.fa")
                .unwrap()
                .as_deref(),
            Some("file_alpha")
        );
        assert_eq!(
            pair_prefix("vfs://data/other.fastq", "data/*_R{1,2}.fastq").unwrap(),
            None
        );
    }
    use super::*;
    use crate::workflow::model::{FlowGraphBuilder, Process, Source, Wire};

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
    async fn streaming_glob_matches_existing_vfs_enumeration() {
        let storage = Arc::new(storage_with_files().await);
        let graph = FlowGraphBuilder::default()
            .source(Source::glob("reads", "data/{a,b}.fastq"))
            .build()
            .unwrap();
        let mut streams = stream_sources(&graph, Some(Arc::clone(&storage))).unwrap();
        let reader = streams.readers.get_mut("reads").unwrap();
        let mut actual = Vec::new();
        while let Some(value) = reader.recv().await {
            actual.push(value.unwrap());
        }
        actual.sort_by_key(|v| v.as_str().unwrap().to_owned());
        assert_eq!(
            actual,
            enumerate_glob("data/{a,b}.fastq", &storage).await.unwrap()
        );
    }

    #[tokio::test]
    async fn multiple_path_patterns_emit_in_order_and_keep_overlap() {
        let storage = Arc::new(storage_with_files().await);
        let spec: SourceSpec = serde_json::from_value(
            serde_json::json!({"type":"glob", "pattern":["data/a.fastq", "data/*.fastq"]}),
        )
        .unwrap();
        let graph = FlowGraphBuilder::default()
            .source(Source {
                id: "reads".into(),
                spec,
            })
            .build()
            .unwrap();
        let mut streams = stream_sources(&graph, Some(Arc::clone(&storage))).unwrap();
        let reader = streams.readers.get_mut("reads").unwrap();
        let mut values = Vec::new();
        while let Some(value) = reader.recv().await {
            values.push(value.unwrap());
        }
        assert_eq!(values.len(), 4);
        assert_eq!(values[0], serde_json::json!("vfs://data/a.fastq"));
        assert_eq!(
            values
                .iter()
                .filter(|value| **value == serde_json::json!("vfs://data/a.fastq"))
                .count(),
            2
        );
        let batch = resolve_sources(&graph, Some(&storage)).await.unwrap();
        let items = batch.get("reads").unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0], serde_json::json!("vfs://data/a.fastq"));
    }

    #[tokio::test]
    async fn multiple_path_patterns_report_failure_after_prior_matches() {
        let storage = Arc::new(storage_with_files().await);
        let spec: SourceSpec = serde_json::from_value(serde_json::json!({"type":"glob", "pattern":["data/a.fastq", "data/missing/*.fastq"], "checkIfExists":true})).unwrap();
        let graph = FlowGraphBuilder::default()
            .source(Source {
                id: "reads".into(),
                spec,
            })
            .build()
            .unwrap();
        let mut streams = stream_sources(&graph, Some(storage)).unwrap();
        let reader = streams.readers.get_mut("reads").unwrap();
        assert_eq!(
            reader.recv().await.unwrap().unwrap(),
            serde_json::json!("vfs://data/a.fastq")
        );
        assert!(
            reader
                .recv()
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("no files match")
        );
    }

    #[tokio::test]
    async fn streaming_glob_reports_invalid_patterns() {
        let storage = Arc::new(storage_with_files().await);
        let graph = FlowGraphBuilder::default()
            .source(Source::glob("reads", "data/["))
            .build()
            .unwrap();
        let mut streams = stream_sources(&graph, Some(storage)).unwrap();
        let error = streams
            .readers
            .get_mut("reads")
            .unwrap()
            .recv()
            .await
            .unwrap()
            .unwrap_err();
        assert!(
            error.to_string().contains("invalid glob pattern"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn source_panic_is_reported_after_partial_delivery() {
        let (tx, mut rx) = mpsc::channel(2);
        let error_tx = tx.clone();
        let task = tokio::spawn(guard_source("reads".into(), error_tx, async move {
            tx.send(Ok(serde_json::json!("first"))).await.unwrap();
            panic!("source backend panicked");
        }));
        assert_eq!(
            rx.recv().await.unwrap().unwrap(),
            serde_json::json!("first")
        );
        let error = rx.recv().await.unwrap().unwrap_err();
        assert!(
            error.to_string().contains("source `reads` panicked"),
            "{error}"
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn from_path_options_match_hidden_and_check_existence() {
        let storage = storage_with_files().await;
        storage
            .write_bytes("data/.secret.fastq", b"x".to_vec())
            .await
            .unwrap();
        let hidden = enumerate_glob_with_options(
            "data/*.fastq",
            &storage,
            GlobOptions {
                hidden: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(hidden.contains(&serde_json::json!("vfs://data/.secret.fastq")));
        let default = enumerate_glob("data/*.fastq", &storage).await.unwrap();
        assert!(!default.contains(&serde_json::json!("vfs://data/.secret.fastq")));
        for pattern in [
            "data/missing.fastq",
            "data/missing/*.fastq",
            "data/*.missing",
        ] {
            let error = enumerate_glob_with_options(
                pattern,
                &storage,
                GlobOptions {
                    check_if_exists: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains(pattern), "{error}");
        }
        let literal = enumerate_glob_with_options(
            "data/[*",
            &storage,
            GlobOptions {
                glob: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(literal, vec![serde_json::json!("vfs://data/[*")]);
        assert!(
            enumerate_glob_with_options(
                "data/[*",
                &storage,
                GlobOptions {
                    glob: false,
                    check_if_exists: true,
                    ..Default::default()
                }
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn glob_type_selects_files_directories_or_both() {
        let storage = OpendalFileStorage::new_temp();
        for path in ["data/root.txt", "data/sample/a.txt", "data/other/b.txt"] {
            storage.write_bytes(path, b"x".to_vec()).await.unwrap();
        }
        for (path_type, expected) in [
            (PathType::File, vec!["vfs://data/root.txt"]),
            (PathType::Dir, vec!["vfs://data/other", "vfs://data/sample"]),
            (
                PathType::Any,
                vec![
                    "vfs://data/other",
                    "vfs://data/root.txt",
                    "vfs://data/sample",
                ],
            ),
        ] {
            let actual = enumerate_glob_with_options(
                "data/*",
                &storage,
                GlobOptions {
                    path_type,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(
                actual,
                expected
                    .into_iter()
                    .map(serde_json::Value::from)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[tokio::test]
    async fn max_depth_limits_directory_traversal_like_nextflow() {
        let storage = OpendalFileStorage::new_temp();
        for path in ["data/root.fastq", "data/a/one.fastq", "data/a/b/two.fastq"] {
            storage.write_bytes(path, b"x".to_vec()).await.unwrap();
        }
        for (depth, expected) in [
            (-1, vec![]),
            (0, vec!["vfs://data/root.fastq"]),
            (1, vec!["vfs://data/a/one.fastq", "vfs://data/root.fastq"]),
            (
                2,
                vec![
                    "vfs://data/a/b/two.fastq",
                    "vfs://data/a/one.fastq",
                    "vfs://data/root.fastq",
                ],
            ),
        ] {
            let result = enumerate_glob_with_options(
                "data/**/*.fastq",
                &storage,
                GlobOptions {
                    max_depth: Some(depth),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(
                result,
                expected
                    .into_iter()
                    .map(|value| serde_json::json!(value))
                    .collect::<Vec<_>>(),
                "depth={depth}"
            );
        }
    }

    #[tokio::test]
    async fn star_glob_lists_matching_files_sorted() {
        let storage = storage_with_files().await;
        let items = enumerate_glob("data/*.fastq", &storage).await.unwrap();
        let paths: Vec<&str> = items.iter().filter_map(|value| value.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "vfs://data/a.fastq",
                "vfs://data/b.fastq",
                "vfs://data/sample_1.fastq"
            ]
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
    async fn exact_path_is_emitted_even_when_it_does_not_exist() {
        let storage = storage_with_files().await;
        assert_eq!(
            enumerate_glob("data/c.txt", &storage).await.unwrap(),
            vec![serde_json::json!("vfs://data/c.txt")]
        );
        assert_eq!(
            enumerate_glob("data/missing.txt", &storage).await.unwrap(),
            vec![serde_json::json!("vfs://data/missing.txt")]
        );
    }

    #[tokio::test]
    async fn missing_directory_glob_produces_no_items() {
        let storage = storage_with_files().await;
        assert!(
            enumerate_glob("missing/*.txt", &storage)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn brace_alternatives_match_existing_paths_only() {
        let storage = storage_with_files().await;
        let items = enumerate_glob("data/{a,b,missing}.fastq", &storage)
            .await
            .unwrap();
        assert_eq!(
            items,
            vec![
                serde_json::json!("vfs://data/a.fastq"),
                serde_json::json!("vfs://data/b.fastq")
            ]
        );
        let items = enumerate_glob("data/{a,b}.{fastq,txt}", &storage)
            .await
            .unwrap();
        assert_eq!(
            items,
            vec![
                serde_json::json!("vfs://data/a.fastq"),
                serde_json::json!("vfs://data/b.fastq")
            ]
        );
    }

    #[tokio::test]
    async fn empty_match_is_an_empty_fanout_not_an_error() {
        let storage = storage_with_files().await;
        let items = enumerate_glob("data/*.csv", &storage).await.unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn wildcard_directory_and_recursive_globs_find_matching_files() {
        let storage = storage_with_files().await;
        for name in [
            "data/a/one.fastq",
            "data/b/two.fastq",
            "data/b/nested/three.fastq",
            "data/.hidden/secret.fastq",
        ] {
            storage.write_bytes(name, b"x".to_vec()).await.unwrap();
        }
        let one_level = enumerate_glob("data/*/*.fastq", &storage).await.unwrap();
        assert_eq!(
            one_level,
            vec![
                serde_json::json!("vfs://data/a/one.fastq"),
                serde_json::json!("vfs://data/b/two.fastq")
            ]
        );
        let recursive = enumerate_glob("data/**/*.fastq", &storage).await.unwrap();
        assert!(recursive.contains(&serde_json::json!("vfs://data/b/nested/three.fastq")));
        assert!(recursive.contains(&serde_json::json!("vfs://data/a.fastq")));
        assert!(!recursive.contains(&serde_json::json!("vfs://data/.hidden/secret.fastq")));
    }

    #[tokio::test]
    async fn resolve_sources_fills_glob_and_items_uniformly() {
        let storage = storage_with_files().await;
        let graph = FlowGraphBuilder::default()
            .source(Source::glob("glob_reads", "data/*.fastq"))
            .source(Source::items(
                "manual_reads",
                vec![serde_json::json!("x.fastq")],
            ))
            .process(Process::new(
                "qc",
                "fastqc",
                serde_json::json!({ "input": "{{item}}" }),
            ))
            .process(Process::new(
                "manual",
                "fastqc",
                serde_json::json!({ "input": "{{item}}" }),
            ))
            .wire(Wire::direct("glob_reads", 0, "qc", 0))
            .wire(Wire::direct("manual_reads", 0, "manual", 0))
            .build()
            .unwrap();
        let resolution = resolve_sources(&graph, Some(&storage)).await.unwrap();
        // Glob enumerated through the VFS; items passed through as declared.
        let plan = crate::workflow::plan::build_plan(&graph, &resolution).unwrap();
        assert_eq!(plan.processes["qc"].instances.len(), 3);
        assert_eq!(plan.processes["manual"].instances.len(), 1);
    }

    #[tokio::test]
    async fn glob_without_storage_is_rejected() {
        let graph = FlowGraphBuilder::default()
            .source(Source::glob("reads", "data/*.fastq"))
            .process(Process::new("qc", "fastqc", serde_json::json!({})))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .build()
            .unwrap();
        let error = resolve_sources(&graph, None).await.unwrap_err();
        assert!(error.to_string().contains("no VFS storage"), "{error}");
    }
}
