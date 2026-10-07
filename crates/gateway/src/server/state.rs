//! Request state shared by all gateway handlers.

use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwapOption;
use runtime::SharedInfra;
use runtime::control::HostControl;
use tokio_util::sync::CancellationToken;

use crate::driver::SessionCache;
use crate::hub::EventHub;
use crate::model_store::ModelStore;
use crate::server::plugin_environments::DockerHubClient;

/// Shared state for every gateway request handler. Cheap to clone —
/// everything inside is an Arc or a channel sender.
#[derive(Clone)]
pub struct GatewayState {
    pub hub: EventHub,
    pub sessions: SessionCache,
    pub control: HostControl,
    pub infra: SharedInfra,
    /// Read-only Docker Hub lookup client used by the user approval flow.
    pub dockerhub: Arc<DockerHubClient>,
    pub models: Arc<ModelStore>,
    /// The daemon-wide default model slot, shared with every agent
    /// spawned without a profile-specific override.
    pub model_slot: Arc<ArcSwapOption<agentik_sdk::model::Model>>,
    /// Startup-loaded profile cache (spawn-by-path + hydration).
    pub profiles: Arc<Vec<agentik_core::AgentProfile>>,
    /// The bound API address, installed by the daemon after
    /// `api_server::server::start` returns — bind resolves only after
    /// the router (and this state) is built (`127.0.0.1:0` in tests
    /// binds an ephemeral port, so the pre-bind string is not the
    /// truth). Same install-later pattern as `model_slot`.
    pub addr: Arc<ArcSwapOption<String>>,
    pub started: Instant,
    pub shutdown: CancellationToken,
    pub version: &'static str,
}
