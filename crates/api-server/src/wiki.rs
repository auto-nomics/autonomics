//! Wiki REST module: entries / recycle bin / versions / search / links /
//! mentions / image attachments, all funneled through the worker handle.
//!
//! Mounted at `/api/v1/wiki`. The state is the worker handle
//! (`Arc<WikiHandle>`); the wiki feature has no runtime gate.

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete as rdelete, get, post, put},
};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use wiki::service::WikiHandle;
use wiki::storage::types::{ListFilter, SearchFilter};

const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

#[derive(Deserialize, Default)]
struct ListQuery {
    title: Option<String>,
    tags: Option<String>,
    since: Option<i64>,
    until: Option<i64>,
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Deserialize)]
struct CreateEntryBody {
    title: String,
    content: String,
    #[serde(default)]
    alias: Option<Vec<String>>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
struct UpdateEntryBody {
    new_title: Option<String>,
    alias: Option<Vec<String>>,
    tags: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<usize>,
    tags: Option<String>,
}

#[derive(Deserialize)]
struct SearchBody {
    query: String,
    #[serde(default = "default_search_limit")]
    limit: usize,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    since: Option<i64>,
    #[serde(default)]
    until: Option<i64>,
}

fn default_search_limit() -> usize {
    10
}

#[derive(serde::Serialize)]
struct ApiError {
    error: String,
}

type ApiErr = (StatusCode, Json<ApiError>);
type ApiResult = Result<Json<Value>, ApiErr>;

fn error(status: StatusCode, message: impl Into<String>) -> ApiErr {
    (
        status,
        Json(ApiError {
            error: message.into(),
        }),
    )
}

fn internal(message: impl std::fmt::Display) -> ApiErr {
    error(StatusCode::INTERNAL_SERVER_ERROR, format!("{message}"))
}

/// wiki 错误 → HTTP:NotFound→404,撞名→409,空标题→400,worker 退场→503。
fn api_err(e: wiki::error::Error) -> ApiErr {
    match e {
        wiki::error::Error::Storage(wiki::storage::error::Error::NotFound(id)) => {
            error(StatusCode::NOT_FOUND, format!("entry not found: {id}"))
        }
        wiki::error::Error::Storage(wiki::storage::error::Error::DuplicateTitle {
            title,
            existing_id,
        }) => error(
            StatusCode::CONFLICT,
            format!("title \"{title}\" already used by entry {existing_id}"),
        ),
        wiki::error::Error::Storage(wiki::storage::error::Error::EmptyTitle) => {
            error(StatusCode::BAD_REQUEST, "title is empty after trim")
        }
        wiki::error::Error::WorkerGone(msg) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("wiki worker unavailable: {msg}"),
        ),
        other => internal(other),
    }
}

fn parse_uuid(raw: &str, what: &str) -> Result<uuid::Uuid, ApiErr> {
    uuid::Uuid::parse_str(raw).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: format!("invalid {what} id: {raw}"),
            }),
        )
    })
}

