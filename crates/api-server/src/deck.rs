//! Deck REST module: decks / slides / assets / exports, plus the
//! print-token-authenticated open surface for the headless-Chromium
//! export page.
//!
//! Mounted at `/api/v1/deck`. The state is [`ppt_base::DeckShared`].
//! `router()` carries everything the GUI needs; gateway wraps it with
//! `bearer_or_session` (when an env token is configured). `open_router()`
//! is nested WITHOUT that layer and validates the one-time print token
//! itself — that is the export page's only credential.

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete as rdelete, get, patch, post, put},
};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use ppt_base::assets::{BlobArea, asset_blob_key};
use ppt_base::operations::{SlidePatch, insert_slide, move_slide, remove_slide};
use ppt_base::store::{ExportFormat, ExportRecord, ExportStatus};

const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;
/// Print tokens live 15 minutes (single deck scope, one-time-ish).
const PRINT_TOKEN_TTL_SECS: i64 = 15 * 60;

type ApiErr = (StatusCode, Json<ApiError>);
type ApiResult = Result<Json<Value>, ApiErr>;

#[derive(serde::Serialize)]
struct ApiError {
    error: String,
}

fn error(status: StatusCode, message: impl Into<String>) -> ApiErr {
    (
        status,
        Json(ApiError {
            error: message.into(),
        }),
    )
}

