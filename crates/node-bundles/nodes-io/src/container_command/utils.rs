use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

use dag_core::node::NodeInput;
use dag_core::value::NodeValue;

use container_runtime::DEFAULT_CONTAINER_WORKDIR;

use super::ContainerCommandOutputSpec;
use super::error::{FAILURE_CAPTURE_PREVIEW_CHARS, FAILURE_OUTPUT_PREVIEW_BYTES};

pub(crate) fn capture_preview(label: &str, capture: &str) -> String {
    let total_chars = capture.chars().count();
    if total_chars <= FAILURE_CAPTURE_PREVIEW_CHARS {
        return format!("{label}: {capture}");
    }

    let start = capture
        .char_indices()
        .nth_back(FAILURE_CAPTURE_PREVIEW_CHARS - 1)
        .map(|(index, _)| index)
        .unwrap_or(0);
    let tail = capture[start..].trim_end();
    let omitted_chars = total_chars - tail.chars().count();
    format!("{label} tail ({omitted_chars} chars omitted): {tail}")
}

pub(crate) fn capture_declared_output_logs(
    resolved_outputs: &[(&ContainerCommandOutputSpec, PathBuf)],
) -> Vec<(String, String)> {
    let mut logs = Vec::new();
    for (spec, path) in resolved_outputs {
        if !matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("log" | "txt" | "out" | "stderr" | "stdout")
        ) {
            continue;
        }
        let Some(capture) = read_text_tail(path, FAILURE_OUTPUT_PREVIEW_BYTES) else {
            continue;
        };
        if !capture.trim().is_empty() {
            logs.push((spec.path.clone(), capture));
        }
    }
    logs
}

pub(crate) fn capture_input_manifest(staged_inputs: &[NodeInput]) -> Vec<(String, String)> {
    staged_inputs
        .iter()
        .filter_map(|input| {
            let path = match &input.data {
                NodeValue::File(f) => PathBuf::from(&f.path),
                NodeValue::FileSet(files) if let Some(first) = files.first() => {
                    PathBuf::from(&first.path)
                }
                _ => return None,
            };
            let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            Some((path.to_string_lossy().into_owned(), bytes.to_string()))
        })
        .collect()
}

pub(crate) fn write_failure_logs(workspace_path: &Path, stdout: &str, stderr: &str) {
    let log_dir = workspace_path.join(".autonomics").join("failure-logs");
    if std::fs::create_dir_all(&log_dir).is_err() {
        return;
    }
    if !stdout.is_empty() {
        let _ = std::fs::write(log_dir.join("stdout.log"), stdout);
    }
    if !stderr.is_empty() {
        let _ = std::fs::write(log_dir.join("stderr.log"), stderr);
    }
}

fn read_text_tail(path: &Path, max_bytes: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::with_capacity((len - start).try_into().ok()?);
    file.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

// ──────────────────────────────────────────────────────────────────────
// Workspace-relative path validation (used by ContainerCommandSpec outputs
// and the inline `files` map). Pure, has no dependency on the spec type.

// Safe path relative to `/work`. Absolute paths and `..` are rejected so a
// declaration cannot escape the workspace mount.
pub(crate) fn validate_workspace_relative_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.contains('\0') {
        return Err("file paths in `files` cannot be empty".into());
    }
    let candidate = Path::new(path);
    if candidate.is_absolute()
        || !candidate
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(format!("file path `{path}` must be a safe relative path"));
    }
    Ok(())
}

// Like [`validate_workspace_relative_path`], but glob metacharacters
// (`*`, `?`, `[`, `]`) are accepted in any component so an output can be
// declared as a pattern. `..` and absolute paths stay rejected.
pub(crate) fn validate_workspace_relative_glob(path: &str) -> Result<(), String> {
    if path.is_empty() || path.contains('\0') {
        return Err("output path cannot be empty".into());
    }
    let candidate = Path::new(path);
    if candidate.is_absolute()
        || !candidate.components().all(|component| {
            matches!(component, Component::Normal(_))
                || component
                    .as_os_str()
                    .to_str()
                    .is_some_and(|segment| !segment.is_empty() && segment != "..")
        })
    {
        return Err(format!("output path `{path}` must be a safe relative path"));
    }
    if !path.split('/').all(|segment| segment != "..") {
        return Err(format!("output path `{path}` must not traverse upward"));
    }
    Ok(())
}

/// Whether a symlink source's parent directory may be bind-mounted into the
/// container. Tmpfs and kernel/runtime virtual filesystems must never be
/// mount sources (podman mounts tmpfs at `/tmp` itself), and mounting the
/// filesystem root would expose the whole host.
pub(crate) fn mount_safe_parent(parent: &Path) -> bool {
    const UNSAFE: [&str; 5] = ["/tmp", "/dev", "/proc", "/sys", "/run"];
    parent != Path::new("/") && !UNSAFE.iter().any(|root| parent.starts_with(root))
}

/// Whether a declared output path carries glob metacharacters.
pub(crate) fn contains_glob(path: &str) -> bool {
    path.contains('*') || path.contains('?') || path.contains('[')
}

