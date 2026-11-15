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
async fn list_feeds(State(shared): State<Arc<BibShared>>) -> Result<Json<Value>> {
    Ok(Json(json!({"feeds": shared.rss.list_feeds().await.map_err(error)?})))
}
async fn status(State(shared): State<Arc<BibShared>>) -> Result<Json<Value>> {
    Ok(Json(json!({"scheduler": scheduler(&shared).await?.status()})))
}
async fn create_feed(
    State(shared): State<Arc<BibShared>>,
    Json(spec): Json<rss::FeedSpec>,
) -> Result<(StatusCode, Json<Value>)> {
    let feed = shared.rss.create_feed(spec).await.map_err(error)?;
    scheduler(&shared).await?;
    Ok((StatusCode::CREATED, Json(json!({"feed": feed}))))
}
async fn read_feed(State(shared): State<Arc<BibShared>>, Path(id): Path<String>) -> Result<Json<Value>> {
    Ok(Json(json!({"feed": shared.rss.get_feed(&id).await.map_err(error)?})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Replace {
    version: u64,
    spec: rss::FeedSpec,
}
async fn replace_feed(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<Replace>,
) -> Result<Json<Value>> {
    Ok(Json(
        json!({"feed": shared.rss.replace_feed(&id, input.version, input.spec).await.map_err(error)?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Version {
    version: u64,
}
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
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Enabled {
    version: u64,
    enabled: bool,
}
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
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ItemPage {
    feed: Option<String>,
    before: Option<i64>,
    limit: u16,
}
impl Default for ItemPage {
    fn default() -> Self {
        Self { feed: None, before: None, limit: 50 }
    }
}
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
async fn read_item(State(shared): State<Arc<BibShared>>, Path(id): Path<String>) -> Result<Json<Value>> {
    Ok(Json(json!({"item": shared.rss.get_item(&id).await.map_err(error)?})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    read: bool,
}
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
async fn delete_item(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    shared.rss.delete_item(&id).await.map_err(error)?;
    Ok(StatusCode::NO_CONTENT)
}
/// Import one inbox item into the library (optionally into a collection).
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Import {
    collection_id: Option<String>,
}
async fn import_item(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<Import>,
) -> Result<Json<Value>> {
    let item = shared.rss.get_item(&id).await.map_err(error)?;
    if let Some(article_id) = &item.imported_article_id {
        return Ok(Json(json!({"article_id": article_id, "already_imported": true})));
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
    Ok(Json(json!({"article_id": article.id, "already_imported": false})))
}
