//! VFS browse and file handlers: the daemon-side backend for the GUI/TUI
//! VFS browser.
//!
//! Everything dispatches through the [`vfs::browse`] facade, so
//! permissions (including read-only mounts) are enforced inside
//! [`vfs::OpendalFileStorage`] exactly as they are for agent tools — the
//! handlers cannot bypass the mount table. Reads use the storage's
//! lock-holding streams so chunked HTTP responses never race a
//! concurrent replacement writer.

use axum::Json;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use vfs::OpendalFileStorage;
use vfs::browse::{self, BrowseError};

use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;

/// Default `max_bytes` for the text endpoint (1 MiB), matching the
/// editor's comfortable first-load size.
const DEFAULT_TEXT_MAX_BYTES: usize = 1024 * 1024;
/// Hard cap for the text endpoint (10 MiB), matching the vbash `cat`
/// bound.
const MAX_TEXT_MAX_BYTES: usize = 10 * 1024 * 1024;
/// Upload body cap for the VFS route group.
pub(crate) const VFS_MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

/// Map a typed browse error onto the gateway error convention
/// (`{"error": message}` with the suggested status code).
fn vfs_error(error: BrowseError) -> GatewayError {
    let status = StatusCode::from_u16(error.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    GatewayError::Status(status, error.message().to_string())
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListQuery {
    path: Option<String>,
    recursive: Option<bool>,
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PathQuery {
    path: String,
    download: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TextQuery {
    path: String,
    max_bytes: Option<usize>,
}

/// `GET /vfs/list` — paginated, mount-aware directory listing.
#[utoipa::path(get, path = "/api/v1/vfs/list", tag = "vfs",
    params(
        ("path" = Option<String>, Query, description = "Virtual directory path; defaults to `/`"),
        ("recursive" = Option<bool>, Query, description = "Walk recursively"),
        ("limit" = Option<usize>, Query, description = "Page size (default 200)"),
        ("offset" = Option<usize>, Query, description = "Entries to skip"),
    ),
    responses(
        (status = 200, description = "Directory listing with synthetic mount-point entries: {path, entries: [{path, name, is_dir, size, last_modified}], returned, truncated, next_offset}"),
        (status = 403, description = "Path is not listable for this principal"),
    ))]
pub(crate) async fn list(
    State(state): State<GatewayState>,
    Query(query): Query<ListQuery>,
) -> GatewayResult<Json<browse::VfsListing>> {
    let listing = browse::list(
        &state.infra.file_storage,
        query.path.as_deref().unwrap_or("/"),
        query.recursive.unwrap_or(false),
        query.limit,
        query.offset,
    )
    .await
    .map_err(vfs_error)?;
    Ok(Json(listing))
}

/// `GET /vfs/stat` — metadata plus the covering mount's read-only flag.
#[utoipa::path(get, path = "/api/v1/vfs/stat", tag = "vfs",
    params(("path" = String, Query, description = "Virtual path")),
    responses((status = 200, description = "Entry metadata"), (status = 404, description = "Not found")))]
pub(crate) async fn stat(
    State(state): State<GatewayState>,
    Query(query): Query<PathQuery>,
) -> GatewayResult<Json<serde_json::Value>> {
    let storage = &state.infra.file_storage;
    let entry = browse::stat(storage, &query.path)
        .await
        .map_err(vfs_error)?;
    let mount = browse::covering_mount(storage, &query.path);
    Ok(Json(serde_json::json!({
        "path": entry.path,
        "uri": entry.uri,
        "name": entry.name,
        "is_dir": entry.is_dir,
        "size": entry.size,
        "last_modified": entry.last_modified,
        "read_only": mount.as_ref().is_some_and(|m| m.read_only),
        "mount": mount.map(|m| serde_json::json!({
            "path": m.path,
            "backend": m.backend,
            "source": m.source,
            "read_only": m.read_only,
            "domain": m.domain,
        })),
    })))
}

/// `GET /vfs/mounts` — the declarative mount table.
#[utoipa::path(get, path = "/api/v1/vfs/mounts", tag = "vfs",
    responses((status = 200, description = "Every mount (path / backend / source / read_only)")))]
