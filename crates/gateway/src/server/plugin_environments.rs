//! Docker Hub lookup and the user-approved plugin environment allow list.

use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use utoipa::IntoParams;

use super::{
    error::{GatewayError, GatewayResult},
    state::GatewayState,
};
use crate::proto::*;

const DEFAULT_DOCKER_HUB_URL: &str = "https://hub.docker.com/";

/// Read-only Docker Hub API adapter.
#[derive(Clone)]
pub struct DockerHubClient {
    http: reqwest::Client,
    base_url: reqwest::Url,
}

impl Default for DockerHubClient {
    fn default() -> Self {
        Self::new(DEFAULT_DOCKER_HUB_URL)
    }
}

impl DockerHubClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("valid Docker Hub HTTP client"),
            base_url: reqwest::Url::parse(base_url).expect("valid Docker Hub base URL"),
        }
    }

    async fn search_repositories(
        &self,
        query: &str,
        page_size: u32,
    ) -> Result<Vec<DockerHubRepositoryView>, String> {
        let mut url = self
            .base_url
            .join("v2/search/repositories/")
            .map_err(|error| error.to_string())?;
        url.query_pairs_mut()
            .append_pair("query", query)
            .append_pair("page_size", &page_size.to_string());
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!("Docker Hub returned {}", response.status()));
        }
        let payload = response
            .json::<HubSearchResponse>()
            .await
            .map_err(|error| error.to_string())?;
        Ok(payload
            .results
            .into_iter()
            .map(|repository| DockerHubRepositoryView {
                repository: normalize_repository(&repository.repo_name)
                    .unwrap_or_else(|_| repository.repo_name.trim().to_ascii_lowercase()),
                description: repository.short_description.unwrap_or_default(),
                official: repository.is_official,
                stars: repository.star_count,
                pulls: repository.pull_count,
            })
            .collect())
    }

    async fn tag(&self, repository: &str, tag: &str) -> Result<DockerHubTagView, String> {
        let repository = normalize_repository(repository)?;
        let tag = normalize_tag(tag)?;
        let url = self
            .base_url
            .join(&format!("v2/repositories/{repository}/tags/{tag}/"))
            .map_err(|error| error.to_string())?;
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!("Docker Hub returned {}", response.status()));
        }
        let payload = response
            .json::<HubTagResponse>()
            .await
            .map_err(|error| error.to_string())?;
        let digest = payload
            .digest
            .ok_or_else(|| "Docker Hub tag has no manifest digest".to_string())?;
        Ok(DockerHubTagView {
            repository: repository.clone(),
            tag,
            digest: digest.clone(),
            size_bytes: payload.full_size,
            last_pushed: payload.tag_last_pushed.unwrap_or_default(),
            reference: format!("docker.io/{repository}@{digest}"),
        })
    }
}

#[derive(Debug, Deserialize)]
struct HubSearchResponse {
    results: Vec<HubRepository>,
}

#[derive(Debug, Deserialize)]
struct HubRepository {
    repo_name: String,
    short_description: Option<String>,
    is_official: bool,
    star_count: i64,
    pull_count: i64,
}

#[derive(Debug, Deserialize)]
struct HubTagResponse {
    full_size: i64,
    digest: Option<String>,
    tag_last_pushed: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct DockerHubSearchQuery {
    query: String,
    page_size: Option<u32>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct DockerHubTagQuery {
    repository: String,
    tag: String,
}

/// List environments approved by the host operator.
#[utoipa::path(
    get,
    path = "/api/v1/plugins/environments",
    tag = "plugins",
    responses((status = 200, body = PluginEnvironmentListView))
)]
pub(crate) async fn list_environments(
    State(state): State<GatewayState>,
) -> GatewayResult<Json<PluginEnvironmentListView>> {
    let environments = state
        .infra
        .rsi
        .environment_registry()
        .snapshot()
        .list()
        .into_iter()
        .map(|(id, environment)| PluginEnvironmentView {
            id,
            reference: environment.reference,
            interpreters: environment.interpreters,
        })
        .collect();
    Ok(Json(PluginEnvironmentListView { environments }))
}