fn api_err(e: ppt_base::Error) -> ApiErr {
    match e {
        ppt_base::Error::DeckNotFound(id) => {
            error(StatusCode::NOT_FOUND, format!("deck not found: {id}"))
        }
        ppt_base::Error::SlideOutOfRange { deck, no, total } => error(
            StatusCode::NOT_FOUND,
            format!("deck {deck} has {total} slides (1-{total}); got slide {no}"),
        ),
        ppt_base::Error::BadFrontmatter(m) => {
            error(StatusCode::BAD_REQUEST, format!("invalid frontmatter YAML: {m}"))
        }
        ppt_base::Error::BadAspectRatio(e) => {
            error(StatusCode::BAD_REQUEST, e.to_string())
        }
        ppt_base::Error::BadRange(e) => error(StatusCode::BAD_REQUEST, e.to_string()),
        ppt_base::Error::HeadmatterGuard => error(
            StatusCode::CONFLICT,
            "slide 1 frontmatter is the deck headmatter; edit the deck headmatter instead",
        ),
        ppt_base::Error::RevisionConflict { expected, actual } => error(
            StatusCode::CONFLICT,
            format!("revision conflict: deck is at {expected}, request was based on {actual}"),
        ),
        ppt_base::Error::Export(m) => error(StatusCode::INTERNAL_SERVER_ERROR, format!("export: {m}")),
        ppt_base::Error::Asset(m) => error(StatusCode::NOT_FOUND, format!("asset: {m}")),
        ppt_base::Error::Storage(m) => {
            error(StatusCode::INTERNAL_SERVER_ERROR, format!("storage: {m}"))
        }
        ppt_base::Error::Serde(e) => {
            error(StatusCode::BAD_REQUEST, format!("serialization: {e}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

/// The guarded API (session/bearer).
pub fn router(shared: Arc<ppt_base::DeckShared>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/decks", get(list_decks).post(create_deck))
        .route(
            "/decks/{id}",
            get(get_deck).delete(delete_deck),
        )
        .route("/decks/{id}/source", put(put_source))
        .route("/decks/{id}/slides", post(insert_slide_route))
        .route(
            "/decks/{id}/slides/{no}",
            get(get_slide).patch(patch_slide).delete(delete_slide),
        )
        .route("/decks/{id}/slides/{no}/move", post(move_slide_route))
        .route(
            "/decks/{id}/assets",
            get(list_assets).post(upload_asset),
        )
        .route(
            "/decks/{id}/assets/{aid}/raw",
            get(download_asset),
        )
        .route(
            "/decks/{id}/assets/{aid}",
            rdelete(delete_asset),
        )
        .route(
            "/decks/{id}/exports",
            get(list_exports).post(create_export),
        )
        .route("/exports/{eid}", get(get_export))
        .route("/exports/{eid}/download", get(download_export))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .with_state(shared)
}

/// The open print surface (headless export page). Token-validated in the
/// handlers; nested without the bearer/session layer.
pub fn open_router(shared: Arc<ppt_base::DeckShared>) -> Router {
    Router::new()
        .route("/decks/{id}/print-data", get(print_data))
        .route("/decks/{id}/print-assets/{aid}", get(print_asset))
        .with_state(shared)
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

// ---------------------------------------------------------------------------
// Body/query types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateDeckBody {
    title: String,
    #[serde(default)]
    template: Option<String>,
    #[serde(default)]
    markdown: Option<String>,
}

#[derive(Deserialize, Default)]
struct PutSourceBody {
    markdown: String,
    #[serde(default)]
    message: Option<String>,
    /// Expected revision for optimistic concurrency; omit to force.
    #[serde(default)]
    revision: Option<String>,
}

#[derive(Deserialize, Default)]
struct PatchSlideBody {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    frontmatter_raw: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize, Default)]
struct InsertSlideBody {
    after: u32,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    frontmatter_raw: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize)]
struct MoveSlideBody {
    to: u32,
}

#[derive(Deserialize)]
struct CreateExportBody {
    format: String,
    #[serde(default)]
    dpi: Option<u32>,
    #[serde(default)]
    range: Option<String>,
}

#[derive(Deserialize, Default)]
struct PrintQuery {
    #[serde(default)]
    token: Option<String>,
}

// ---------------------------------------------------------------------------
// Deck endpoints
// ---------------------------------------------------------------------------

async fn list_decks(State(shared): State<Arc<ppt_base::DeckShared>>) -> ApiResult {
    let decks = shared.store.list_decks().await.map_err(api_err)?;
    // Enrich with live slide counts (cheap: single-column reads).
    let mut rows = Vec::with_capacity(decks.len());
    for d in decks {
        let slide_count = shared
            .load_deck(&d.id)
            .await
            .map(|deck| deck.len() as i64)
            .unwrap_or(0);
        rows.push(json!({
            "id": d.id,
            "title": d.title,
            "slideCount": slide_count,
            "version": d.version,
            "createdAt": d.created_at,
            "updatedAt": d.updated_at,
        }));
    }
    Ok(Json(json!({ "decks": rows })))
}

async fn create_deck(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Json(body): Json<CreateDeckBody>,
) -> Result<Response, ApiErr> {
    let deck = if let Some(md) = body.markdown {
        let deck = ppt_base::deck::assemble(&md, &ppt_types::new_deck_id(), "", "");
        let cache = serde_json::to_string(&ppt_base::CachedDeck {
            meta: deck.meta.clone(),
            slides: deck.slides.clone(),
        })
        .unwrap_or_default();
        shared
            .store
            .create_deck(&deck.meta.id, &body.title, &md, &deck.meta.revision, &cache)
            .await
            .map_err(api_err)?;
        let row = shared.store.get_deck_row(&deck.meta.id).await.map_err(api_err)?;
        let mut deck = deck;
        deck.meta.created_at = row.created_at;
        deck.meta.updated_at = row.updated_at;
        deck
    } else {
        shared
            .create_deck(&body.title, body.template.as_deref())
            .await
            .map_err(api_err)?
    };
    let (Json(payload),) = deck_payload(&shared, deck).await;
    Ok((StatusCode::CREATED, Json(payload)).into_response())
}

async fn get_deck(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path(id): Path<String>,
) -> Response {
    match shared.load_deck(&id).await {
        Ok(deck) => {
            let (Json(mut payload),) = deck_payload(&shared, deck).await;
            let etag = format!(
                "\"{}\"",
                payload["deck"]["revision"].as_str().unwrap_or_default()
            );
            (
                [(header::ETAG, etag)],
                Json(payload),
            )
                .into_response()
        }
        Err(e) => api_err(e).into_response(),
    }
}

async fn deck_payload(
    shared: &Arc<ppt_base::DeckShared>,
    deck: ppt_base::deck::Deck,
) -> (Json<Value>,) {
    let assets = shared.store.list_assets(&deck.meta.id).await.unwrap_or_default();
    let asset_rows: Vec<Value> = assets
        .iter()
        .map(|a| {
            json!({
                "assetId": a.id,
                "filename": a.filename,
                "mime": a.mime,
                "size": a.size,
                "caption": a.caption,
                "provenance": a.provenance,
                "ref": format!("assets://{}", a.id),
            })
        })
        .collect();
    (
        Json(json!({
            "deck": deck.meta,
            "slides": deck.slides,
            "assets": asset_rows,
        })),
    )
}

async fn put_source(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path(id): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    Json(body): Json<PutSourceBody>,
) -> ApiResult {
    let row = shared.store.get_deck_row(&id).await.map_err(api_err)?;
    // Optimistic concurrency: ?rev= in the query or `revision` in the body.
    if let Some(expected) = q
        .get("rev")
        .cloned()
        .or_else(|| body.revision.clone())
        .filter(|s| !s.is_empty())
    {
        if expected != row.revision {
            return Err(error(
                StatusCode::CONFLICT,
                format!(
                    "revision conflict: deck is at {}, request was based on {expected}",
                    row.revision
                ),
            ));
        }
    }
    let deck = shared
        .save_markdown(&id, &body.markdown, body.message.as_deref().unwrap_or("set source"))
        .await
        .map_err(api_err)?;
    Ok(Json(json!({
        "deck": deck.meta,
        "slideCount": deck.len(),
    })))
}

async fn delete_deck(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    // blob cleanup best-effort, then the row cascade
    for a in shared.store.list_assets(&id).await.map_err(api_err)? {
        let _ = shared.assets.delete(&a.blob_key).await;
    }
    for e in shared.store.list_exports(&id).await.map_err(api_err)? {
        if let Some(key) = e.artifact_key {
            let _ = shared.exports.delete(&key).await;
        }
    }
    shared.store.delete_deck(&id).await.map_err(api_err)?;
    Ok(Json(json!({ "deleted": id })))
}

// ---------------------------------------------------------------------------
// Slide endpoints
// ---------------------------------------------------------------------------

async fn deck_markdown(shared: &Arc<ppt_base::DeckShared>, id: &str) -> Result<String, ApiErr> {
    Ok(shared.store.get_deck_row(id).await.map_err(api_err)?.markdown)
}

async fn get_slide(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path((id, no)): Path<(String, u32)>,
) -> ApiResult {
    let deck = shared.load_deck(&id).await.map_err(api_err)?;
    let total = deck.len();
    let slide = deck.slide(no as usize).ok_or_else(|| {
        error(
            StatusCode::NOT_FOUND,
            format!("deck {id} has {total} slides (1-{total}); got slide {no}"),
        )
    })?;
    Ok(Json(json!({ "slide": slide, "slideCount": total })))
}

async fn patch_slide(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path((id, no)): Path<(String, u32)>,
    Json(body): Json<PatchSlideBody>,
) -> ApiResult {
    let md = deck_markdown(&shared, &id).await?;
    let md = ppt_base::operations::apply_slide_patch(
        &md,
        &id,
        no as usize,
        SlidePatch {
            content: body.content,
            frontmatter_raw: body.frontmatter_raw,
            note: body.note,
        },
    )
    .map_err(api_err)?;
    let deck = shared
        .save_markdown(&id, &md, &format!("patch slide {no}"))
        .await
        .map_err(api_err)?;
    Ok(Json(json!({
        "deck": deck.meta,
        "slideCount": deck.len(),
    })))
}

async fn insert_slide_route(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path(id): Path<String>,
    Json(body): Json<InsertSlideBody>,
) -> Result<Response, ApiErr> {
    let md = deck_markdown(&shared, &id).await?;
    let md = insert_slide(
        &md,
        &id,
        body.after as usize,
        body.content.as_deref().unwrap_or(""),
        body.frontmatter_raw.as_deref(),
        body.note.as_deref(),
    )
    .map_err(api_err)?;
    let deck = shared
        .save_markdown(&id, &md, &format!("insert after {}", body.after))
        .await
        .map_err(api_err)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "deck": deck.meta,
            "slideCount": deck.len(),
            "insertedNo": body.after as u64 + 1,
        })),
    )
        .into_response())
}