pub(crate) async fn mounts(State(state): State<GatewayState>) -> Json<serde_json::Value> {
    let mounts: Vec<serde_json::Value> = browse::mounts(&state.infra.file_storage)
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "path": m.path,
                "backend": m.backend,
                "source": m.source,
                "read_only": m.read_only,
            "domain": m.domain,
            })
        })
        .collect();
    Json(serde_json::json!({
        "mounts": mounts,
        "count": mounts.len(),
    }))
}

/// `GET /vfs/text` — bounded, UTF-8-validated read for the editor.
#[utoipa::path(get, path = "/api/v1/vfs/text", tag = "vfs",
    params(
        ("path" = String, Query, description = "Virtual file path"),
        ("max_bytes" = Option<usize>, Query, description = "Read bound (default 1 MiB, cap 10 MiB)"),
    ),
    responses(
        (status = 200, description = "Text contents: {path, contents, size, truncated}"),
        (status = 400, description = "File is not valid UTF-8 text"),
        (status = 404, description = "Not found"),
    ))]
pub(crate) async fn text(
    State(state): State<GatewayState>,
    Query(query): Query<TextQuery>,
) -> GatewayResult<Json<browse::TextRead>> {
    let max_bytes = query
        .max_bytes
        .unwrap_or(DEFAULT_TEXT_MAX_BYTES)
        .min(MAX_TEXT_MAX_BYTES);
    let text = browse::read_text(&state.infra.file_storage, &query.path, max_bytes)
        .await
        .map_err(vfs_error)?;
    Ok(Json(text))
}

/// `GET /vfs/file` — stream raw bytes; honors a single `bytes=a-b`
/// Range header (206/416) and switches `Content-Disposition` between
/// inline and attachment via `?download=true`.
#[utoipa::path(get, path = "/api/v1/vfs/file", tag = "vfs",
    params(
        ("path" = String, Query, description = "Virtual file path"),
        ("download" = Option<bool>, Query, description = "Set Content-Disposition to attachment"),
    ),
    responses(
        (status = 200, description = "Full file stream"),
        (status = 206, description = "Range slice stream"),
        (status = 404, description = "Not found"),
    ))]
pub(crate) async fn file(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> GatewayResult<Response> {
    let storage: &OpendalFileStorage = &state.infra.file_storage;
    let path = &query.path;
    let content_type = content_type_for(path);
    let disposition = if query.download.unwrap_or(false) {
        attachment_disposition(path)
    } else {
        header::HeaderValue::from_static("inline")
    };

    // ── Range request ──
    if let Some(spec) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        let size = storage
            .content_length(path)
            .await
            .map_err(BrowseError::from)
            .map_err(vfs_error)?;
        let Some(range) = parse_range(spec, size) else {
            return Err(GatewayError::Status(
                StatusCode::RANGE_NOT_SATISFIABLE,
                format!("requested range '{spec}' is not satisfiable for '{path}' ({size} bytes)"),
            ));
        };
        let stream = storage
            .read_stream(path, range.clone())
            .await
            .map_err(BrowseError::from)
            .map_err(vfs_error)?;
        let mut response = Response::new(Body::from_stream(stream));
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, content_type);
        headers.insert(header::CONTENT_DISPOSITION, disposition);
        headers.insert(
            header::CONTENT_LENGTH,
            header::HeaderValue::from_str(&(range.end - range.start).to_string()).unwrap(),
        );
        headers.insert(
            header::CONTENT_RANGE,
            header::HeaderValue::from_str(&format!(
                "bytes {}-{}/{}",
                range.start,
                range.end.saturating_sub(1),
                size
            ))
            .unwrap(),
        );
        headers.insert(
            header::ACCEPT_RANGES,
            header::HeaderValue::from_static("bytes"),
        );
        return Ok(response);
    }

    // ── Full stream (stat + read under the same object lock) ──
    let (size, stream) = storage
        .read_full_stream(path)
        .await
        .map_err(BrowseError::from)
        .map_err(vfs_error)?;
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, content_type);
    headers.insert(header::CONTENT_DISPOSITION, disposition);
    headers.insert(
        header::CONTENT_LENGTH,
        header::HeaderValue::from_str(&size.to_string()).unwrap(),
    );
    headers.insert(
        header::ACCEPT_RANGES,
        header::HeaderValue::from_static("bytes"),
    );
    Ok(response)
}

