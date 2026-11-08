//! Deck REST module: decks / slides / assets / exports, plus the
//! print-token-authenticated open surface for the headless-Chromium
//! export page.
//!
//! Mounted at `/api/v1/deck`. The state is [`ppt_base::DeckHandle`] —
//! the deck write actor. Mutating handlers enqueue one write closure
//! (read-modify-write sequences serialize on the worker); read handlers
//! hit `handle.shared()` directly.

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

use ppt_base::DeckHandle;
use ppt_base::assets::asset_blob_key;
use ppt_base::operations::{SlidePatch, insert_slide, move_slide, remove_slide};
use ppt_base::store::{ExportFormat, ExportRecord, ExportStatus};

const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

type ApiErr = (StatusCode, Json<ApiError>);
type ApiResult = Result<Json<Value>, ApiErr>;
type WriteResult = Result<Response, ApiErr>;

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
        ppt_base::Error::InvalidThemeCss(m) => error(StatusCode::BAD_REQUEST, m),
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

/// Box a write-closure future into the shape [`DeckHandle::write`]
/// expects, keeping call sites free of pin/cast noise.
fn box_w<T, F>(f: F) -> std::pin::Pin<Box<dyn std::future::Future<Output = ppt_base::Result<T>> + Send>>
where
    T: Send + 'static,
    F: std::future::Future<Output = ppt_base::Result<T>> + Send + 'static,
{
    Box::pin(f)
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

/// The guarded API (session/bearer).
pub fn router(handle: Arc<DeckHandle>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/decks", get(list_decks).post(create_deck))
        .route(
            "/decks/{id}",
            get(get_deck).delete(delete_deck),
        )
        .route("/decks/{id}/source", get(get_source).put(put_source))
        .route("/decks/{id}/theme.css", get(get_theme_css).put(put_theme_css))
        .route("/decks/{id}/default-theme.css", get(get_default_theme_css))
        .route("/decks/{id}/slides/{no}/css", put(put_slide_css))
        .route(
            "/decks/{id}/diagnostics",
            get(get_diagnostics).post(run_diagnostics),
        )
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
        .with_state(handle)
}

/// The open print surface (headless export page). Token-validated in the
/// handlers; nested without the bearer/session layer.
pub fn open_router(handle: Arc<DeckHandle>) -> Router {
    Router::new()
        .route("/decks/{id}/print-data", get(print_data))
        .route("/decks/{id}/print-assets/{aid}", get(print_asset))
        .with_state(handle)
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

async fn list_decks(State(handle): State<Arc<DeckHandle>>) -> ApiResult {
    let shared = handle.shared().clone();
    let decks = shared.store.list_decks().await.map_err(api_err)?;
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
    State(handle): State<Arc<DeckHandle>>,
    Json(body): Json<CreateDeckBody>,
) -> WriteResult {
    let deck = handle
        .write({
            let title = body.title.clone();
            let template = body.template.clone();
            let markdown = body.markdown.clone();
            move |s: Arc<ppt_base::shared::DeckShared>| {
                box_w(async move {
                    if let Some(md) = markdown {
                        let deck =
                            ppt_base::deck::assemble(&md, &ppt_types::new_deck_id(), "", "");
                        let cache = serde_json::to_string(&ppt_base::CachedDeck {
                            meta: deck.meta.clone(),
                            slides: deck.slides.clone(),
                        })
                        .unwrap_or_default();
                        s.store
                            .create_deck(&deck.meta.id, &title, &md, &deck.meta.revision, &cache)
                            .await?;
                        if let Err(error) = s.assets.put(&format!("themes/{}/theme.css", deck.meta.id), ppt_base::theme::starter_css(&deck.meta).into_bytes()).await {
                            let _ = s.store.delete_deck(&deck.meta.id).await;
                            return Err(error);
                        }
                        let row = s.store.get_deck_row(&deck.meta.id).await?;
                        let mut deck = deck;
                        deck.meta.created_at = row.created_at;
                        deck.meta.updated_at = row.updated_at;
                        Ok(deck)
                    } else {
                        s.create_deck(&title, template.as_deref()).await
                    }
                })
            }
        })
        .await
        .map_err(api_err)?;
    let Json(payload) = deck_payload(&handle, deck).await?;
    Ok((StatusCode::CREATED, Json(payload)).into_response())
}

async fn get_deck(State(handle): State<Arc<DeckHandle>>, Path(id): Path<String>) -> Response {
    match handle.shared().load_deck(&id).await {
        Ok(deck) => {
            let Json(payload) = match deck_payload(&handle, deck).await {
                Ok(payload) => payload,
                Err(err) => return err.into_response(),
            };
            let etag = format!(
                "\"{}\"",
                payload["deck"]["revision"].as_str().unwrap_or_default()
            );
            ([(header::ETAG, etag)], Json(payload)).into_response()
        }
        Err(e) => api_err(e).into_response(),
    }
}

async fn deck_payload(
    handle: &Arc<DeckHandle>,
    deck: ppt_base::deck::Deck,
) -> ApiResult {
    let shared = handle.shared();
    let theme_css = shared.theme_css(&deck.meta.id).await.map_err(api_err)?;
    let theme_revision = ppt_base::shared::css_revision(&theme_css);
    let canvas_size = ppt_base::theme::canvas_size(&theme_css, ppt_base::theme::CanvasSize { width: deck.meta.canvas(), height: deck.meta.canvas_height() }).map_err(api_err)?;
    let diagnostics = shared
        .latest_diagnostics(&deck.meta.id)
        .await
        .unwrap_or(None)
        .map(|report| {
            let (errors, warnings, infos) = report.counts();
            json!({
                "fresh": report.is_fresh(&deck.meta.revision, &theme_revision),
                "rendered": report.rendered,
                "errors": errors,
                "warnings": warnings,
                "infos": infos,
                "createdAt": report.created_at,
            })
        });
    let assets = shared
        .store
        .list_assets(&deck.meta.id)
        .await
        .unwrap_or_default();
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
    let outline = ppt_base::outline::build_outline(&deck.slides);
    Ok(Json(json!({
        "deck": deck.meta,
        "slides": deck.slides,
        "outline": outline,
        "assets": asset_rows,
        "themeCss": theme_css,
        "themeRevision": theme_revision,
        "canvasSize": canvas_size,
        "diagnostics": diagnostics,
    })))
}

#[derive(Deserialize)]
struct PutCssBody {
    css: String,
    revision: String,
}

async fn get_theme_css(State(handle): State<Arc<DeckHandle>>, Path(id): Path<String>) -> ApiResult {
    let css = handle.shared().theme_css(&id).await.map_err(api_err)?;
    Ok(Json(json!({ "revision": ppt_base::shared::css_revision(&css), "css": css })))
}

async fn get_default_theme_css(State(handle): State<Arc<DeckHandle>>, Path(id): Path<String>) -> ApiResult {
    let deck = handle.shared().load_deck(&id).await.map_err(api_err)?;
    let size = handle.shared().canvas_size(&id, &deck.meta).await.map_err(api_err)?;
    Ok(Json(json!({ "css": ppt_base::theme::css_for_size(size) })))
}

async fn put_theme_css(State(handle): State<Arc<DeckHandle>>, Path(id): Path<String>, Json(body): Json<PutCssBody>) -> ApiResult {
    let revision = handle.write(move |s| box_w(async move {
        s.save_theme_css(&id, &body.css, &body.revision).await
    })).await.map_err(api_err)?;
    Ok(Json(json!({ "revision": revision })))
}

async fn put_slide_css(State(handle): State<Arc<DeckHandle>>, Path((id, no)): Path<(String, usize)>, Json(body): Json<PutCssBody>) -> ApiResult {
    let deck = handle.write(move |s| box_w(async move {
        let row = s.store.get_deck_row(&id).await?;
        if row.revision != body.revision {
            return Err(ppt_base::Error::RevisionConflict { expected: row.revision, actual: body.revision });
        }
        let markdown = ppt_base::operations::set_slide_css(&row.markdown, &id, no, &body.css)?;
        s.save_markdown(&id, &markdown, "set slide CSS").await
    })).await.map_err(api_err)?;
    Ok(Json(json!({ "revision": deck.meta.revision })))
}

async fn get_source(
    State(handle): State<Arc<DeckHandle>>,
    Path(id): Path<String>,
) -> ApiResult {
    let row = handle.shared().store.get_deck_row(&id).await.map_err(api_err)?;
    Ok(Json(json!({ "markdown": row.markdown, "revision": row.revision })))
}

/// Latest stored diagnostics report (post-write background run), plus a
/// freshness flag against the current markdown + theme revisions.
async fn get_diagnostics(State(handle): State<Arc<DeckHandle>>, Path(id): Path<String>) -> ApiResult {
    let shared = handle.shared();
    let deck = shared.load_deck(&id).await.map_err(api_err)?;
    let theme_revision = shared
        .theme_css(&id)
        .await
        .map(|css| ppt_base::shared::css_revision(&css))
        .map_err(api_err)?;
    let report = shared.latest_diagnostics(&id).await.map_err(api_err)?;
    Ok(Json(json!({
        "deckId": id,
        "fresh": report.as_ref().is_some_and(|r| r.is_fresh(&deck.meta.revision, &theme_revision)),
        "currentRevision": deck.meta.revision,
        "currentThemeRevision": theme_revision,
        "report": report,
    })))
}

/// Force a synchronous diagnostics run (heavy: renders every slide).
async fn run_diagnostics(State(handle): State<Arc<DeckHandle>>, Path(id): Path<String>) -> ApiResult {
    let report = ppt_base::diagnostics::run_diagnostics(handle.shared(), &id)
        .await
        .map_err(api_err)?;
    Ok(Json(serde_json::to_value(&report).unwrap_or_default()))
}

async fn put_source(
    State(handle): State<Arc<DeckHandle>>,
    Path(id): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    Json(body): Json<PutSourceBody>,
) -> WriteResult {
    let expected = q.get("rev").cloned().or(body.revision).filter(|s| !s.is_empty());
    let deck = handle.write(move |s: Arc<ppt_base::shared::DeckShared>| {
        box_w(async move {
            let row = s.store.get_deck_row(&id).await?;
            if let Some(expected) = expected {
                if expected != row.revision {
                    return Err(ppt_base::Error::RevisionConflict { expected: row.revision, actual: expected });
                }
            }
            s.save_markdown(&id, &body.markdown, body.message.as_deref().unwrap_or("set source")).await
        })
    }).await.map_err(api_err)?;
    Ok(Json(json!({ "deck": deck.meta, "slideCount": deck.len() })).into_response())
}

async fn delete_deck(
    State(handle): State<Arc<DeckHandle>>,
    Path(id): Path<String>,
) -> ApiResult {
    let deleted = id.clone();
    handle
        .write(move |s: Arc<ppt_base::shared::DeckShared>| {
            Box::pin(async move {
                // blob cleanup best-effort, then the row cascade
                for a in s.store.list_assets(&deleted).await? {
                    let _ = s.assets.delete(&a.blob_key).await;
                }
                for e in s.store.list_exports(&deleted).await? {
                    if let Some(key) = e.artifact_key {
                        let _ = s.exports.delete(&key).await;
                    }
                }
                let _ = s.assets.delete(&format!("themes/{deleted}/theme.css")).await;
                s.store.delete_deck(&deleted).await
            })
        })
        .await
        .map_err(api_err)?;
    Ok(Json(json!({ "deleted": id })))
}

// ---------------------------------------------------------------------------
// Slide endpoints
// ---------------------------------------------------------------------------

async fn get_slide(
    State(handle): State<Arc<DeckHandle>>,
    Path((id, no)): Path<(String, u32)>,
) -> ApiResult {
    let deck = handle.shared().load_deck(&id).await.map_err(api_err)?;
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
    State(handle): State<Arc<DeckHandle>>,
    Path((id, no)): Path<(String, u32)>,
    Json(body): Json<PatchSlideBody>,
) -> ApiResult {
    let deck = handle
        .write({
            let id = id.clone();
            move |s: Arc<ppt_base::shared::DeckShared>| {
                box_w(async move {
                    let row = s.store.get_deck_row(&id).await?;
                    let md = ppt_base::operations::apply_slide_patch(
                        &row.markdown,
                        &id,
                        no as usize,
                        SlidePatch {
                            content: body.content,
                            frontmatter_raw: body.frontmatter_raw,
                            note: body.note,
                        },
                    )?;
                    s.save_markdown(&id, &md, &format!("patch slide {no}")).await
                })
            }
        })
        .await
        .map_err(api_err)?;
    Ok(Json(json!({
        "deck": deck.meta,
        "slideCount": deck.len(),
    })))
}

async fn insert_slide_route(
    State(handle): State<Arc<DeckHandle>>,
    Path(id): Path<String>,
    Json(body): Json<InsertSlideBody>,
) -> WriteResult {
    let after = body.after;
    let deck = handle
        .write({
            let id = id.clone();
            move |s: Arc<ppt_base::shared::DeckShared>| {
                box_w(async move {
                    let row = s.store.get_deck_row(&id).await?;
                    let md = insert_slide(
                        &row.markdown,
                        &id,
                        after as usize,
                        body.content.as_deref().unwrap_or(""),
                        body.frontmatter_raw.as_deref(),
                        body.note.as_deref(),
                    )?;
                    s.save_markdown(&id, &md, &format!("insert after {after}"))
                        .await
                })
            }
        })
        .await
        .map_err(api_err)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "deck": deck.meta,
            "slideCount": deck.len(),
            "insertedNo": after as u64 + 1,
        })),
    )
        .into_response())
}

