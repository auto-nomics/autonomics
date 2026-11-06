//! Route composition for the gateway HTTP API.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::routing::{delete, get, patch, post, put};
use serde_json::json;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use super::auth::{self, AuthContext, bearer_or_session};
use super::docs::ApiDoc;
use super::state::GatewayState;
use super::{
    agents, compute, context, events, gui, hpc_config, hpc_jobs, hydration, lifecycle,
    model_config, plugin_environments, plugins, settings, skills, storage,
};

/// Build the gateway API router. Routes are relative and mounted by the
/// caller under `/api/v1` (see [`router_with_bib`]).
pub fn api_router(state: GatewayState) -> Router {
    Router::new()
        // ── compute targets and HPC jobs ──
        .route("/compute", get(compute::status))
        .route("/compute/readiness", post(compute::readiness))
        .route("/compute/targets", put(compute::target))
        .route("/compute/targets/{id}/check", post(compute::check))
        .route("/compute/policy", put(compute::select))
        .route("/compute/data-sources", post(compute::source))
        .route("/compute/image-evidence", get(compute::image_evidence))
        .route("/hpc/config", get(hpc_config::show).put(hpc_config::update))
        .route("/hpc/jobs", get(hpc_jobs::list))
        .route("/hpc/jobs/watch", post(hpc_jobs::watch))
        .route("/hpc/jobs/{id}/stop-monitor", post(hpc_jobs::stop_monitor))
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
        .route("/agents/{name}/context", get(context::request_context_list))
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
        .route(
            "/agents/{agent_id}/sessions/{session_id}/plan",
            get(storage::get_plan),
        )
        // ── model config ──
        .route("/model-config", get(model_config::get_model_config))
        .route("/model-config/provider", put(model_config::put_provider))
        .route(
            "/model-config/model",
            put(model_config::put_model).delete(model_config::delete_model),
        )
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
        .route(
            "/embedding-config",
            get(model_config::get_embedding_config).put(model_config::put_embedding_config),
        )
        // ── settings ──
        .route(
            "/settings",
            get(settings::get_settings).put(settings::put_setting),
        )
        .route(
            "/plugin-projects",
            get(plugins::list_projects).post(plugins::create_project),
        )
        .route("/plugin-projects/{name}/files", get(plugins::project_files))
        .route("/plugin-projects/{name}/snapshot", get(plugins::project_snapshot))
        .route("/plugin-projects/{name}/container", get(super::devcontainer::status).post(super::devcontainer::open).delete(super::devcontainer::close))
        .route("/plugin-projects/{name}/terminal", get(super::devcontainer::terminal_output).post(super::devcontainer::terminal_open).delete(super::devcontainer::terminal_close))
        .route("/plugin-projects/{name}/terminal/input", post(super::devcontainer::terminal_input))
        .route("/plugin-projects/{name}/terminal/size", put(super::devcontainer::terminal_resize))
        .route("/plugin-projects/{name}/language/{language}", post(super::devcontainer::language_open))
        .route("/plugin-projects/{name}/language/{language}/{client}", get(super::devcontainer::language_output).post(super::devcontainer::language_input).delete(super::devcontainer::language_close))
        .route("/plugin-projects/{name}/debug/{language}", post(super::devcontainer::debug_open))
        .route("/plugin-projects/{name}/debug/{language}/{client}", get(super::devcontainer::debug_output).post(super::devcontainer::debug_input).delete(super::devcontainer::debug_close))
        .route("/plugin-projects/{name}/file", get(plugins::read_project_file).put(plugins::save_project_file).post(plugins::create_project_file).delete(plugins::delete_project_file))
        .route("/plugin-projects/{name}/file/rename", post(plugins::rename_project_file))
        .route("/plugin-projects/{name}/directory", post(plugins::create_project_directory).delete(plugins::delete_project_directory))
        .route("/plugin-projects/{name}/directory/rename", post(plugins::rename_project_directory))
        // ── plugins ──
        .route("/plugins", get(plugins::list_plugins))
        .route(
            "/plugins/environments",
            get(plugin_environments::list_environments),
        )
        .route(
            "/plugins/environments/dockerhub",
            get(plugin_environments::search_docker_hub),
        )
        .route(
            "/plugins/environments/dockerhub/tag",
            get(plugin_environments::inspect_docker_hub_tag),
        )
        .route(
            "/plugins/environments/approve",
            post(plugin_environments::approve_environment),
        )
        .route(
            "/plugins/environments/{id}",
            delete(plugin_environments::delete_environment),
        )
        // ── skills ──
        .route("/skills/library", get(skills::list_skill_library))
        .route(
            "/skills/library/{name}",
            get(skills::get_skill_library_detail),
        )
        .with_state(state)
}

/// Compose the daemon's full public router: the gateway API under
/// `/api/v1`, the bib module under `/api/v1/bib`, the bib web frontend's
/// static routes, and `/api/health`.
///
/// Auth policy: the gateway API is always guarded — by the bearer
/// token (the env token when provided, else the generated token file)
/// or by a browser session cookie. The bib module keeps its documented
/// contract — open on loopback unless `AUTONOMICS_HTTP_API_TOKEN` is
/// set, in which case that env token also guards it. Swagger UI and its
/// OpenAPI document are metadata-only and intentionally
/// browser-loadable; the operations they describe still require a
/// credential.
///
/// The session endpoints live at `/api/session`, *outside* the guarded
/// `/api/v1` nest: `POST /api/session` is how a browser exchanges the
/// bearer token for a session cookie, so it cannot sit behind the
/// guard it exists to satisfy.
#[allow(clippy::too_many_arguments)]
pub fn router_with_bib(
    state: GatewayState,
    gateway_token: String,
    bib_shared: bib_base::BibShared,
    wiki: Arc<wiki::service::WikiHandle>,
    env_token: Option<String>,
) -> Router {
    let auth = Arc::new(AuthContext::new(gateway_token));
    let api = api_router(state).layer(axum::middleware::from_fn_with_state(
        auth.clone(),
        bearer_or_session,
    ));
    let docs =
        SwaggerUi::new("/swagger-ui").url("/api/v1/api-docs/openapi.json", ApiDoc::openapi());
    let bib = api_server::bib::router(bib_shared);
    let wiki_router = api_server::wiki::router(wiki);
    let gated = env_token.filter(|t| !t.trim().is_empty());
    let bib = match gated {
        Some(_) => bib.layer(axum::middleware::from_fn_with_state(
            auth.clone(),
            bearer_or_session,
        )),
        None => bib,
    };
    let wiki_router = match gated {
        Some(_) => wiki_router.layer(axum::middleware::from_fn_with_state(
            auth.clone(),
            bearer_or_session,
        )),
        None => wiki_router,
    };
    let session: Router = Router::new()
        .route(
            "/api/session",
            post(auth::login)
                .get(auth::session_status)
                .delete(auth::logout),
        )
        .with_state(auth);
    Router::new()
        .merge(docs)
        .merge(session)
        .nest("/api/v1/bib", bib)
        .nest("/api/v1/wiki", wiki_router)
        .nest("/api/v1", api)
        .merge(gui::router())
        .route("/api/health", get(health))
        .route("/api/v1", get(lifecycle::api_index))
}

pub(crate) async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "gateway": true }))
}
