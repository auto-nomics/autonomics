//! Model provider and active-model handlers.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use crate::proto::*;

// ── model config ─────────────────────────────────────────────────────

#[utoipa::path(get, path = "/api/v1/model-config", tag = "model-config", responses((status = 200, body = ModelCatalog), (status = 500, description = "Catalog read failed")))]
pub(crate) async fn get_model_config(
    State(state): State<GatewayState>,
) -> GatewayResult<Json<ModelCatalog>> {
    let catalog = state
        .models
        .catalog()
        .await
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(catalog.into()))
}

#[utoipa::path(put, path = "/api/v1/model-config/provider", tag = "model-config", request_body = SaveProviderRequest, responses((status = 204, description = "Provider saved"), (status = 500, description = "Provider save failed")))]
pub(crate) async fn put_provider(
    State(state): State<GatewayState>,
    Json(req): Json<SaveProviderRequest>,
) -> GatewayResult<StatusCode> {
    let params = runtime::model_bootstrap::SaveProviderParams {
        name: req.name,
        provider_type: req.provider_type,
        base_url: req.base_url,
        api_key: req.api_key,
    };
    state
        .models
        .save_provider(&params)
        .await
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(put, path = "/api/v1/model-config/active-model", tag = "model-config", request_body = SetActiveModelRequest, responses((status = 204, description = "Active model set"), (status = 400, description = "Model could not be resolved")))]
pub(crate) async fn put_active_model(
    State(state): State<GatewayState>,
    Json(req): Json<SetActiveModelRequest>,
) -> GatewayResult<StatusCode> {
    // Resolve + persist + slot swap happen atomically inside the actor;
    // the handler only forwards the spec.
    state
        .models
        .set_active_model(&req.spec)
        .await
        .map_err(GatewayError::Message)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/model-config/chatgpt/login", tag = "model-config", responses((status = 200, body = ChatgptLoginStart), (status = 400, description = "Login could not start")))]
pub(crate) async fn chatgpt_login(
    State(state): State<GatewayState>,
) -> GatewayResult<Json<ChatgptLoginStart>> {
    let url = state
        .models
        .start_chatgpt_login()
        .await
        .map_err(GatewayError::Message)?;
    Ok(Json(ChatgptLoginStart { url }))
}

#[utoipa::path(post, path = "/api/v1/model-config/chatgpt/refresh", tag = "model-config", responses((status = 200, body = ChatgptRefreshResponse), (status = 400, description = "Refresh failed")))]
pub(crate) async fn chatgpt_refresh(
    State(state): State<GatewayState>,
) -> GatewayResult<Json<ChatgptRefreshResponse>> {
    let refreshed = state
        .models
        .refresh_chatgpt_now()
        .await
        .map_err(GatewayError::Message)?;
    Ok(Json(ChatgptRefreshResponse { refreshed }))
}

#[utoipa::path(post, path = "/api/v1/model-config/providers/{name}/catalog", tag = "model-config", request_body = FetchCatalogRequest, responses((status = 202, description = "Catalog fetch started"), (status = 404, description = "Provider does not support remote catalogs")))]
pub(crate) async fn fetch_catalog(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
    Json(req): Json<FetchCatalogRequest>,
) -> StatusCode {
    use agentik_sdk::provider::registry;

    let provider_type = agentik_sdk::model::ProviderType::from(name.as_str());
    if !registry::supports_remote_catalog(&provider_type) {
        return StatusCode::NOT_FOUND;
    }
    let url = if req.base_url.is_empty() {
        registry::default_base_url(&provider_type)
            .unwrap_or("")
            .to_string()
    } else {
        req.base_url
    };

    // The worker spawns the fetch and reports completion as a
    // `CatalogFetched` notice.
    let _ = state.models.fetch_remote_catalog(name, url);
    StatusCode::ACCEPTED
}

#[utoipa::path(get, path = "/api/v1/embedding-config", tag = "model-config", responses((status = 200, body = EmbeddingConfigView)))]
pub(crate) async fn get_embedding_config(
    State(state): State<GatewayState>,
) -> GatewayResult<Json<EmbeddingConfigView>> {
    let config = state
        .models
        .embedding_config()
        .await
        .map_err(GatewayError::Message)?;
    Ok(Json(config.0.into()))
}

#[utoipa::path(put, path = "/api/v1/embedding-config", tag = "model-config", request_body = SaveEmbeddingConfigRequest, responses((status = 200, body = EmbeddingConfigView), (status = 400, description = "Invalid embedding config")))]
pub(crate) async fn put_embedding_config(
    State(state): State<GatewayState>,
    Json(req): Json<SaveEmbeddingConfigRequest>,
) -> GatewayResult<Json<EmbeddingConfigView>> {
    let config = text_embedding::EmbeddingConfig::from(req.config);
    let saved = state
        .models
        .save_embedding_config(config)
        .await
        .map_err(GatewayError::Message)?;
    state
        .infra
        .configure_embedding(&saved.0)
        .await
        .map_err(|e| GatewayError::Message(e.to_string()))?;
    Ok(Json(saved.0.into()))
}
