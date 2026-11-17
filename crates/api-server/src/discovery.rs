//! Session event delivery uses a bounded long poll over the existing
//! authenticated JSON transport, equally available to browsers and Tauri.
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use bib_base::{BibShared, discovery_sessions::DiscoverySessionError};
use bib_types::discovery::{DiscoveryImportRequest, DiscoveryRequest, DiscoveryResultOrder};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;
async fn no_cache(
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
fn error(error: DiscoverySessionError) -> (StatusCode, Json<Value>) {
    let status = match &error {
        DiscoverySessionError::Invalid(_) => StatusCode::BAD_REQUEST,
        DiscoverySessionError::NotFound => StatusCode::NOT_FOUND,
        DiscoverySessionError::Conflict(_) => StatusCode::CONFLICT,
        DiscoverySessionError::Capacity(_) => StatusCode::TOO_MANY_REQUESTS,
    };
    (status, Json(json!({"error":error.to_string()})))
}
pub(crate) fn router() -> Router<Arc<BibShared>> {
    Router::new()
        .route("/search/sources", get(sources))
        .route("/search/sessions", post(create))
        .route("/search/sessions/{id}", get(snapshot).delete(remove))
        .route("/search/sessions/{id}/events", get(events))
        .route("/search/sessions/{id}/results", get(results))
        .route(
            "/search/sessions/{id}/candidates/{candidate}",
            get(candidate),
        )
        .route("/search/sessions/{id}/cancel", post(cancel))
        .route("/search/sessions/{id}/sources/{source}/next", post(next))
        .route("/search/sessions/{id}/sources/{source}/retry", post(retry))
        .route("/search/sessions/{id}/import/preview", post(import_preview))
        .route("/search/sessions/{id}/import", post(import))
        .route("/articles/{id}/discovery-provenance", get(provenance))
        .route("/search/acquisitions/{id}", get(acquisition))
        .route("/search/acquisitions/{id}/cancel", post(cancel_acquisition))
        .layer(axum::middleware::from_fn(no_cache))
}
#[utoipa::path(method(get), path = "/api/v1/bib/search/sources", tag = "literature-discovery",
    operation_id = "bib_discovery_sources",
    summary = "List supported literature sources",
    responses((status = 200, description = "Sources", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn sources(State(shared): State<Arc<BibShared>>) -> ApiResult {
    Ok(Json(
        json!({"sources":shared.gateway.discovery_capabilities(),"runtime":shared.discovery_sessions.source_runtime()}),
    ))
}
#[utoipa::path(method(post), path = "/api/v1/bib/search/sessions", tag = "literature-discovery",
    operation_id = "bib_discovery_create",
    summary = "Create a literature search session",
    params(("x-discovery-session-id" = Option<String>, Header, description = "Optional client-provided session ID")),
    request_body = DiscoveryRequest,
    responses((status = 429, description = "Discovery or full-text queue capacity exceeded"), (status = 201, description = "Create", body = crate::bib_openapi::DiscoverySessionResponse, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn create(
    State(shared): State<Arc<BibShared>>,
    headers: HeaderMap,
    Json(request): Json<DiscoveryRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let client_id = headers
        .get("x-discovery-session-id")
        .map(|value| {
            value.to_str().map_err(|_| {
                error(DiscoverySessionError::Invalid(
                    "invalid client session ID header".into(),
                ))
            })
        })
        .transpose()?;
    let session = shared
        .discovery_sessions
        .create_with_id(shared.gateway.clone(), request, client_id)
        .map_err(error)?;
    Ok((StatusCode::CREATED, Json(json!({"session":session}))))
}
#[utoipa::path(method(get), path = "/api/v1/bib/search/sessions/{id}", tag = "literature-discovery",
    operation_id = "bib_discovery_snapshot",
    summary = "Read discovery session status",
    params(("id" = String, Path, description = "id")),
    responses((status = 200, description = "Snapshot", body = crate::bib_openapi::DiscoverySessionResponse, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn snapshot(State(shared): State<Arc<BibShared>>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(
        json!({"session":shared.discovery_sessions.snapshot(&id).map_err(error)?}),
    ))
}
#[utoipa::path(method(delete), path = "/api/v1/bib/search/sessions/{id}", tag = "literature-discovery",
    operation_id = "bib_discovery_remove",
    summary = "Delete a discovery session",
    params(("id" = String, Path, description = "id")),
    responses((status = 204, description = "Completed"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn remove(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    shared.discovery_sessions.delete(&id).map_err(error)?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize, Default, utoipa::IntoParams)]
#[serde(default, deny_unknown_fields)]
struct EventsQuery {
    after: u64,
    wait_ms: u64,
}
#[utoipa::path(method(get), path = "/api/v1/bib/search/sessions/{id}/events", tag = "literature-discovery",
    operation_id = "bib_discovery_events",
    summary = "Poll discovery events",
    description = "JSON long polling after an event sequence, not an SSE stream. wait_ms is bounded to 25000; preserve the returned cursor when polling again.",
    params(("id" = String, Path, description = "id"), EventsQuery),
    responses((status = 200, description = "Events", body = bib_types::discovery::DiscoveryEventPage, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn events(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Query(query): Query<EventsQuery>,
) -> ApiResult {
    if query.wait_ms > 25_000 {
        return Err(error(DiscoverySessionError::Invalid(
            "event wait_ms must not exceed 25000".into(),
        )));
    }
    let page = shared
        .discovery_sessions
        .events_after(&id, query.after, Duration::from_millis(query.wait_ms))
        .await
        .map_err(error)?;
    Ok(Json(json!(page)))
}
#[derive(Deserialize, utoipa::IntoParams)]
#[serde(default, deny_unknown_fields)]
struct ResultsQuery {
    source: Option<String>,
    order: DiscoveryResultOrder,
    offset: usize,
    limit: usize,
    sequence: Option<u64>,
}
impl Default for ResultsQuery {
    fn default() -> Self {
        Self {
            source: None,
            order: Default::default(),
            offset: 0,
            limit: 50,
            sequence: None,
        }
    }
}
#[utoipa::path(method(get), path = "/api/v1/bib/search/sessions/{id}/results", tag = "literature-discovery",
    operation_id = "bib_discovery_results",
    summary = "Read paginated merged search candidates",
    params(("id" = String, Path, description = "id"), ResultsQuery),
    responses((status = 200, description = "Results", body = bib_types::discovery::DiscoveryResultPage, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn results(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Query(query): Query<ResultsQuery>,
) -> ApiResult {
    let page = shared
        .discovery_sessions
        .results(
            &id,
            query.source.as_deref(),
            query.order,
            query.offset,
            query.limit,
            query.sequence,
        )
        .map_err(error)?;
    Ok(Json(json!(page)))
}
#[utoipa::path(method(get), path = "/api/v1/bib/search/sessions/{id}/candidates/{candidate}", tag = "literature-discovery",
    operation_id = "bib_discovery_candidate",
    summary = "Read a candidate and its source variants",
    params(("id" = String, Path, description = "id"), ("candidate" = String, Path, description = "candidate")),
    responses((status = 200, description = "Candidate", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn candidate(
    State(shared): State<Arc<BibShared>>,
    Path((id, candidate)): Path<(String, String)>,
) -> ApiResult {
    Ok(Json(
        json!({"candidate":shared.discovery_sessions.candidate(&id,&candidate).map_err(error)?}),
    ))
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct CancelInput {
    source: Option<String>,
}
#[utoipa::path(method(post), path = "/api/v1/bib/search/sessions/{id}/cancel", tag = "literature-discovery",
    operation_id = "bib_discovery_cancel",
    summary = "Cancel all sources or a selected source",
    params(("id" = String, Path, description = "id")),
    request_body = CancelInput,
    responses((status = 200, description = "Cancel", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn cancel(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<CancelInput>,
) -> ApiResult {
    Ok(Json(
        json!({"session":shared.discovery_sessions.cancel(&id,input.source.as_deref()).map_err(error)?}),
    ))
}
#[utoipa::path(method(post), path = "/api/v1/bib/search/sessions/{id}/sources/{source}/next", tag = "literature-discovery",
    operation_id = "bib_discovery_next",
    summary = "Fetch the next page from one source",
    params(("id" = String, Path, description = "id"), ("source" = String, Path, description = "source")),
    responses((status = 200, description = "Next", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn next(
    State(shared): State<Arc<BibShared>>,
    Path((id, source)): Path<(String, String)>,
) -> ApiResult {
    Ok(Json(
        json!({"session":shared.discovery_sessions.next_page(&id,&source).map_err(error)?}),
    ))
}
#[utoipa::path(method(post), path = "/api/v1/bib/search/sessions/{id}/sources/{source}/retry", tag = "literature-discovery",
    operation_id = "bib_discovery_retry",
    summary = "Retry a failed source",
    params(("id" = String, Path, description = "id"), ("source" = String, Path, description = "source")),
    responses((status = 200, description = "Retry", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn retry(
    State(shared): State<Arc<BibShared>>,
    Path((id, source)): Path<(String, String)>,
) -> ApiResult {
    Ok(Json(
        json!({"session":shared.discovery_sessions.retry(&id,&source).map_err(error)?}),
    ))
}

fn library_error(error: bib_base::Error) -> (StatusCode, Json<Value>) {
    let status = match &error {
        bib_base::Error::InvalidSearchQuery(_) | bib_base::Error::InvalidCollection(_) => {
            StatusCode::BAD_REQUEST
        }
        bib_base::Error::SearchConflict(_) | bib_base::Error::CollectionConflict(_) => {
            StatusCode::CONFLICT
        }
        bib_base::Error::NotFound(_) => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(json!({"error":error.to_string()})))
}
#[utoipa::path(method(post), path = "/api/v1/bib/search/sessions/{id}/import/preview", tag = "literature-discovery",
    operation_id = "bib_discovery_import_preview",
    summary = "Preview candidate matches in the target library",
    params(("id" = String, Path, description = "id")),
    request_body = DiscoveryImportRequest,
    responses((status = 200, description = "Import preview", body = crate::bib_openapi::DiscoveryPreviewResponse, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn import_preview(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<DiscoveryImportRequest>,
) -> ApiResult {
    let records = shared
        .discovery_sessions
        .import_records(&id, &input.selections)
        .map_err(error)?;
    let preview = shared
        .bib
        .preview_discovery_import(records, input.library_id, input.collection_id)
        .await
        .map_err(library_error)?;
    Ok(Json(json!({"preview":preview})))
}
#[utoipa::path(method(post), path = "/api/v1/bib/search/sessions/{id}/import", tag = "literature-discovery",
    operation_id = "bib_discovery_import",
    summary = "Import selected candidates into the library",
    description = "Select 1–100 candidates by ID and expected fingerprint from this session. Each item commits independently: inspect items and failed even when HTTP status is 200. Existing metadata is preserved; fetch_fulltext optionally starts a background acquisition job.",
    params(("id" = String, Path, description = "id")),
    request_body = DiscoveryImportRequest,
    responses((status = 503, description = "Full-text acquisition requires configured file storage"), (status = 429, description = "Discovery or full-text queue capacity exceeded"), (status = 200, description = "Import", body = crate::bib_openapi::DiscoveryImportResponse, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn import(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<DiscoveryImportRequest>,
) -> ApiResult {
    if input.fetch_fulltext && shared.file_storage.is_none() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"full-text acquisition requires configured file storage"})),
        ));
    }
    let records = shared
        .discovery_sessions
        .import_records(&id, &input.selections)
        .map_err(error)?;
    let reservation = if input.fetch_fulltext {
        Some(shared.discovery_acquisitions.reserve().map_err(error)?)
    } else {
        None
    };
    let outcomes = shared
        .bib
        .import_discovery(records, input.library_id, input.collection_id)
        .await
        .map_err(library_error)?;
    let imported = outcomes
        .iter()
        .filter(|item| item.status == bib_types::discovery::DiscoveryImportStatus::Imported)
        .count();
    let failed = outcomes
        .iter()
        .filter(|item| item.status == bib_types::discovery::DiscoveryImportStatus::Failed)
        .count();
    let articles: Vec<_> = outcomes
        .iter()
        .filter_map(|item| item.article.clone())
        .collect();
    let acquisition = match reservation {
        Some(reservation) if !articles.is_empty() => Some(
            reservation
                .start(shared.as_ref().clone(), articles)
                .map_err(error)?,
        ),
        _ => None,
    };
    Ok(Json(
        json!({"items":outcomes,"imported":imported,"failed":failed,"acquisition":acquisition}),
    ))
}
#[utoipa::path(method(get), path = "/api/v1/bib/search/acquisitions/{id}", tag = "literature-discovery",
    operation_id = "bib_discovery_acquisition",
    summary = "Read background full-text acquisition status",
    params(("id" = String, Path, description = "id")),
    responses((status = 200, description = "Acquisition", body = crate::bib_openapi::AcquisitionResponse, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn acquisition(State(shared): State<Arc<BibShared>>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(
        json!({"acquisition":shared.discovery_acquisitions.snapshot(&id).map_err(error)?}),
    ))
}
#[utoipa::path(method(post), path = "/api/v1/bib/search/acquisitions/{id}/cancel", tag = "literature-discovery",
    operation_id = "bib_discovery_cancel_acquisition",
    summary = "Cancel queued full-text acquisition",
    params(("id" = String, Path, description = "id")),
    responses((status = 200, description = "Cancel acquisition", body = crate::bib_openapi::AcquisitionResponse, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn cancel_acquisition(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    Ok(Json(
        json!({"acquisition":shared.discovery_acquisitions.cancel(&id).map_err(error)?}),
    ))
}
#[derive(Deserialize, utoipa::IntoParams)]
#[serde(default, deny_unknown_fields)]
struct ProvenanceQuery {
    offset: usize,
    limit: usize,
}
impl Default for ProvenanceQuery {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: 20,
        }
    }
}
#[utoipa::path(method(get), path = "/api/v1/bib/articles/{id}/discovery-provenance", tag = "literature-discovery",
    operation_id = "bib_discovery_provenance",
    summary = "Read retained discovery provenance",
    params(("id" = String, Path, description = "id"), ProvenanceQuery),
    responses((status = 200, description = "Provenance", body = bib_types::discovery::DiscoveryProvenancePage, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn provenance(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Query(query): Query<ProvenanceQuery>,
) -> ApiResult {
    let page = shared
        .bib
        .discovery_provenance(&id, query.offset, query.limit)
        .await
        .map_err(library_error)?;
    Ok(Json(json!(page)))
}