async fn delete_slide(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path((id, no)): Path<(String, u32)>,
) -> ApiResult {
    let md = deck_markdown(&shared, &id).await?;
    let md = remove_slide(&md, &id, no as usize).map_err(api_err)?;
    let deck = shared
        .save_markdown(&id, &md, &format!("remove slide {no}"))
        .await
        .map_err(api_err)?;
    Ok(Json(json!({ "deck": deck.meta, "slideCount": deck.len() })))
}

async fn move_slide_route(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path((id, no)): Path<(String, u32)>,
    Json(body): Json<MoveSlideBody>,
) -> ApiResult {
    let md = deck_markdown(&shared, &id).await?;
    let md = move_slide(&md, &id, no as usize, body.to as usize).map_err(api_err)?;
    let deck = shared
        .save_markdown(&id, &md, &format!("move {no} -> {}", body.to))
        .await
        .map_err(api_err)?;
    Ok(Json(json!({ "deck": deck.meta, "slideCount": deck.len() })))
}

// ---------------------------------------------------------------------------
// Asset endpoints
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct AssetListQuery {
    #[serde(default)]
    limit: Option<usize>,
}

async fn list_assets(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path(id): Path<String>,
    Query(q): Query<AssetListQuery>,
) -> ApiResult {
    // validate deck exists
    shared.store.get_deck_row(&id).await.map_err(api_err)?;
    let mut assets = shared.store.list_assets(&id).await.map_err(api_err)?;
    if let Some(limit) = q.limit {
        assets.truncate(limit);
    }
    let rows: Vec<Value> = assets
        .iter()
        .map(|a| {
            json!({
                "assetId": a.id,
                "filename": a.filename,
                "mime": a.mime,
                "size": a.size,
                "sha256": a.sha256,
                "caption": a.caption,
                "provenance": a.provenance,
                "ref": format!("assets://{}", a.id),
            })
        })
        .collect();
    Ok(Json(json!({ "assets": rows })))
}

