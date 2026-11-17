//! RSS/Atom feed subscriptions: feeds, background scheduler, item inbox.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use bib_base::{BibShared, rss};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
type Failure = (StatusCode, Json<Value>);
type Result<T> = std::result::Result<T, Failure>;
fn error(error: rss::RssError) -> Failure {
    let status = match &error {
        rss::RssError::Invalid(_) => StatusCode::BAD_REQUEST,
        rss::RssError::Conflict(_) => StatusCode::CONFLICT,
        rss::RssError::NotFound => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(json!({"error": error.to_string()})))
}
pub(crate) fn router() -> Router<Arc<BibShared>> {
    Router::new()
        .route("/rss/feeds", get(list_feeds).post(create_feed))
        .route("/rss/status", get(status))
        .route(
            "/rss/feeds/{id}",
            get(read_feed)
                .put(replace_feed)
                .delete(delete_feed)
                .post(check_feed),
        )
        .route("/rss/feeds/{id}/enabled", post(set_enabled))
        .route(
            "/rss/items/{id}",
            get(read_item)
                .put(set_item_read)
                .delete(delete_item)
                .post(import_item),
        )
        .route("/rss/items", get(items))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(axum::middleware::from_fn(crate::cache_control::no_cache))
}
async fn scheduler(shared: &BibShared) -> Result<rss::RssScheduler> {
    shared.rss_service().await.map_err(error)
}
#[utoipa::path(method(get), path = "/api/v1/bib/rss/feeds", tag = "bibliography-rss",
    operation_id = "bib_rss_list_feeds",
    summary = "List feeds",
    responses((status = 200, description = "List feeds", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn list_feeds(State(shared): State<Arc<BibShared>>) -> Result<Json<Value>> {
    Ok(Json(
        json!({"feeds": shared.rss.list_feeds().await.map_err(error)?}),
    ))
}
#[utoipa::path(method(get), path = "/api/v1/bib/rss/status", tag = "bibliography-rss",
    operation_id = "bib_rss_status",
    summary = "Status",
    responses((status = 200, description = "Status", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn status(State(shared): State<Arc<BibShared>>) -> Result<Json<Value>> {
    Ok(Json(
        json!({"scheduler": scheduler(&shared).await?.status()}),
    ))
}
#[utoipa::path(method(post), path = "/api/v1/bib/rss/feeds", tag = "bibliography-rss",
    operation_id = "bib_rss_create_feed",
    summary = "Create feed",
    request_body = rss::FeedSpec,
    responses((status = 201, description = "Create feed", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn create_feed(
    State(shared): State<Arc<BibShared>>,
    Json(spec): Json<rss::FeedSpec>,
) -> Result<(StatusCode, Json<Value>)> {
    let feed = shared.rss.create_feed(spec).await.map_err(error)?;
    scheduler(&shared).await?;
    Ok((StatusCode::CREATED, Json(json!({"feed": feed}))))
}
#[utoipa::path(method(get), path = "/api/v1/bib/rss/feeds/{id}", tag = "bibliography-rss",
    operation_id = "bib_rss_read_feed",
    summary = "Read feed",
    params(("id" = String, Path, description = "id")),
    responses((status = 200, description = "Read feed", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn read_feed(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    Ok(Json(
        json!({"feed": shared.rss.get_feed(&id).await.map_err(error)?}),
    ))
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct Replace {
    version: u64,
    spec: rss::FeedSpec,
}
#[utoipa::path(method(put), path = "/api/v1/bib/rss/feeds/{id}", tag = "bibliography-rss",
    operation_id = "bib_rss_replace_feed",
    summary = "Replace feed",
    params(("id" = String, Path, description = "id")),
    request_body = Replace,
    responses((status = 200, description = "Replace feed", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn replace_feed(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<Replace>,
) -> Result<Json<Value>> {
    Ok(Json(
        json!({"feed": shared.rss.replace_feed(&id, input.version, input.spec).await.map_err(error)?}),
    ))
}
#[derive(Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
struct Version {
    version: u64,
}
#[utoipa::path(method(delete), path = "/api/v1/bib/rss/feeds/{id}", tag = "bibliography-rss",
    operation_id = "bib_rss_delete_feed",
    summary = "Delete feed",
    params(("id" = String, Path, description = "id"), Version),
    responses((status = 204, description = "Completed"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn delete_feed(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Query(input): Query<Version>,
) -> Result<StatusCode> {
    shared
        .rss
        .delete_feed(&id, input.version)
        .await
        .map_err(error)?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct Enabled {
    version: u64,
    enabled: bool,
}
#[utoipa::path(method(post), path = "/api/v1/bib/rss/feeds/{id}/enabled", tag = "bibliography-rss",
    operation_id = "bib_rss_set_enabled",
    summary = "Set enabled",
    params(("id" = String, Path, description = "id")),
    request_body = Enabled,
    responses((status = 200, description = "Set enabled", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn set_enabled(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<Enabled>,
) -> Result<Json<Value>> {
    let feed = shared
        .rss
        .set_enabled(&id, input.version, input.enabled)
        .await
        .map_err(error)?;
    scheduler(&shared).await?;
    Ok(Json(json!({"feed": feed})))
}
/// Fetch one feed now. A failing fetch still returns the outcome payload so
/// the GUI can render per-feed errors; only transport/authorization fails.
#[utoipa::path(method(post), path = "/api/v1/bib/rss/feeds/{id}", tag = "bibliography-rss",
    operation_id = "bib_rss_check_feed",
    summary = "Check feed",
    params(("id" = String, Path, description = "id")),
    responses((status = 200, description = "Check feed", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn check_feed(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    scheduler(&shared).await?;
    match rss::check_feed(&shared, &id).await {
        Ok(outcome) => Ok(Json(json!({"outcome": outcome}))),
        Err(failure @ rss::RssError::NotFound) => Err(error(failure)),
        Err(failure) => Ok(Json(json!({"outcome": {
            "feed_id": id, "status": "error", "new_items": 0, "total_items": 0,
            "error": failure.to_string()
        }}))),
    }
}
#[derive(Deserialize, utoipa::IntoParams)]
#[serde(default, deny_unknown_fields)]
struct ItemPage {
    feed: Option<String>,
    before: Option<i64>,
    limit: u16,
}
impl Default for ItemPage {
    fn default() -> Self {
        Self {
            feed: None,
            before: None,
            limit: 50,
        }
    }
}
#[utoipa::path(method(get), path = "/api/v1/bib/rss/items", tag = "bibliography-rss",
    operation_id = "bib_rss_items",
    summary = "Items",
    params(ItemPage),
    responses((status = 200, description = "Items", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn items(
    State(shared): State<Arc<BibShared>>,
    Query(page): Query<ItemPage>,
) -> Result<Json<Value>> {
    let (items, next) = shared
        .rss
        .items(page.feed.as_deref(), page.before, page.limit.clamp(1, 200))
        .await
        .map_err(error)?;
    Ok(Json(json!({"items": items, "next": next})))
}
#[utoipa::path(method(get), path = "/api/v1/bib/rss/items/{id}", tag = "bibliography-rss",
    operation_id = "bib_rss_read_item",
    summary = "Read item",
    params(("id" = String, Path, description = "id")),
    responses((status = 200, description = "Read item", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn read_item(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    Ok(Json(
        json!({"item": shared.rss.get_item(&id).await.map_err(error)?}),
    ))
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct Read {
    read: bool,
}
#[utoipa::path(method(put), path = "/api/v1/bib/rss/items/{id}", tag = "bibliography-rss",
    operation_id = "bib_rss_set_item_read",
    summary = "Set item read",
    params(("id" = String, Path, description = "id")),
    request_body = Read,
    responses((status = 204, description = "Completed"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn set_item_read(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<Read>,
) -> Result<StatusCode> {
    shared
        .rss
        .set_item_read(&id, input.read)
        .await
        .map_err(error)?;
    Ok(StatusCode::NO_CONTENT)
}
#[utoipa::path(method(delete), path = "/api/v1/bib/rss/items/{id}", tag = "bibliography-rss",
    operation_id = "bib_rss_delete_item",
    summary = "Delete item",
    params(("id" = String, Path, description = "id")),
    responses((status = 204, description = "Completed"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn delete_item(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    shared.rss.delete_item(&id).await.map_err(error)?;
    Ok(StatusCode::NO_CONTENT)
}
/// Import one inbox item into the library (optionally into a collection).
#[derive(Deserialize, Default, utoipa::ToSchema)]
#[serde(default, deny_unknown_fields)]
struct Import {
    collection_id: Option<String>,
}
#[utoipa::path(method(post), path = "/api/v1/bib/rss/items/{id}", tag = "bibliography-rss",
    operation_id = "bib_rss_import_item",
    summary = "Import item",
    params(("id" = String, Path, description = "id")),
    request_body = Import,
    responses((status = 200, description = "Import item", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn import_item(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<Import>,
) -> Result<Json<Value>> {
    let item = shared.rss.get_item(&id).await.map_err(error)?;
    if let Some(article_id) = &item.imported_article_id {
        return Ok(Json(
            json!({"article_id": article_id, "already_imported": true}),
        ));
    }
    let article = rss::article_from_item(&item);
    match &input.collection_id {
        Some(collection_id) => {
            shared
                .bib
                .upsert_articles_with_collection(
                    std::slice::from_ref(&article),
                    collection_id,
                    &[bib_base::CollectionAssignment {
                        article_id: article.id.clone(),
                        role: bib_types::ArticleRole::Requested,
                        added_by: bib_types::AddedBy::User,
                        note: Some("Imported from RSS subscription".into()),
                    }],
                )
                .await
                .map_err(|e| error(rss::RssError::Fetch(e.to_string())))?;
        }
        None => {
            shared
                .bib
                .upsert_article(&article)
                .await
                .map_err(|e| error(rss::RssError::Fetch(e.to_string())))?;
        }
    }
    shared
        .rss
        .mark_imported(&id, &article.id)
        .await
        .map_err(error)?;
    Ok(Json(
        json!({"article_id": article.id, "already_imported": false}),
    ))
}