fn parse_tags(raw: &Option<String>) -> Vec<String> {
    raw.as_deref()
        .unwrap_or("")
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn meta_json(m: &wiki::storage::types::AttachmentMeta) -> Value {
    json!({
        "attachment_id": m.id,
        "entry_id": m.entry_id,
        "filename": m.filename,
        "mime": m.mime,
        "size": m.size,
        "sha256": m.sha256,
        "created_at": m.created_at,
    })
}

pub fn router(handle: Arc<WikiHandle>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/entries", get(list_entries).post(create_entry))
        .route(
            "/entries/{id}",
            get(get_entry).put(update_entry).delete(delete_entry),
        )
        .route("/entries/{id}/content", put(put_entry_content))
        .route("/entries/{id}/restore", post(restore_entry))
        .route("/entries/{id}/purge", rdelete(purge_entry))
        .route("/entries/{id}/versions", get(list_versions))
        .route("/entries/{id}/versions/{version}", get(get_version))
        .route(
            "/entries/{id}/versions/{version}/restore",
            post(restore_version),
        )
        .route("/entries/{id}/links", get(links))
        .route("/entries/{id}/mentions", get(mentions))
        .route(
            "/entries/{id}/attachments",
            get(list_attachments).post(upload_attachment),
        )
        .route(
            "/entries/{id}/attachments/{attachment_id}/raw",
            get(download_attachment),
        )
        .route(
            "/entries/{id}/attachments/{attachment_id}",
            rdelete(detach_attachment),
        )
        .route("/search", get(search_get).post(search_post))
        .route("/recycle", get(recycle))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .with_state(handle)
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

// ─────────────────────── entries ───────────────────────

async fn list_entries(
    State(handle): State<Arc<WikiHandle>>,
    Query(query): Query<ListQuery>,
) -> ApiResult {
    // title 精确查找优先(标题可能含斜杠,不适合做路径段)。
    if let Some(title) = query.title.as_deref() {
        let entry = handle
            .find_by_title(title)
            .await
            .map_err(api_err)?
            .ok_or_else(|| {
                error(
                    StatusCode::NOT_FOUND,
                    format!("no entry titled \"{title}\""),
                )
            })?;
        return Ok(Json(json!({ "entries": [entry] })));
    }
    let entries = handle
        .list_entries(&ListFilter {
            tags: parse_tags(&query.tags),
            mtime_since: query.since,
            mtime_until: query.until,
            limit: query.limit.unwrap_or(50).clamp(1, 200),
            offset: query.offset.unwrap_or(0),
        })
        .await
        .map_err(api_err)?;
    let count = entries.len();
    Ok(Json(json!({ "count": count, "entries": entries })))
}

async fn create_entry(
    State(handle): State<Arc<WikiHandle>>,
    Json(body): Json<CreateEntryBody>,
) -> ApiResult {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let entry = wiki::storage::types::KnowledgeEntryData {
        id: uuid::Uuid::new_v4().to_string(),
        title: body.title,
        alias: body.alias.unwrap_or_default(),
        tags: body.tags.unwrap_or_default(),
        content: body.content,
        ctime: now,
        mtime: now,
    };
    handle.create_entry(entry.clone()).await.map_err(api_err)?;
    Ok(Json(json!({ "id": entry.id, "title": entry.title })))
}

async fn get_entry(State(handle): State<Arc<WikiHandle>>, Path(id): Path<String>) -> ApiResult {
    let id = parse_uuid(&id, "entry")?;
    let entry = handle.get_entry(id).await.map_err(api_err)?;
    Ok(Json(json!(entry)))
}

async fn update_entry(
    State(handle): State<Arc<WikiHandle>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateEntryBody>,
) -> ApiResult {
    let id = parse_uuid(&id, "entry")?;
    let mut entry = handle.get_entry(id).await.map_err(api_err)?;
    // 纯元数据合并(与工具面同语义):正文改动走 wiki_patch 端点链路
    // (edit/patch 属 agent 工具面;REST v1 以整篇 PUT 为准时不在此处)。
    if let Some(new_title) = body.new_title {
        entry.title = new_title;
    }
    if let Some(alias) = body.alias {
        entry.alias = alias;
    }
    if let Some(tags) = body.tags {
        entry.tags = tags;
    }
    entry.mtime = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(entry.mtime);
    handle.update_entry(entry.clone()).await.map_err(api_err)?;
    Ok(Json(json!(entry)))
}

#[derive(Deserialize)]
struct PutContentBody {
    content: String,
}

/// 整篇替换正文(存储层同事务重建倒排/链接/版本快照)。元数据更新走
/// `PUT /entries/{id}`。
async fn put_entry_content(
    State(handle): State<Arc<WikiHandle>>,
    Path(id): Path<String>,
    Json(body): Json<PutContentBody>,
) -> ApiResult {
    let id = parse_uuid(&id, "entry")?;
    let mut entry = handle.get_entry(id).await.map_err(api_err)?;
    entry.content = body.content;
    entry.mtime = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(entry.mtime);
    handle.update_entry(entry.clone()).await.map_err(api_err)?;
    Ok(Json(json!({
        "id": entry.id,
        "title": entry.title,
        "size": entry.content.len(),
    })))
}

async fn delete_entry(State(handle): State<Arc<WikiHandle>>, Path(id): Path<String>) -> ApiResult {
    let id = parse_uuid(&id, "entry")?;
    handle.delete_entry(id).await.map_err(api_err)?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

async fn restore_entry(State(handle): State<Arc<WikiHandle>>, Path(id): Path<String>) -> ApiResult {
    let id = parse_uuid(&id, "entry")?;
    handle.restore_entry(id).await.map_err(api_err)?;
    Ok(Json(json!({ "restored": true, "id": id })))
}

async fn purge_entry(State(handle): State<Arc<WikiHandle>>, Path(id): Path<String>) -> ApiResult {
    let id = parse_uuid(&id, "entry")?;
    handle.purge_entry(id).await.map_err(api_err)?;
    Ok(Json(json!({ "purged": true, "id": id })))
}

async fn recycle(State(handle): State<Arc<WikiHandle>>) -> ApiResult {
    let items = handle.list_deleted().await.map_err(api_err)?;
    let count = items.len();
    Ok(Json(json!({ "count": count, "items": items })))
}

// ─────────────────────── versions ───────────────────────

async fn list_versions(State(handle): State<Arc<WikiHandle>>, Path(id): Path<String>) -> ApiResult {
    let id = parse_uuid(&id, "entry")?;
    let versions = handle.list_versions(id).await.map_err(api_err)?;
    let count = versions.len();
    Ok(Json(json!({ "count": count, "versions": versions })))
}

async fn get_version(
    State(handle): State<Arc<WikiHandle>>,
    Path((id, version)): Path<(String, i64)>,
) -> ApiResult {
    let id = parse_uuid(&id, "entry")?;
    let entry = handle.get_version(id, version).await.map_err(api_err)?;
    Ok(Json(json!({ "version": version, "entry": entry })))
}

async fn restore_version(
    State(handle): State<Arc<WikiHandle>>,
    Path((id, version)): Path<(String, i64)>,
) -> ApiResult {
    let id = parse_uuid(&id, "entry")?;
    handle.restore_version(id, version).await.map_err(api_err)?;
    Ok(Json(json!({ "rolled_back_to": version, "id": id })))
}

// ─────────────────────── search / graph ───────────────────────

async fn search_get(
    State(handle): State<Arc<WikiHandle>>,
    Query(query): Query<SearchQuery>,
) -> ApiResult {
    let q = query.q.unwrap_or_default();
    let hits = match parse_tags(&query.tags) {
        tags if tags.is_empty() => handle.search(&q, query.limit.unwrap_or(10)).await,
        tags => {
            handle
                .search_filtered(
                    &q,
                    query.limit.unwrap_or(10),
                    &SearchFilter {
                        tags,
                        ..Default::default()
                    },
                )
                .await
        }
    };
    Ok(Json(json!({ "results": hits.map_err(api_err)? })))
}

async fn search_post(
    State(handle): State<Arc<WikiHandle>>,
    Json(body): Json<SearchBody>,
) -> ApiResult {
    let hits = handle
        .search_filtered(
            &body.query,
            body.limit.clamp(1, 50),
            &SearchFilter {
                tags: body.tags,
                mtime_since: body.since,
                mtime_until: body.until,
            },
        )
        .await
        .map_err(api_err)?;
    Ok(Json(json!({ "results": hits })))
}

async fn links(State(handle): State<Arc<WikiHandle>>, Path(id): Path<String>) -> ApiResult {
    let id = parse_uuid(&id, "entry")?;
    let entry = handle.get_entry(id).await.map_err(api_err)?;
    let out = handle
        .get_out_links_batch(std::slice::from_ref(&entry.id))
        .await
        .map_err(api_err)?
        .into_iter()
        .next()
        .unwrap_or_default();
    let inn = handle
        .get_in_links_batch(std::slice::from_ref(&entry.id))
        .await
        .map_err(api_err)?
        .into_iter()
        .next()
        .unwrap_or_default();
    // UI 按名称显示链接,不显示裸 id。
    Ok(Json(json!({
        "entry": { "id": entry.id, "title": entry.title },
        "out_links": link_refs(&handle, out).await,
        "in_links": link_refs(&handle, inn).await,
    })))
}

/// 批量把链接 id 列表解析成 {id, title}(与 wiki_links 工具同一语义)。
async fn link_refs(handle: &WikiHandle, ids: Vec<String>) -> Vec<serde_json::Value> {
    handle
        .id_title_pairs(ids)
        .await
        .into_iter()
        .map(|(id, title)| json!({ "id": id, "title": title }))
        .collect()
}

async fn mentions(State(handle): State<Arc<WikiHandle>>, Path(id): Path<String>) -> ApiResult {
    let id = parse_uuid(&id, "entry")?;
    let entry = handle.get_entry(id).await.map_err(api_err)?;
    let ids = handle.get_mentions(&entry.id).await.map_err(api_err)?;
    let count = ids.len();
    let mentions = link_refs(&handle, ids).await;
    Ok(Json(json!({ "count": count, "unlinked_mentions": mentions })))
}

// ─────────────────────── attachments ───────────────────────

async fn list_attachments(
    State(handle): State<Arc<WikiHandle>>,
    Path(id): Path<String>,
) -> ApiResult {
    let id = parse_uuid(&id, "entry")?;
    let items = handle.list_attachments(id).await.map_err(api_err)?;
    let count = items.len();
    Ok(Json(json!({
        "count": count,
        "attachments": items.iter().map(meta_json).collect::<Vec<_>>(),
    })))
}

async fn upload_attachment(
    State(handle): State<Arc<WikiHandle>>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> ApiResult {
    let entry_id = parse_uuid(&id, "entry")?;
    let mut filename: Option<String> = None;
    let mut data: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| error(StatusCode::BAD_REQUEST, e.to_string()))?
    {
        if field.name() == Some("file") {
            filename = field
                .file_name()
                .map(str::to_owned)
                .or_else(|| field.content_type().map(str::to_owned));
            let bytes = field
                .bytes()
                .await
                .map_err(|e| error(StatusCode::BAD_REQUEST, e.to_string()))?;
            if bytes.len() > MAX_UPLOAD_BYTES {
                return Err(error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "attachment exceeds 50 MiB",
                ));
            }
            data = Some(bytes.to_vec());
        }
    }
    let filename =
        filename.ok_or_else(|| error(StatusCode::BAD_REQUEST, "missing 'file' field"))?;
    let data = data.ok_or_else(|| error(StatusCode::BAD_REQUEST, "missing 'file' field"))?;
    let meta = handle
        .attach_image(entry_id, filename, data)
        .await
        .map_err(api_err)?;
    Ok(Json(meta_json(&meta)))
}

async fn download_attachment(
    State(handle): State<Arc<WikiHandle>>,
    Path((id, attachment_id)): Path<(String, String)>,
) -> Result<Response, Response> {
    let entry_id = match parse_uuid(&id, "entry") {
        Ok(id) => id,
        Err(response) => return Err(response.into_response()),
    };
    let attachment_id = match parse_uuid(&attachment_id, "attachment") {
        Ok(id) => id,
        Err(response) => return Err(response.into_response()),
    };
    let (meta, data) = handle
        .get_attachment(entry_id, attachment_id)
        .await
        .map_err(|e| api_err(e).into_response())?;
    let mime: axum::http::HeaderValue = meta
        .mime
        .parse()
        .map_err(|_| internal("invalid stored mime type").into_response())?;
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (
                header::CONTENT_DISPOSITION,
                axum::http::HeaderValue::from_str(&format!(
                    "inline; filename=\"{}\"",
                    meta.filename.replace('"', "")
                ))
                .map_err(|_| internal("invalid filename").into_response())?,
            ),
        ],
        data,
    )
        .into_response())
}

async fn detach_attachment(
    State(handle): State<Arc<WikiHandle>>,
    Path((id, attachment_id)): Path<(String, String)>,
) -> ApiResult {
    let entry_id = parse_uuid(&id, "entry")?;
    let attachment_id = parse_uuid(&attachment_id, "attachment")?;
    handle
        .detach_attachment(entry_id, attachment_id)
        .await
        .map_err(api_err)?;
    Ok(Json(
        json!({ "detached": true, "attachment_id": attachment_id }),
    ))
}