async fn delete_slide(
    State(handle): State<Arc<DeckHandle>>,
    Path((id, no)): Path<(String, u32)>,
) -> ApiResult {
    let deck = handle
        .write({
            let id = id.clone();
            move |s: Arc<ppt_base::shared::DeckShared>| {
                box_w(async move {
                    let row = s.store.get_deck_row(&id).await?;
                    let md = remove_slide(&row.markdown, &id, no as usize)?;
                    s.save_markdown(&id, &md, &format!("remove slide {no}")).await
                })
            }
        })
        .await
        .map_err(api_err)?;
    Ok(Json(json!({ "deck": deck.meta, "slideCount": deck.len() })))
}

async fn move_slide_route(
    State(handle): State<Arc<DeckHandle>>,
    Path((id, no)): Path<(String, u32)>,
    Json(body): Json<MoveSlideBody>,
) -> ApiResult {
    let to = body.to;
    let deck = handle
        .write({
            let id = id.clone();
            move |s: Arc<ppt_base::shared::DeckShared>| {
                box_w(async move {
                    let row = s.store.get_deck_row(&id).await?;
                    let md = move_slide(&row.markdown, &id, no as usize, to as usize)?;
                    s.save_markdown(&id, &md, &format!("move {no} -> {to}")).await
                })
            }
        })
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
    State(handle): State<Arc<DeckHandle>>,
    Path(id): Path<String>,
    Query(q): Query<AssetListQuery>,
) -> ApiResult {
    let shared = handle.shared();
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
    State(handle): State<Arc<DeckHandle>>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> WriteResult {
    let shared = handle.shared();
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

    let asset = handle
        .write({
            let id = id.clone();
            let filename = filename.clone();
            let bytes = bytes.clone();
            move |s: Arc<ppt_base::shared::DeckShared>| {
                box_w(async move {
                    let asset_id = format!("asset-{}", uuid::Uuid::new_v4().simple());
                    let blob_key = asset_blob_key(&id, &asset_id, &filename);
                    s.assets.put(&blob_key, bytes.clone()).await?;
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
                        caption: caption.clone().filter(|c| !c.is_empty()),
                        provenance: provenance.clone().filter(|p| !p.is_empty()),
                        blob_key: blob_key.clone(),
                        created_at: chrono::Utc::now().to_rfc3339(),
                    };
                    s.store.add_asset(&meta).await?;
                    Ok(meta)
                })
            }
        })
        .await
        .map_err(api_err)?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "assetId": asset.id,
            "filename": asset.filename,
            "mime": asset.mime,
            "size": asset.size,
            "ref": format!("assets://{}", asset.id),
        })),
    )
        .into_response())
}

