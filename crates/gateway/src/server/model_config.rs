//! Model provider and active-model handlers.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use crate::hub::EventKind;
use crate::proto::*;

// ── model config ─────────────────────────────────────────────────────

#[utoipa::path(get, path = "/api/v1/model-config", tag = "model-config", responses((status = 200, body = ModelCatalog), (status = 500, description = "Catalog read failed")))]
pub(crate) async fn get_model_config(
    State(state): State<GatewayState>,
) -> GatewayResult<Json<ModelCatalog>> {
    let models = state.models.clone();
    let catalog = tokio::task::block_in_place(|| models.catalog())
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(catalog))
}

#[utoipa::path(put, path = "/api/v1/model-config/provider", tag = "model-config", request_body = SaveProviderRequest, responses((status = 204, description = "Provider saved"), (status = 500, description = "Provider save failed")))]
pub(crate) async fn put_provider(
    State(state): State<GatewayState>,
    Json(req): Json<SaveProviderRequest>,
) -> GatewayResult<StatusCode> {
    let models = state.models.clone();
    tokio::task::block_in_place(|| models.save_provider(&req))
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(put, path = "/api/v1/model-config/active-model", tag = "model-config", request_body = SetActiveModelRequest, responses((status = 204, description = "Active model set"), (status = 400, description = "Model could not be resolved")))]
pub(crate) async fn put_active_model(
    State(state): State<GatewayState>,
    Json(req): Json<SetActiveModelRequest>,
) -> GatewayResult<StatusCode> {
    let models = state.models.clone();
    let hub = state.hub.clone();
    let spec = req.spec.clone();
    let model = tokio::task::block_in_place(|| models.set_active_model(&spec, &hub))
        .map_err(|e| GatewayError::Message(e.to_string()))?;
    state.model_slot.store(Some(Arc::new(model)));
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/model-config/chatgpt/login", tag = "model-config", responses((status = 200, body = ChatgptLoginStart), (status = 400, description = "Login could not start")))]
pub(crate) async fn chatgpt_login(
    State(state): State<GatewayState>,
) -> GatewayResult<Json<ChatgptLoginStart>> {
    let models = state.models.clone();
    let hub = state.hub.clone();
    let url = tokio::task::block_in_place(|| models.start_chatgpt_login(hub.clone()))
        .map_err(|e| GatewayError::Message(e.to_string()))?;
    Ok(Json(ChatgptLoginStart { url }))
}

#[utoipa::path(post, path = "/api/v1/model-config/chatgpt/refresh", tag = "model-config", responses((status = 200, body = ChatgptRefreshResponse), (status = 400, description = "Refresh failed")))]
pub(crate) async fn chatgpt_refresh(
    State(state): State<GatewayState>,
) -> GatewayResult<Json<ChatgptRefreshResponse>> {
    let models = state.models.clone();
    let hub = state.hub.clone();
    let refreshed = models.refresh_chatgpt_now(hub).await?;
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

    let models = state.models.clone();
    let hub = state.hub.clone();
    let provider_name = name.clone();
    tokio::spawn(async move {
        let result = if matches!(provider_type, agentik_sdk::model::ProviderType::Openai) {
            match models.chatgpt_blob() {
                Some(blob) => {
                    agentik_sdk::provider::openai::OpenaiProvider::fetch_remote_catalog(
                        &url,
                        &blob.access_token,
                        &blob.account_id,
                    )
                    .await
                }
                None => Err("openai model catalogue requires a ChatGPT login".to_string()),
            }
        } else {
            agentik_sdk::provider::openrouter::OpenrouterProvider::fetch_remote_catalog(&url).await
        };
        let notice = match result {
            Ok(models_info) => {
                let count = models.persist_remote_models(&provider_name, &models_info);
                match count {
                    Ok(count) => GatewayNotice::CatalogFetched {
                        provider: provider_name.clone(),
                        result: Ok(count),
                    },
                    Err(e) => GatewayNotice::CatalogFetched {
                        provider: provider_name.clone(),
                        result: Err(e.to_string()),
                    },
                }
            }
            Err(e) => GatewayNotice::CatalogFetched {
                provider: provider_name.clone(),
                result: Err(e),
            },
        };
        hub.publish(EventKind::Notice(notice));
    });
    StatusCode::ACCEPTED
}