/// The literal directory prefix of a declared output path: every component
/// before the first one carrying glob metacharacters. Used to pre-create
/// output directories for patterns the same way literal paths do.
pub(crate) fn literal_prefix(path: &str) -> Option<String> {
    let mut literal = Vec::new();
    for segment in path.split('/') {
        if contains_glob(segment) {
            break;
        }
        literal.push(segment);
    }
    (!literal.is_empty()).then(|| literal.join("/"))
}

/// Resolve one declared output inside the work dir after the container ran.
///
/// Literal paths keep the exactly-one-file contract. Glob patterns collect
/// sorted matches (skipping the `.autonomics` control directory) and must
/// match exactly one file — Nextflow single-`path` strictness.
pub(crate) fn resolve_output(
    pattern: &str,
    workdir: &Path,
) -> Result<PathBuf, super::error::ContainerCommandError> {
    if !contains_glob(pattern) {
        let path = workdir.join(pattern);
        if path.is_file() {
            return Ok(path);
        }
        return Err(super::error::ContainerCommandError::MissingOutput {
            path: pattern.to_string(),
        });
    }

    let full = workdir.join(pattern);
    let pattern_str = full.to_string_lossy().into_owned();
    let matches = glob::glob(&pattern_str)
        .map_err(|error| {
            super::error::ContainerCommandError::Invalid(format!(
                "invalid output pattern `{pattern}`: {error}"
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| {
            super::error::ContainerCommandError::Invalid(format!(
                "cannot enumerate output pattern `{pattern}`: {error}"
            ))
        })?;
    let mut files: Vec<PathBuf> = matches
        .into_iter()
        .filter(|path| {
            path.is_file()
                && !path
                    .components()
                    .any(|component| component.as_os_str() == ".autonomics")
        })
        .collect();
    files.sort();
    match files.len() {
        0 => Err(super::error::ContainerCommandError::MissingOutput {
            path: pattern.to_string(),
        }),
        1 => Ok(files.remove(0)),
        _ => {
            let matches = files
                .iter()
                .map(|path| {
                    path.strip_prefix(workdir)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            Err(super::error::ContainerCommandError::AmbiguousOutput {
                pattern: pattern.to_string(),
                matches,
            })
        }
    }
}

// Materialize `relative` under `base` after validation. Used for inline
// scripts and the `files` map under `/work/.autonomics`.
pub(crate) fn write_strictly_within(
    base: &Path,
    relative: &str,
    content: &str,
) -> Result<PathBuf, String> {
    validate_workspace_relative_path(relative)?;
    let destination = base.join(relative);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create directory `{}`: {e}", parent.display()))?;
    }
    std::fs::write(&destination, content)
        .map_err(|e| format!("cannot write file `{}`: {e}", destination.display()))?;
    Ok(destination)
}

// ──────────────────────────────────────────────────────────────────────
// Input staging. Extract the host path, compute the container-side vpath,
// and project a `NodeValue` into a filesystem path for the staging dir.

pub(crate) fn input_path(value: &NodeValue) -> Result<String, String> {
    match value {
        NodeValue::File(file) => Ok(file.path.clone()),
        NodeValue::FileSet(files) if !files.is_empty() => Ok(files
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>()
            .join(",")),
        NodeValue::FileSet(_) => Err("an empty FileSet cannot be bound to a command input".into()),
        NodeValue::DataFrame(_) | NodeValue::Channel(_) => {
            Err("container_command inputs must be File or FileSet values".into())
        }
    }
}

pub(crate) fn virtual_path(path: &str) -> Option<String> {
    if let Some(rest) = path.strip_prefix("vfs://") {
        return Some(vfs::OpendalFileStorage::normalize_path(rest));
    }
    if let Some(rest) = path.strip_prefix("file://") {
        return Some(vfs::OpendalFileStorage::normalize_path(rest));
    }
    path.starts_with('/')
        .then(|| vfs::OpendalFileStorage::normalize_path(path))
}

// Compute a stable staged filename that preserves compound `.nii.gz`
// extensions — `Path::extension` only sees the trailing segment, so we
// reattach the inner stem explicitly.
pub(crate) fn staged_path(base: &Path, prefix: &str, index: usize, source: &str) -> PathBuf {
    let source_name = Path::new(source)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let extension = if source_name.to_ascii_lowercase().ends_with(".gz") {
        source_name
            .strip_suffix(".gz")
            .map(Path::new)
            .and_then(|stem| stem.extension())
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{extension}.gz"))
            .unwrap_or_else(|| ".gz".into())
    } else {
        Path::new(source)
            .extension()
            .and_then(|extension| extension.to_str())
            .filter(|extension| !extension.is_empty())
            .map(|extension| format!(".{extension}"))
            .unwrap_or_default()
    };
    base.join(format!("{prefix}-{index}{extension}"))
}

// ──────────────────────────────────────────────────────────────────────
// Host <-> container path translation + workspace scratch naming.

pub(crate) fn container_path(workspace_path: &Path, host_path: &str) -> String {
    host_path
        .split(',')
        .map(|single_path| {
            let path = Path::new(single_path);
            let relative = path.strip_prefix(workspace_path).unwrap_or(path);
            Path::new(DEFAULT_CONTAINER_WORKDIR)
                .join(relative)
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>()
        .join(",")
}

// Lower-case hex dump. Used by `publish_output` to record the artifact
// digest.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