async fn upload_asset(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<Response, ApiErr> {
    shared.store.get_deck_row(&id).await.map_err(api_err)?;

    let mut filename = String::new();
    let mut caption: Option<String> = None;
    let mut provenance: Option<String> = None;
    let mut bytes: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| error(StatusCode::BAD_REQUEST, format!("multipart: {e}")))?
    {
        match field.name().unwrap_or_default() {
            "file" => {
                filename = field
                    .file_name()
                    .unwrap_or("asset.bin")
                    .to_string();
                bytes = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| error(StatusCode::BAD_REQUEST, format!("read file: {e}")))?
                        .to_vec(),
                );
            }
            "caption" => {
                caption = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| error(StatusCode::BAD_REQUEST, format!("read caption: {e}")))?,
                );
            }
            "provenance" => {
                provenance = Some(
                    field.text().await.map_err(|e| {
                        error(StatusCode::BAD_REQUEST, format!("read provenance: {e}"))
                    })?,
                );
            }
            _ => {}
        }
    }

    let bytes = bytes
        .filter(|b| !b.is_empty())
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "missing 'file' field"))?;

    let asset_id = format!("asset-{}", uuid::Uuid::new_v4().simple());
    let blob_key = asset_blob_key(&id, &asset_id, &filename);
    shared
        .assets
        .put(&blob_key, bytes.clone())
        .await
        .map_err(api_err)?;

    let sha = {
        let mut h = Sha256::new();
        h.update(&bytes);
        format!("{:x}", h.finalize())
    };
    let mime = ppt_base::tools::asset_tools::guess_mime(&filename);
    let meta = ppt_base::store::AssetMeta {
        id: asset_id.clone(),
        deck_id: id.clone(),
        filename: filename.clone(),
        mime: mime.into(),
        size: bytes.len() as i64,
        sha256: sha,
        caption: caption.filter(|c| !c.is_empty()),
        provenance: provenance.filter(|p| !p.is_empty()),
        blob_key: blob_key.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    shared.store.add_asset(&meta).await.map_err(api_err)?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "assetId": asset_id,
            "filename": filename,
            "mime": mime,
            "size": bytes.len(),
            "ref": format!("assets://{asset_id}"),
        })),
    )
        .into_response())
}