/// Parse a single-range `bytes=` header against a resource of `size`
/// bytes. Returns `None` for malformed or unsatisfiable ranges
/// (including empty suffix ranges).
fn parse_range(spec: &str, size: u64) -> Option<std::ops::Range<u64>> {
    let spec = spec.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        // Multi-range requests are not supported; serve the full body.
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    if start.is_empty() {
        // Suffix form `bytes=-n`: the final n bytes.
        let suffix: u64 = end.parse().ok()?;
        if suffix == 0 || size == 0 {
            return None;
        }
        let length = suffix.min(size);
        Some(size - length..size)
    } else {
        let start: u64 = start.parse().ok()?;
        let end: u64 = if end.is_empty() {
            size.saturating_sub(1)
        } else {
            end.parse().ok()?
        };
        if size == 0 || start >= size || end < start {
            return None;
        }
        Some(start..end.min(size - 1) + 1)
    }
}

/// Attachment disposition carrying the entry's file name.
fn attachment_disposition(path: &str) -> header::HeaderValue {
    let name = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("file")
        .chars()
        .filter(|c| !matches!(c, '"' | '\\' | '\r' | '\n'))
        .collect::<String>();
    let name = if name.is_empty() {
        "file".to_string()
    } else {
        name
    };
    header::HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
        .unwrap_or_else(|_| header::HeaderValue::from_static("attachment"))
}

/// Content type by file extension for the common formats the browser
/// serves; everything else is an octet stream so browsers download
/// instead of guessing.
fn content_type_for(path: &str) -> header::HeaderValue {
    let ext = path
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let mime = match ext.as_str() {
        "txt" | "md" | "markdown" | "log" | "csv" | "tsv" | "r" | "py" | "rs" | "sh" | "toml"
        | "yaml" | "yml" | "json" | "xml" | "js" | "ts" | "css" | "svg" => match ext.as_str() {
            "svg" => "image/svg+xml",
            "css" => "text/css",
            "json" => "application/json",
            "xml" | "ts" | "js" => "text/javascript",
            _ => "text/plain; charset=utf-8",
        },
        "html" | "htm" => "text/html; charset=utf-8",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "parquet" => "application/vnd.apache.parquet",
        "gz" => "application/gzip",
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        _ => "application/octet-stream",
    };
    header::HeaderValue::from_str(mime).expect("static mime strings are valid header values")
}

// ── Write endpoints ──────────────────────────────────────────────────
//
// Every mutation dispatches through the browse facade, so read-only
// mounts and MountPermissions are enforced inside OpendalFileStorage —
// exactly like the agent-facing vbash ops.

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(crate) struct MkdirRequest {
    path: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RemoveQuery {
    path: String,
    recursive: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(crate) struct TransferRequest {
    src: String,
    dst: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(crate) struct SaveTextRequest {
    path: String,
    contents: String,
    /// Last-known contents; a mismatch answers 409 so the editor
    /// reloads instead of clobbering a concurrent change. `None`
    /// overwrites unconditionally (upload semantics).
    original: Option<String>,
}

/// `POST /vfs/mkdir` — create a directory.
#[utoipa::path(post, path = "/api/v1/vfs/mkdir", tag = "vfs",
    request_body = MkdirRequest,
    responses(
        (status = 200, description = "Directory created"),
        (status = 403, description = "Read-only mount"),
    ))]
pub(crate) async fn mkdir(
    State(state): State<GatewayState>,
    Json(request): Json<MkdirRequest>,
) -> GatewayResult<Json<serde_json::Value>> {
    let path = browse::mkdir(&state.infra.file_storage, &request.path)
        .await
        .map_err(vfs_error)?;
    Ok(Json(serde_json::json!({ "path": path, "created": true })))
}

/// `DELETE /vfs/entry` — delete a file or directory.
#[utoipa::path(delete, path = "/api/v1/vfs/entry", tag = "vfs",
    params(
        ("path" = String, Query, description = "Virtual path"),
        ("recursive" = Option<bool>, Query, description = "Remove non-empty directories"),
    ),
    responses(
        (status = 200, description = "Entry deleted"),
        (status = 400, description = "Refused (virtual root)"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Directory not empty without recursive"),
    ))]