async fn download_asset(
    State(handle): State<Arc<DeckHandle>>,
    Path((id, aid)): Path<(String, String)>,
) -> Response {
    serve_asset(&handle, &id, &aid).await
}

async fn print_asset(
    State(handle): State<Arc<DeckHandle>>,
    Path((id, aid)): Path<(String, String)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    // Token-authenticated sibling of `download_asset` for the export page.
    match q.get("token").map(String::as_str) {
        Some(token) if validate_print_token(&handle, &id, token).await.is_ok() => {
            serve_asset(&handle, &id, &aid).await
        }
        _ => error(StatusCode::UNAUTHORIZED, "invalid or expired print token")
            .into_response(),
    }
}

async fn serve_asset(handle: &Arc<DeckHandle>, id: &str, aid: &str) -> Response {
    let shared = handle.shared();
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
    State(handle): State<Arc<DeckHandle>>,
    Path((id, aid)): Path<(String, String)>,
) -> ApiResult {
    let deleted = aid.clone();
    let blob_key = handle
        .write({
            let id = id.clone();
            move |s: Arc<ppt_base::shared::DeckShared>| {
                box_w(async move { s.store.delete_asset(&id, &deleted).await })
            }
        })
        .await
        .map_err(api_err)?;
    let _ = handle.shared().assets.delete(&blob_key).await;
    Ok(Json(json!({ "deleted": aid })))
}

