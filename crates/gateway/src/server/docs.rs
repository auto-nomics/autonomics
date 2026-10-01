//! OpenAPI metadata for the gateway HTTP API.

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme};
use utoipa::{Modify, OpenApi};

use super::{
    agents, events, hydration, lifecycle, model_config, plugins, sessions, settings, skills,
    storage,
};
use crate::proto::*;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Autonomics Gateway API",
        version = env!("CARGO_PKG_VERSION"),
        description = "Control-plane API for the resident Autonomics daemon.",
    ),
    paths(
        lifecycle::gateway_status,
        lifecycle::gateway_shutdown,
        hydration::get_state,
        hydration::get_profiles,
        events::get_events,
        agents::list_agents,
        agents::spawn_agent,
        agents::deliver_message,
        agents::cancel_agent,
        agents::compact_agent,
        agents::shutdown_agent,
        agents::get_agent_model,
        agents::set_agent_model,
        agents::get_agent_config,
        agents::set_agent_config,
        agents::get_agent_dag,
        sessions::request_session_list,
        sessions::create_session,
        sessions::activate_session,
        sessions::close_session,
        sessions::rename_session,
        storage::list_storage_agents,
        storage::rename_storage_agent,
        storage::delete_storage_agent,
        storage::list_stored_sessions,
        storage::get_history,
        storage::get_plan,
        model_config::get_model_config,
        model_config::put_provider,
        model_config::put_active_model,
        model_config::chatgpt_login,
        model_config::chatgpt_refresh,
        model_config::fetch_catalog,
        settings::get_settings,
        settings::put_setting,
        plugins::list_plugins,
        skills::get_skill_evolution_status,
        skills::trigger_skill_evolution,
        skills::list_skill_proposals,
        skills::approve_skill_proposal,
        skills::reject_skill_proposal,
    ),
    components(schemas(
        GatewayStatus,
        StateSnapshot,
        SpawnAgentRequest,
        SpawnAgentResponse,
        DeliverMessageRequest,
        SetAgentModelRequest,
        AgentModelInfoView,
        AgentRuntimeConfigView,
        SetAgentRuntimeConfigRequest,
        StoredSession,
        CreateSessionRequest,
        RenameSessionRequest,
        RenameAgentRequest,
        SaveProviderRequest,
        SetActiveModelRequest,
        FetchCatalogRequest,
        SkillEvolutionStatus,
        SkillProposalView,
        TriggerEvolutionRequest,
        SkillEvolutionReport,
        SkippedCluster,
        SkillApproveOutcome,
        ChatgptLoginStart,
        ChatgptRefreshResponse,
        SettingsMap,
        PutSettingRequest,
        PluginListView,
        PluginView,
        PluginPanelView,
        PluginSourceView,
    )),
    tags(
        (name = "gateway", description = "Daemon lifecycle and health"),
        (name = "hydration", description = "Frontend state bootstrap and SSE"),
        (name = "agents", description = "Live agent control"),
        (name = "sessions", description = "Agent session control"),
        (name = "storage", description = "Persisted agents, sessions, and history"),
        (name = "model-config", description = "Model providers and active model"),
        (name = "settings", description = "Display settings"),
        (name = "plugins", description = "Manifest plugin family management"),
    ),
    modifiers(&BearerSecurityAddon)
)]
pub(crate) struct ApiDoc;

struct BearerSecurityAddon;

impl Modify for BearerSecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let security = vec![SecurityRequirement::new(
            "bearer_auth",
            Vec::<String>::new(),
        )];
        for path_item in openapi.paths.paths.values_mut() {
            for operation in [
                &mut path_item.get,
                &mut path_item.put,
                &mut path_item.post,
                &mut path_item.delete,
                &mut path_item.options,
                &mut path_item.head,
                &mut path_item.patch,
                &mut path_item.trace,
            ]
            .into_iter()
            .flatten()
            {
                operation.security = Some(security.clone());
            }
        }
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer_auth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("opaque")
                    .description(Some(
                    "Gateway bearer token. The daemon writes it to gateway.token unless AUTONOMICS_HTTP_API_TOKEN is set."
                        .to_string(),
                    ))
                    .build(),
            ),
        );
    }
}