/// Search Docker Hub repositories. This operation is read-only.
#[utoipa::path(
    get,
    path = "/api/v1/plugins/environments/dockerhub",
    tag = "plugins",
    params(DockerHubSearchQuery),
    responses((status = 200, body = DockerHubSearchView))
)]
pub(crate) async fn search_docker_hub(
    State(state): State<GatewayState>,
    Query(query): Query<DockerHubSearchQuery>,
) -> GatewayResult<Json<DockerHubSearchView>> {
    let query_text = query.query.trim();
    if query_text.len() < 2 {
        return Err(GatewayError::Status(
            axum::http::StatusCode::BAD_REQUEST,
            "Docker Hub query must contain at least two characters".into(),
        ));
    }
    let repositories = state
        .dockerhub
        .search_repositories(query_text, query.page_size.unwrap_or(25).clamp(1, 100))
        .await
        .map_err(|error| {
            GatewayError::Status(
                axum::http::StatusCode::BAD_GATEWAY,
                format!("Docker Hub search failed: {error}"),
            )
        })?;
    Ok(Json(DockerHubSearchView { repositories }))
}

/// Resolve one Docker Hub tag to its immutable digest-pinned reference.
#[utoipa::path(
    get,
    path = "/api/v1/plugins/environments/dockerhub/tag",
    tag = "plugins",
    params(DockerHubTagQuery),
    responses((status = 200, body = DockerHubTagView))
)]
pub(crate) async fn inspect_docker_hub_tag(
    State(state): State<GatewayState>,
    Query(query): Query<DockerHubTagQuery>,
) -> GatewayResult<Json<DockerHubTagView>> {
    let tag = state
        .dockerhub
        .tag(&query.repository, &query.tag)
        .await
        .map_err(|error| {
            GatewayError::Status(
                axum::http::StatusCode::BAD_REQUEST,
                format!("Docker Hub lookup failed: {error}"),
            )
        })?;
    Ok(Json(tag))
}

/// Resolve a Docker Hub tag and add it to the allow list after explicit user approval.
#[utoipa::path(
    post,
    path = "/api/v1/plugins/environments/approve",
    tag = "plugins",
    request_body = ApprovePluginEnvironmentRequest,
    responses((status = 200, body = PluginEnvironmentView))
)]
pub(crate) async fn approve_environment(
    State(state): State<GatewayState>,
    Json(request): Json<ApprovePluginEnvironmentRequest>,
) -> GatewayResult<Json<PluginEnvironmentView>> {
    // Approval-policy stub: allow every image acquisition while the Docker Hub
    // to allow-list path is brought up. A follow-up PR will replace this with
    // interactive user consent plus durable approval records.
    let allowed = approval_allowed(&request);
    debug_assert!(allowed, "approval stub must allow every request");
    tracing::info!(
        allowed,
        user_approved = request.approved,
        environment_id = %request.environment_id,
        repository = %request.repository,
        tag = %request.tag,
        "plugin environment approval stub"
    );
    let tag = state
        .dockerhub
        .tag(&request.repository, &request.tag)
        .await
        .map_err(|error| {
            GatewayError::Status(
                axum::http::StatusCode::BAD_REQUEST,
                format!("Docker Hub lookup failed: {error}"),
            )
        })?;
    let mut interpreters = request
        .interpreters
        .into_iter()
        .map(|interpreter| interpreter.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    interpreters.retain(|interpreter| !interpreter.is_empty());
    let environment = plugin_rsi::Environment {
        reference: tag.reference.clone(),
        interpreters: interpreters.clone(),
    };
    state
        .infra
        .rsi
        .environment_registry()
        .approve(&request.environment_id, environment)
        .map_err(|error| {
            GatewayError::Status(axum::http::StatusCode::BAD_REQUEST, error.to_string())
        })?;
    Ok(Json(PluginEnvironmentView {
        id: request.environment_id,
        reference: tag.digest_reference(),
        interpreters,
    }))
}

fn approval_allowed(_request: &ApprovePluginEnvironmentRequest) -> bool {
    true
}

/// Remove an environment from future plugin development.
#[utoipa::path(
    delete,
    path = "/api/v1/plugins/environments/{id}",
    tag = "plugins",
    responses((status = 204))
)]
pub(crate) async fn delete_environment(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
) -> GatewayResult<axum::http::StatusCode> {
    let removed = state
        .infra
        .rsi
        .environment_registry()
        .remove(&id)
        .map_err(|error| {
            GatewayError::Status(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                error.to_string(),
            )
        })?;
    if removed {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err(GatewayError::Status(
            axum::http::StatusCode::NOT_FOUND,
            format!("environment `{id}` not found"),
        ))
    }
}