// ---------------------------------------------------------------------------
// Export endpoints
// ---------------------------------------------------------------------------

async fn list_exports(
    State(handle): State<Arc<DeckHandle>>,
    Path(id): Path<String>,
) -> ApiResult {
    let exports = handle
        .shared()
        .store
        .list_exports(&id)
        .await
        .map_err(api_err)?;
    Ok(Json(
        json!({ "exports": exports.iter().map(export_json).collect::<Vec<_>>() }),
    ))
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
    State(handle): State<Arc<DeckHandle>>,
    Path(id): Path<String>,
    Json(body): Json<CreateExportBody>,
) -> WriteResult {
    let format = ExportFormat::parse(&body.format)
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "format must be pdf | png | pptx"))?;
    let dpi = body.dpi.unwrap_or(192).clamp(96, 300);
    let export_body = body;

    // Token minting + queueing live in the engine (worker consumes the
    // raw token; only its hash is persisted). Enqueue through the actor.
    let (record, pages) = handle
        .write({
            let id = id.clone();
            move |s: Arc<ppt_base::shared::DeckShared>| {
                Box::pin(async move {
                    let deck = s.load_deck(&id).await?;
                    let pages = deck
                        .resolve_range(export_body.range.as_deref())
                        .map_err(ppt_base::Error::BadRange)?;
                    let notes = pages
                        .iter()
                        .map(|p| {
                            deck.slide(p + 1)
                                .and_then(|sl| sl.note.clone())
                                .unwrap_or_default()
                        })
                        .collect();
                    let canvas = s.canvas_size(&id, &deck.meta).await?;
                    let (record, _token) = s
                        .enqueue_export(
                            &id,
                            format,
                            dpi,
                            pages.iter().map(|p| p + 1).collect(),
                            canvas.width,
                            canvas.height,
                            notes,
                        )
                        .await?;
                    Ok((record, pages))
                })
            }
        })
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
    State(handle): State<Arc<DeckHandle>>,
    Path(eid): Path<String>,
) -> ApiResult {
    let record = handle
        .shared()
        .store
        .get_export(&eid)
        .await
        .map_err(api_err)?;
    Ok(Json(export_json(&record)))
}

