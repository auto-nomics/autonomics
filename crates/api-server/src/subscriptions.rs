//! Durable local subscriptions; background checks outlive individual requests.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use bib_base::{BibShared, subscription_execution::now_ms, subscriptions::*};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
type Failure = (StatusCode, Json<Value>);
type Result<T> = std::result::Result<T, Failure>;
fn error(error: SubscriptionError) -> Failure {
    let status = match &error {
        SubscriptionError::Invalid(_) => StatusCode::BAD_REQUEST,
        SubscriptionError::Conflict(_) => StatusCode::CONFLICT,
        SubscriptionError::NotFound => StatusCode::NOT_FOUND,
        SubscriptionError::Budget(_) | SubscriptionError::RunBudget(_) => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(json!({"error":error.to_string()})))
}
pub(crate) fn router() -> Router<Arc<BibShared>> {
    Router::new()
        .route("/subscriptions", get(list).post(create))
        .route("/subscriptions/status", get(status))
        .route("/subscriptions/limits", get(limits).put(configure_limits))
        .route("/subscriptions/inbox", get(inbox))
        .route(
            "/subscriptions/inbox/{id}",
            axum::routing::delete(delete_inbox).put(mark_read),
        )
        .route("/subscriptions/runs/{id}", get(run).delete(delete_run))
        .route("/subscriptions/runs/{id}/cancel", post(cancel_run))
        .route("/subscriptions/{id}", get(read).put(replace).delete(remove))
        .route("/subscriptions/{id}/enabled", post(enabled))
        .route("/subscriptions/{id}/check", post(check))
        .route("/subscriptions/{id}/runs", get(runs))
        .layer(DefaultBodyLimit::max(128 * 1024))
        .layer(axum::middleware::from_fn(request_deadline))
        .layer(axum::middleware::from_fn(crate::snapshots::no_cache))
}
// This bounds acknowledgement/storage work, independently of the durable
// run's source and execution deadlines. A queued manual command keeps its UUID
// intent after a lost/expired HTTP response and can be retried idempotently.
async fn request_deadline(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match tokio::time::timeout(std::time::Duration::from_secs(15), next.run(request)).await {
        Ok(response) => response,
        Err(_) => (StatusCode::GATEWAY_TIMEOUT, Json(json!({"error":"subscription API acknowledgement deadline exceeded; retry a check with the same run_id"}))).into_response(),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Version {
    version: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Replace {
    version: u64,
    spec: SubscriptionSpec,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Enabled {
    version: u64,
    enabled: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Check {
    run_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    read: bool,
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Page {
    before: Option<i64>,
    limit: u16,
}
impl Default for Page {
    fn default() -> Self {
        Self {
            before: None,
            limit: 50,
        }
    }
}
async fn service(
    shared: &BibShared,
) -> Result<bib_base::subscription_scheduler::SubscriptionScheduler> {
    shared.subscription_service().await.map_err(error)
}
async fn list(State(shared): State<Arc<BibShared>>) -> Result<Json<Value>> {
    Ok(Json(
        json!({"rules":shared.subscriptions.list().await.map_err(error)?}),
    ))
}
async fn status(State(shared): State<Arc<BibShared>>) -> Result<Json<Value>> {
    Ok(Json(json!({"scheduler":service(&shared).await?.status()})))
}
async fn create(
    State(shared): State<Arc<BibShared>>,
    Json(spec): Json<SubscriptionSpec>,
) -> Result<(StatusCode, Json<Value>)> {
    let rule = shared
        .subscriptions
        .create(spec, now_ms())
        .await
        .map_err(error)?;
    service(&shared).await?;
    Ok((StatusCode::CREATED, Json(json!({"rule":rule}))))
}
async fn read(State(shared): State<Arc<BibShared>>, Path(id): Path<String>) -> Result<Json<Value>> {
    Ok(Json(
        json!({"rule":shared.subscriptions.get(&id).await.map_err(error)?}),
    ))
}
async fn replace(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<Replace>,
) -> Result<Json<Value>> {
    Ok(Json(
        json!({"rule":shared.subscriptions.replace(&id,input.version,input.spec,now_ms()).await.map_err(error)?}),
    ))
}
async fn remove(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Query(input): Query<Version>,
) -> Result<StatusCode> {
    shared
        .subscriptions
        .delete(&id, input.version)
        .await
        .map_err(error)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn enabled(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<Enabled>,
) -> Result<Json<Value>> {
    let rule = shared
        .subscriptions
        .set_enabled(&id, input.version, input.enabled, now_ms())
        .await
        .map_err(error)?;
    service(&shared).await?;
    Ok(Json(json!({"rule":rule})))
}
async fn check(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<Check>,
) -> Result<(StatusCode, Json<Value>)> {
    let run = service(&shared)
        .await?
        .check_now(&id, &input.run_id)
        .await
        .map_err(error)?;
    Ok((StatusCode::ACCEPTED, Json(json!({"run":run}))))
}
async fn limits(State(shared): State<Arc<BibShared>>) -> Result<Json<Value>> {
    Ok(Json(
        json!({"limits":shared.subscriptions.limits().await.map_err(error)?}),
    ))
}
async fn configure_limits(
    State(shared): State<Arc<BibShared>>,
    Json(input): Json<SubscriptionLimits>,
) -> Result<Json<Value>> {
    shared
        .subscriptions
        .set_limits(input)
        .await
        .map_err(error)?;
    limits(State(shared)).await
}
async fn inbox(
    State(shared): State<Arc<BibShared>>,
    Query(page): Query<Page>,
) -> Result<Json<Value>> {
    let (items, next) = shared
        .subscriptions
        .inbox(page.before, page.limit)
        .await
        .map_err(error)?;
    Ok(Json(json!({"items":items,"next":next})))
}
async fn delete_inbox(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    shared
        .subscriptions
        .delete_inbox(&id)
        .await
        .map_err(error)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn mark_read(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<Read>,
) -> Result<StatusCode> {
    shared
        .subscriptions
        .set_inbox_read(&id, input.read)
        .await
        .map_err(error)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn runs(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Query(page): Query<Page>,
) -> Result<Json<Value>> {
    shared.subscriptions.get(&id).await.map_err(error)?;
    let (runs, next) = shared
        .subscriptions
        .runs(&id, page.before, page.limit)
        .await
        .map_err(error)?;
    Ok(Json(json!({"runs":runs,"next":next})))
}
async fn run(State(shared): State<Arc<BibShared>>, Path(id): Path<String>) -> Result<Json<Value>> {
    Ok(Json(
        json!({"run":shared.subscriptions.run(&id).await.map_err(error)?}),
    ))
}
async fn cancel_run(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    Ok(Json(
        json!({"run":service(&shared).await?.cancel(&id).await.map_err(error)?}),
    ))
}
async fn delete_run(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    shared.subscriptions.delete_run(&id).await.map_err(error)?;
    Ok(StatusCode::NO_CONTENT)
}