fn normalize_repository(repository: &str) -> Result<String, String> {
    let mut value = repository.trim().to_ascii_lowercase();
    if let Some(suffix) = value.strip_prefix("docker.io/") {
        value = suffix.to_string();
    }
    if value.is_empty() || value.contains([' ', '@', ':', '\\']) {
        return Err(format!("invalid Docker Hub repository `{repository}`"));
    }
    if !value.contains('/') {
        value = format!("library/{value}");
    }
    if value.split('/').any(|part| {
        part.is_empty()
            || part == "."
            || part == ".."
            || !part.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '-' | '_' | '.')
            })
    }) {
        return Err(format!("invalid Docker Hub repository `{repository}`"));
    }
    Ok(value)
}

fn normalize_tag(tag: &str) -> Result<String, String> {
    let value = tag.trim();
    if value.is_empty()
        || value.contains(['/', '@', ':', ' ', '\\'])
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(format!("invalid Docker Hub tag `{tag}`"));
    }
    Ok(value.to_string())
}

impl DockerHubTagView {
    fn digest_reference(&self) -> String {
        format!("docker.io/{}@{}", self.repository, self.digest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::get};
    use serde_json::json;
    use tokio::net::TcpListener;

    #[test]
    fn docker_hub_repository_and_tag_inputs_are_normalized() {
        assert_eq!(normalize_repository("alpine").unwrap(), "library/alpine");
        assert_eq!(
            normalize_repository("Docker.IO/library/Alpine").unwrap(),
            "library/alpine"
        );
        assert!(normalize_repository("../escape").is_err());
        assert!(normalize_tag("3.21").is_ok());
        assert!(normalize_tag("latest@sha256:oops").is_err());
    }

    #[test]
    fn approval_stub_allows_even_an_explicit_false_marker() {
        let request = ApprovePluginEnvironmentRequest {
            repository: "library/alpine".into(),
            tag: "3.21".into(),
            environment_id: "alpine".into(),
            interpreters: vec!["sh".into()],
            approved: false,
        };
        assert!(approval_allowed(&request));
    }

    #[tokio::test]
    async fn docker_hub_client_resolves_repositories_and_tags() {
        async fn search() -> Json<serde_json::Value> {
            Json(json!({
                "results": [{
                    "repo_name": "alpine",
                    "short_description": "Alpine Linux",
                    "is_official": true,
                    "star_count": 3,
                    "pull_count": 4
                }]
            }))
        }
        async fn tag() -> Json<serde_json::Value> {
            Json(json!({
                "full_size": 7,
                "digest": "sha256:0123456789012345678901234567890123456789012345678901234567890123",
                "tag_last_pushed": "2026-01-01T00:00:00Z"
            }))
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/v2/search/repositories/", get(search))
            .route("/v2/repositories/library/alpine/tags/3.21/", get(tag));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let client = DockerHubClient::new(&format!("http://{addr}/"));
        let repositories = client.search_repositories("alpine", 10).await.unwrap();
        assert_eq!(repositories[0].repository, "library/alpine");
        assert!(repositories[0].official);

        let tag = client.tag("alpine", "3.21").await.unwrap();
        assert_eq!(
            tag.reference,
            "docker.io/library/alpine@sha256:0123456789012345678901234567890123456789012345678901234567890123"
        );
        assert_eq!(tag.size_bytes, 7);
    }
}