async fn download_export(
    State(handle): State<Arc<DeckHandle>>,
    Path(eid): Path<String>,
) -> Response {
    let shared = handle.shared();
    let record = match shared.store.get_export(&eid).await {
        Ok(r) => r,
        Err(e) => return api_err(e).into_response(),
    };
    let Some(key) = record.artifact_key.clone() else {
        return error(
            StatusCode::NOT_FOUND,
            format!(
                "export {eid} has no artifact yet (status: {})",
                record.status.as_str()
            ),
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
    handle: &Arc<DeckHandle>,
    deck_id: &str,
    token: &str,
) -> Result<ExportRecord, ApiErr> {
    let record = handle
        .shared()
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
    State(handle): State<Arc<DeckHandle>>,
    Path(id): Path<String>,
    Query(q): Query<PrintQuery>,
) -> ApiResult {
    let token = q.token.clone().unwrap_or_default();
    let export = validate_print_token(&handle, &id, &token).await?;
    let shared = handle.shared();
    let deck = shared.load_deck(&id).await.map_err(api_err)?;
    let assets = shared.store.list_assets(&id).await.map_err(api_err)?;
    let params: Value = serde_json::from_str(&export.params).unwrap_or_default();
    let theme_css = match params.get("themeCss").and_then(Value::as_str) {
        Some(css) => css.to_string(),
        None => shared.theme_css(&id).await.map_err(api_err)?,
    };
    let canvas_size = match (params["canvasWidth"].as_u64(), params["canvasHeight"].as_u64()) {
        (Some(w), Some(h)) if (16..=8192).contains(&w) && (16..=8192).contains(&h) => ppt_base::theme::CanvasSize { width: w as u32, height: h as u32 },
        _ => ppt_base::theme::canvas_size(&theme_css, ppt_base::theme::CanvasSize { width: deck.meta.canvas(), height: deck.meta.canvas_height() }).map_err(api_err)?,
    };
    let asset_rows: Vec<Value> = assets
        .iter()
        .map(|a| {
            json!({
                "assetId": a.id,
                "ref": format!("assets://{}", a.id),
                "url": format!(
                    "/api/v1/deck/decks/{id}/print-assets/{}?token={token}",
                    a.id
                ),
                "mime": a.mime,
            })
        })
        .collect();
    let outline = ppt_base::outline::build_outline(&deck.slides);
    Ok(Json(json!({
        "deck": deck.meta,
        "slides": deck.slides,
        "outline": outline,
        "assets": asset_rows,
        "token": token,
        "themeCss": theme_css,
        "canvasSize": canvas_size,
    })))
}

// ---------------------------------------------------------------------------
// Base64 helper re-export (api-server uploads multipart, but keep the
// engine import for parity)
// ---------------------------------------------------------------------------
pub use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