async fn download_asset(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path((id, aid)): Path<(String, String)>,
) -> Response {
    serve_asset(&shared, &id, &aid).await
}

async fn print_asset(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path((id, aid)): Path<(String, String)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    // Token-authenticated sibling of `download_asset` for the export page.
    match q.get("token").map(String::as_str) {
        Some(token) if validate_print_token(&shared, &id, token).await.is_ok() => {
            serve_asset(&shared, &id, &aid).await
        }
        _ => error(StatusCode::UNAUTHORIZED, "invalid or expired print token")
            .into_response(),
    }
}

async fn serve_asset(shared: &Arc<ppt_base::DeckShared>, id: &str, aid: &str) -> Response {
    let meta = match shared.store.get_asset(id, aid).await {
        Ok(m) => m,
        Err(e) => return api_err(e).into_response(),
    };
    match shared.assets.get(&meta.blob_key).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, meta.mime.clone()),
                (header::CACHE_CONTROL, "private, max-age=3600".to_string()),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => api_err(e).into_response(),
    }
}

async fn delete_asset(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path((id, aid)): Path<(String, String)>,
) -> ApiResult {
    let blob_key = shared
        .store
        .delete_asset(&id, &aid)
        .await
        .map_err(api_err)?;
    let _ = shared.assets.delete(&blob_key).await;
    Ok(Json(json!({ "deleted": aid })))
}

// ---------------------------------------------------------------------------
// Export endpoints
// ---------------------------------------------------------------------------

async fn list_exports(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    let exports = shared.store.list_exports(&id).await.map_err(api_err)?;
    Ok(Json(json!({ "exports": exports.iter().map(export_json).collect::<Vec<_>>() })))
}

fn export_json(e: &ExportRecord) -> Value {
    json!({
        "exportId": e.id,
        "deckId": e.deck_id,
        "format": e.format.as_str(),
        "status": e.status.as_str(),
        "artifactSize": e.artifact_size,
        "error": e.error,
        "createdAt": e.created_at,
        "updatedAt": e.updated_at,
    })
}

async fn create_export(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path(id): Path<String>,
    Json(body): Json<CreateExportBody>,
) -> Result<Response, ApiErr> {
    let format = ExportFormat::parse(&body.format)
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "format must be pdf | png | pptx"))?;
    let deck = shared.load_deck(&id).await.map_err(api_err)?;
    let pages = deck
        .resolve_range(body.range.as_deref())
        .map_err(|e| api_err(ppt_base::Error::BadRange(e)))?;
    let dpi = body.dpi.unwrap_or(192).clamp(96, 300);

    // Token minting + queueing live in the engine (worker consumes the
    // raw token; only its hash is persisted).
    let (record, _token) = shared
        .enqueue_export(
            &id,
            format,
            dpi,
            pages.iter().map(|p| p + 1).collect(),
            deck.meta.canvas(),
            deck.meta.canvas_height(),
        )
        .await
        .map_err(api_err)?;

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "exportId": record.id,
            "deckId": id,
            "format": format.as_str(),
            "status": record.status.as_str(),
            "dpi": dpi,
            "pages": pages.iter().map(|p| p + 1).collect::<Vec<_>>(),
        })),
    )
        .into_response())
}