pub(crate) async fn remove(
    State(state): State<GatewayState>,
    Query(query): Query<RemoveQuery>,
) -> GatewayResult<Json<serde_json::Value>> {
    let recursive = query.recursive.unwrap_or(false);
    let path = browse::remove(&state.infra.file_storage, &query.path, recursive)
        .await
        .map_err(vfs_error)?;
    Ok(Json(
        serde_json::json!({ "path": path, "deleted": true, "recursive": recursive }),
    ))
}

/// `POST /vfs/move` — rename/move. Requires write access on both ends so
/// nothing can be smuggled out of a read-only mount.
#[utoipa::path(post, path = "/api/v1/vfs/move", tag = "vfs",
    request_body = TransferRequest,
    responses(
        (status = 200, description = "Entry moved"),
        (status = 403, description = "Read-only mount on either end"),
    ))]
pub(crate) async fn move_path(
    State(state): State<GatewayState>,
    Json(request): Json<TransferRequest>,
) -> GatewayResult<Json<serde_json::Value>> {
    let (src, dst) = browse::move_entry(&state.infra.file_storage, &request.src, &request.dst)
        .await
        .map_err(vfs_error)?;
    Ok(Json(
        serde_json::json!({ "src": src, "dst": dst, "moved": true }),
    ))
}

/// `POST /vfs/copy` — copy a file (or same-backend subtree).
#[utoipa::path(post, path = "/api/v1/vfs/copy", tag = "vfs",
    request_body = TransferRequest,
    responses(
        (status = 200, description = "Entry copied"),
        (status = 403, description = "Destination is a read-only mount"),
    ))]
pub(crate) async fn copy(
    State(state): State<GatewayState>,
    Json(request): Json<TransferRequest>,
) -> GatewayResult<Json<serde_json::Value>> {
    let (src, dst) = browse::copy(&state.infra.file_storage, &request.src, &request.dst)
        .await
        .map_err(vfs_error)?;
    Ok(Json(
        serde_json::json!({ "src": src, "dst": dst, "copied": true }),
    ))
}

/// `PUT /vfs/file?path=` — raw byte upload (binary or large files).
/// `write_bytes` stages then renames under the per-object lock, so
/// readers never observe a partial file.
#[utoipa::path(put, path = "/api/v1/vfs/file", tag = "vfs",
    params(("path" = String, Query, description = "Virtual destination path")),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = 200, description = "Bytes stored"),
        (status = 403, description = "Read-only mount"),
    ))]
pub(crate) async fn upload(
    State(state): State<GatewayState>,
    Query(query): Query<PathQuery>,
    body: axum::body::Bytes,
) -> GatewayResult<Json<serde_json::Value>> {
    let size = body.len() as u64;
    state
        .infra
        .file_storage
        .write_bytes(&query.path, body.to_vec())
        .await
        .map_err(BrowseError::from)
        .map_err(vfs_error)?;
    Ok(Json(
        serde_json::json!({ "path": query.path, "size": size, "written": true }),
    ))
}

/// `PUT /vfs/text` — conflict-checked text save from the editor.
#[utoipa::path(put, path = "/api/v1/vfs/text", tag = "vfs",
    request_body = SaveTextRequest,
    responses(
        (status = 200, description = "Text stored"),
        (status = 409, description = "File changed on the server; reload before saving"),
        (status = 403, description = "Read-only mount"),
    ))]
pub(crate) async fn save_text(
    State(state): State<GatewayState>,
    Json(request): Json<SaveTextRequest>,
) -> GatewayResult<Json<serde_json::Value>> {
    let storage = &state.infra.file_storage;
    if let Some(original) = &request.original {
        match browse::read_text(storage, &request.path, MAX_TEXT_MAX_BYTES).await {
            Ok(current) => {
                if current.contents != *original {
                    return Err(GatewayError::Status(
                        StatusCode::CONFLICT,
                        "File changed on the server; reload it before saving".into(),
                    ));
                }
            }
            // Missing file: the save creates it (editor "new file" flow).
            Err(BrowseError::NotFound(_)) => {}
            Err(error) => return Err(vfs_error(error)),
        }
    }
    let size = request.contents.len() as u64;
    storage
        .write_bytes(&request.path, request.contents.into_bytes())
        .await
        .map_err(BrowseError::from)
        .map_err(vfs_error)?;
    Ok(Json(
        serde_json::json!({ "path": request.path, "size": size, "written": true }),
    ))
}
