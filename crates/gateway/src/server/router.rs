//! Route composition for the gateway HTTP API.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::routing::{get, patch, post, put};
use serde_json::json;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use super::auth::bearer_auth;
use super::docs::ApiDoc;
use super::state::GatewayState;
use super::{
    agents, events, hydration, lifecycle, model_config, plugins, sessions, settings, skills,
    storage,
};

/// Build the gateway API router. Routes are relative and mounted by the
/// caller under `/api/v1` (see [`router_with_bib`]).
pub fn api_router(state: GatewayState) -> Router {
    Router::new()
        // ── daemon lifecycle ──
        .route("/gateway/status", get(lifecycle::gateway_status))
        .route("/gateway/shutdown", post(lifecycle::gateway_shutdown))
        // ── hydration ──
        .route("/state", get(hydration::get_state))
        .route("/profiles", get(hydration::get_profiles))
        .route("/events", get(events::get_events))
        // ── agents (live) ──
        .route(
            "/agents",
            get(agents::list_agents).post(agents::spawn_agent),
        )
        .route("/agents/{name}/messages", post(agents::deliver_message))
        .route("/agents/{name}/cancel", post(agents::cancel_agent))
        .route("/agents/{name}/compact", post(agents::compact_agent))
        .route("/agents/{name}/shutdown", post(agents::shutdown_agent))
        .route(
            "/agents/{name}/model",
            get(agents::get_agent_model).put(agents::set_agent_model),
        )
        .route(
            "/agents/{name}/config",
            get(agents::get_agent_config).put(agents::set_agent_config),
        )
        .route("/agents/{name}/dag", get(agents::get_agent_dag))
        .route(
            "/agents/{name}/sessions",
            get(sessions::request_session_list).post(sessions::create_session),
        )
        .route(
            "/agents/{name}/sessions/{id}/activate",
            post(sessions::activate_session),
        )
        .route(
            "/agents/{name}/sessions/{id}/close",
            post(sessions::close_session),
        )
        .route(
            "/agents/{name}/sessions/{id}/title",
            patch(sessions::rename_session),
        )
        // ── storage (persisted agents / history / plans) ──
        .route("/storage/agents", get(storage::list_storage_agents))
        .route(
            "/storage/agents/{id}",
            patch(storage::rename_storage_agent).delete(storage::delete_storage_agent),
        )
        .route(
            "/storage/agents/{id}/sessions",
            get(storage::list_stored_sessions),
        )
        .route(
            "/agents/{agent_id}/sessions/{session_id}/history",
            get(storage::get_history),
        )
        .route("/agents/{agent_id}/plan", get(storage::get_plan))
        // ── model config ──
        .route("/model-config", get(model_config::get_model_config))
        .route("/model-config/provider", put(model_config::put_provider))
        .route(
            "/model-config/active-model",
            put(model_config::put_active_model),
        )
        .route(
            "/model-config/chatgpt/login",
            post(model_config::chatgpt_login),
        )
        .route(
            "/model-config/chatgpt/refresh",
            post(model_config::chatgpt_refresh),
        )
        .route(
            "/model-config/providers/{name}/catalog",
            post(model_config::fetch_catalog),
        )
        // ── settings ──
        .route(
            "/settings",
            get(settings::get_settings).put(settings::put_setting),
        )
        // ── plugins ──
        .route("/plugins", get(plugins::list_plugins))
        // ── skills ──
        .route("/skills/library", get(skills::list_skill_library))
        .route(
            "/skills/library/{name}",
            get(skills::get_skill_library_detail),
        )
        .route("/skills/evolution", get(skills::get_skill_evolution_status))
        .route(
            "/skills/evolution/observations",
            get(skills::list_skill_observations),
        )
        .route(
            "/skills/evolution/trigger",
            post(skills::trigger_skill_evolution),
        )
        .route(
            "/skills/evolution/proposals",
            get(skills::list_skill_proposals),
        )
        .route(
            "/skills/evolution/proposals/{name}/approve",
            post(skills::approve_skill_proposal),
        )
        .route(
            "/skills/evolution/proposals/{name}/reject",
            post(skills::reject_skill_proposal),
        )
        .with_state(state)
}

/// Compose the daemon's full public router: the gateway API under
/// `/api/v1`, the bib module under `/api/v1/bib`, the bib web frontend's
/// static routes, and `/api/health`.
///
/// Auth policy: the gateway API is always bearer-guarded (by the env
/// token when provided, else the generated token file). The bib module
/// keeps its documented contract — open on loopback unless
/// `AUTONOMICS_HTTP_API_TOKEN` is set, in which case that env token also
/// guards it. Swagger UI and its OpenAPI document are metadata-only and
/// intentionally browser-loadable; the operations they describe still
/// require the gateway bearer token.
pub fn router_with_bib(
    state: GatewayState,
    gateway_token: String,
    bib_shared: bib_base::BibShared,
    env_token: Option<String>,
) -> Router {
    let api = api_router(state).layer(axum::middleware::from_fn_with_state(
        Arc::new(gateway_token),
        bearer_auth,
    ));
    let docs =
        SwaggerUi::new("/swagger-ui").url("/api/v1/api-docs/openapi.json", ApiDoc::openapi());
    let bib = api_server::bib::router(bib_shared);
    let bib = match env_token.filter(|t| !t.trim().is_empty()) {
        Some(token) => bib.layer(axum::middleware::from_fn_with_state(
            Arc::new(token),
            bearer_auth,
        )),
        None => bib,
    };
    Router::new()
        .merge(docs)
        .nest("/api/v1/bib", bib)
        .nest("/api/v1", api)
        .merge(api_server::frontend_router())
        .route("/api/health", get(health))
        .route("/api/v1", get(lifecycle::api_index))
}

pub(crate) async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "gateway": true }))
}