async fn get_export(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path(eid): Path<String>,
) -> ApiResult {
    let record = shared.store.get_export(&eid).await.map_err(api_err)?;
    Ok(Json(export_json(&record)))
}

async fn download_export(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path(eid): Path<String>,
) -> Response {
    let record = match shared.store.get_export(&eid).await {
        Ok(r) => r,
        Err(e) => return api_err(e).into_response(),
    };
    let Some(key) = record.artifact_key.clone() else {
        return error(
            StatusCode::NOT_FOUND,
            format!("export {eid} has no artifact yet (status: {})", record.status.as_str()),
        )
        .into_response();
    };
    match shared.exports.get(&key).await {
        Ok(bytes) => {
            let mime = record
                .artifact_mime
                .clone()
                .unwrap_or_else(|| record.format.mime().to_string());
            let ext = record.format.as_str();
            (
                [
                    (header::CONTENT_TYPE, mime),
                    (
                        header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"{eid}.{ext}\""),
                    ),
                ],
                bytes,
            )
                .into_response()
        }
        Err(e) => api_err(e).into_response(),
    }
}

// ---------------------------------------------------------------------------
// Print token surface
// ---------------------------------------------------------------------------

fn print_token_hash(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    format!("{:x}", h.finalize())
}

async fn validate_print_token(
    shared: &Arc<ppt_base::DeckShared>,
    deck_id: &str,
    token: &str,
) -> Result<ppt_base::store::ExportRecord, ApiErr> {
    let record = shared
        .store
        .get_export_by_token(deck_id, &print_token_hash(token))
        .await
        .map_err(|_| error(StatusCode::UNAUTHORIZED, "invalid print token"))?;
    if let Some(expiry) = &record.token_expires_at {
        let t = chrono::DateTime::parse_from_rfc3339(expiry)
            .map_err(|_| error(StatusCode::UNAUTHORIZED, "invalid token expiry"))?;
        if chrono::Utc::now() > t {
            return Err(error(StatusCode::UNAUTHORIZED, "print token expired"));
        }
    }
    Ok(record)
}

/// Full render payload for the headless export page: deck meta, slides,
/// and asset URLs (token-gated siblings).
async fn print_data(
    State(shared): State<Arc<ppt_base::DeckShared>>,
    Path(id): Path<String>,
    Query(q): Query<PrintQuery>,
) -> ApiResult {
    let token = q.token.clone().unwrap_or_default();
    validate_print_token(&shared, &id, &token).await?;
    let deck = shared.load_deck(&id).await.map_err(api_err)?;
    let assets = shared.store.list_assets(&id).await.map_err(api_err)?;
    let asset_rows: Vec<Value> = assets
        .iter()
        .map(|a| {
            json!({
                "assetId": a.id,
                "ref": format!("assets://{}", a.id),
                "url": format!(
                    "/api/v1/deck/decks/{id}/print-assets/{}?token={}",
                    a.id, token
                ),
                "mime": a.mime,
            })
        })
        .collect();
    Ok(Json(json!({
        "deck": deck.meta,
        "slides": deck.slides,
        "assets": asset_rows,
        "token": token,
    })))
}

// ---------------------------------------------------------------------------
// Base64 helper re-export (api-server uploads multipart, but keep the
// engine import for parity)
// ---------------------------------------------------------------------------
pub use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
