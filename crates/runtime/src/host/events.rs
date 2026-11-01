use agentik_types::AgentEvent;

/// Lifecycle events emitted by RuntimeHost. The TUI subscribes to keep
/// its session tabs in sync with backend agent state.
#[derive(Debug, Clone)]
pub enum HostEvent {
    /// An agent was just registered with the host's relay.
    AgentRegistered {
        path: agentik_types::AgentPath,
        info: crate::control::AgentInfo,
    },
    /// An agent was shut down and removed from the registry.
    AgentUnregistered { path: String },
    /// An agent's runtime status changed (e.g. `Requesting → ToolRunning`,
    /// `ToolRunning → Idle`). The status is the agent's unified
    /// [`AgentLifecycleStatus`] — no separate coarse projection. Emitted
    /// from [`RuntimeHost::recv_any`] / [`RuntimeHost::recv_next`] after
    /// the new state is written to the agent's [`AgentEntry`]. The TUI and
    /// other subscribers can use this to keep their status panels in sync
    /// without polling [`crate::control::HostControl::get_status`].
    AgentStatusChanged {
        path: String,
        status: agentik_types::AgentLifecycleStatus,
        last_event: Option<String>,
    },
    /// An agent's model slot was swapped (per-agent override via
    /// `SetAgentModel`). Emitted from the command handler with the spec
    /// taken from the command payload, so it does not race the relay's
    /// async store into the shared slot. Frontends use it to refresh
    /// the model shown for the agent without re-fetching `/state`.
    AgentModelChanged {
        path: String,
        spec: String,
        context_length: u64,
    },
}

/// An `AgentEvent` tagged with the agent name that produced it.
pub type TaggedEvent = (String, AgentEvent);

/// One item produced by [`RuntimeHost::recv_next`].
#[derive(Debug, Clone)]
pub enum NextEvent {
    /// An agent event, fully processed (delegation ledger, topology
    /// routing, status derivation) — identical to what [`TaggedEvent`]
    /// consumers of `recv_any` observe.
    Agent(TaggedEvent),
    /// A host lifecycle event (agent registered / unregistered / status
    /// change).
    Host(HostEvent),
    /// A host command was processed, or a background spawn completed and
    /// registered. No payload — observers react via the `Host` stream.
    Command,
}
