//! Immutable search captures. Inputs describe an execution, never result rows.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use bib_base::{BibShared, search_snapshot_exports::SnapshotExportFormat, search_snapshots::*};
use bib_types::{SearchMode, SearchRequestV2, discovery::DiscoveryResultOrder};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
type Failure = (StatusCode, Json<Value>);
type Result<T> = std::result::Result<T, Failure>;
fn failure(status: StatusCode, message: impl Into<String>) -> Failure {
    (status, Json(json!({"error":message.into()})))
}
fn error(error: SnapshotError) -> Failure {
    let status = match &error {
        SnapshotError::Invalid(_) => StatusCode::BAD_REQUEST,
        SnapshotError::Conflict(_) => StatusCode::CONFLICT,
        SnapshotError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        SnapshotError::Budget(_) => StatusCode::PAYLOAD_TOO_LARGE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    failure(status, error.to_string())
}
fn search_error(error: bib_base::Error) -> SnapshotError {
    match error {
        bib_base::Error::InvalidSearchQuery(message) => SnapshotError::Invalid(message),
        bib_base::Error::SearchConflict(message) => SnapshotError::Conflict(message),
        error => SnapshotError::Unavailable(error.to_string()),
    }
}
pub(crate) async fn no_cache(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
pub(crate) fn router() -> Router<Arc<BibShared>> {
    Router::new()
        .route("/search/snapshots", get(list).post(create))
        .route(
            "/search/snapshots/limits",
            get(limits).put(configure_limits),
        )
        .route("/search/snapshots/{id}", get(read).delete(remove))
        .route("/search/snapshots/{id}/export", post(export))
        .layer(DefaultBodyLimit::max(128 * 1024))
        .layer(axum::middleware::from_fn(no_cache))
}
fn max_items() -> usize {
    1000
}
fn deadline() -> u64 {
    30_000
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    key: String,
    name: String,
    #[serde(default = "max_items")]
    max_items: usize,
    #[serde(default = "deadline")]
    deadline_ms: u64,
    execution: Execution,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Execution {
    Library {
        search: SearchRequestV2,
    },
    Discovery {
        session_id: String,
        sequence: u64,
        order: DiscoveryResultOrder,
    },
}
async fn controlled<T>(
    shared: &BibShared,
    headers: &HeaderMap,
    deadline_ms: u64,
    future: impl std::future::Future<Output = std::result::Result<T, SnapshotError>>,
) -> Result<T> {
    if !(100..=120_000).contains(&deadline_ms) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "deadline_ms must be 100..120000",
        ));
    }
    let guard = if let Some(id) = headers.get("x-search-request-id") {
        let id = id
            .to_str()
            .map_err(|_| failure(StatusCode::BAD_REQUEST, "invalid request ID"))?;
        Some(shared.search_cancellations.register(id).map_err(|error| {
            let status = match error {
                bib_base::Error::SearchCancelled => {
                    StatusCode::from_u16(499).expect("valid status")
                }
                bib_base::Error::SearchConflict(_) => StatusCode::CONFLICT,
                _ => StatusCode::BAD_REQUEST,
            };
            failure(status, error.to_string())
        })?)
    } else {
        None
    };
    let timed = async {
        tokio::time::timeout(std::time::Duration::from_millis(deadline_ms), future)
            .await
            .map_err(|_| {
                failure(
                    StatusCode::GATEWAY_TIMEOUT,
                    "Snapshot capture deadline exceeded",
                )
            })?
            .map_err(error)
    };
    if let Some(mut guard) = guard {
        tokio::select! {biased;_ =guard.cancelled()=>Err(failure(StatusCode::from_u16(499).expect("valid status"),"Snapshot capture cancelled")),result=timed=>result}
    } else {
        timed.await
    }
}
async fn runtime(
    shared: &BibShared,
    mode: SearchMode,
) -> std::result::Result<SnapshotRuntime, SnapshotError> {
    if mode == SearchMode::Lexical {
        return Ok(SnapshotRuntime::current(None));
    }
    let status = shared.bib.semantic_search_status().map_err(search_error)?;
    if status.phase != bib_base::semantic_service::SemanticPhase::Ready {
        return Err(SnapshotError::Unavailable(
            "semantic index is not ready".into(),
        ));
    }
    let generation = status
        .generation
        .ok_or_else(|| SnapshotError::Unavailable("missing semantic generation".into()))?;
    let signature = status
        .signature
        .ok_or_else(|| SnapshotError::Unavailable("missing model signature".into()))?;
    Ok(SnapshotRuntime::current(Some(SnapshotModel {
        provider: "ollama".into(),
        origin: status.config.origin,
        name: status.config.model,
        digest: status.config.model_digest,
        signature,
        profile: serde_json::to_value(status.config.profile)?,
        inference: serde_json::to_value(status.config.inference)?,
        semantic_generation: generation,
        configuration_generation: status.configuration_generation,
    })))
}
async fn create(
    State(shared): State<Arc<BibShared>>,
    headers: HeaderMap,
    Json(input): Json<Create>,
) -> Result<(StatusCode, Json<Value>)> {
    let work = async {
        match input.execution {
            Execution::Library { search } => {
                let mode = search.mode;
                capture_library(
                    &shared.search_snapshots,
                    LibraryCaptureCommand {
                        key: input.key,
                        name: input.name,
                        request: search,
                        max_items: input.max_items,
                    },
                    || runtime(&shared, mode),
                    |request| async { shared.bib.search_v2(request).await.map_err(search_error) },
                )
                .await
            }
            Execution::Discovery {
                session_id,
                sequence,
                order,
            } => {
                if let Some(saved) = shared
                    .search_snapshots
                    .find_for_saved_discovery(
                        &input.key,
                        &input.name,
                        &session_id,
                        sequence,
                        order,
                        input.max_items,
                    )
                    .await?
                {
                    return Ok(saved);
                }
                let started_at = chrono::Utc::now();
                let (session, results) = shared
                    .discovery_sessions
                    .capture_obtained(&session_id, sequence, order, input.max_items)
                    .map_err(|failure| match failure {
                        bib_base::discovery_sessions::DiscoverySessionError::Invalid(message) => {
                            SnapshotError::Invalid(message)
                        }
                        error => SnapshotError::Conflict(error.to_string()),
                    })?;
                let termination = if results.next_offset.is_some() {
                    CaptureTermination::Budget
                } else {
                    CaptureTermination::End
                };
                shared
                    .search_snapshots
                    .save(
                        &input.key,
                        &input.name,
                        SnapshotCapture {
                            started_at,
                            finished_at: chrono::Utc::now(),
                            runtime: SnapshotRuntime::current(None),
                            data: SnapshotData::Discovery {
                                session,
                                results,
                                order,
                                requested_max_items: input.max_items,
                                termination,
                            },
                        },
                    )
                    .await
            }
        }
    };
    let saved = controlled(&shared, &headers, input.deadline_ms, work).await?;
    Ok((StatusCode::CREATED, Json(json!({"snapshot":saved}))))
}
fn list_limit() -> usize {
    50
}
#[derive(Deserialize)]
struct List {
    #[serde(default = "list_limit")]
    limit: usize,
    cursor: Option<String>,
}
async fn list(
    State(shared): State<Arc<BibShared>>,
    Query(input): Query<List>,
) -> Result<Json<Value>> {
    Ok(Json(json!(
        shared
            .search_snapshots
            .list(input.limit, input.cursor.as_deref())
            .await
            .map_err(error)?
    )))
}
async fn read(State(shared): State<Arc<BibShared>>, Path(id): Path<String>) -> Result<Json<Value>> {
    let saved = shared
        .search_snapshots
        .get(&id)
        .await
        .map_err(error)?
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "Snapshot not found"))?;
    Ok(Json(json!({"snapshot":saved})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    expected_checksum: String,
}
async fn remove(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Query(input): Query<Delete>,
) -> Result<Json<Value>> {
    let deleted = shared
        .search_snapshots
        .delete(&id, &input.expected_checksum)
        .await
        .map_err(error)?;
    Ok(Json(json!({"deleted":deleted})))
}
fn export_limit() -> usize {
    16 * 1024 * 1024
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Export {
    format: SnapshotExportFormat,
    #[serde(default = "export_limit")]
    max_output_bytes: usize,
}
async fn export(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<Export>,
) -> Result<Json<Value>> {
    let saved = shared
        .search_snapshots
        .get(&id)
        .await
        .map_err(error)?
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "Snapshot not found"))?;
    Ok(Json(json!(
        saved
            .export(input.format, input.max_output_bytes)
            .map_err(error)?
    )))
}
async fn limits(State(shared): State<Arc<BibShared>>) -> Result<Json<Value>> {
    Ok(Json(json!(
        shared.search_snapshots.limits().await.map_err(error)?
    )))
}
async fn configure_limits(
    State(shared): State<Arc<BibShared>>,
    Json(input): Json<SnapshotLimits>,
) -> Result<StatusCode> {
    shared
        .search_snapshots
        .configure_limits(input)
        .await
        .map_err(error)?;
    Ok(StatusCode::NO_CONTENT)
}
