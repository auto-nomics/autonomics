use agentik_types::AgentEvent;

/// Why an instance just registered. Instance lifecycle is an
/// implementation detail of virtual-actor mode — the cause lets
/// consumers react without guessing from context (name matching,
/// registry state at arrival time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationCause {
    /// An operator/frontend explicitly spawned or resumed the identity
    /// (gateway spawn request). May legitimately steal UI focus.
    UserInitiated,
    /// An agent spawned a child via its tools. Registry-only: no view
    /// opens, no focus moves.
    ToolChild,
    /// The host materialized a dormant identity to deliver a buffered
    /// message (virtual-actor wake-up). Invisible unless a view is
    /// already open for the identity.
    WakeUp,
}

/// Why an instance left the registry. The identity survives storage in
/// every variant except the eviction that follows `IdentityDeleted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalCause {
    /// Idle-TTL passivation (virtual-actor mode). The identity stays;
    /// the next message re-activates it.
    IdlePassivation,
    /// An explicit shutdown request (operator, agent child shutdown).
    Explicit,
    /// Evicted because the identity was deleted or renamed in storage.
    Evicted,
    /// The daemon is shutting down; every instance drains.
    DaemonShutdown,
}

/// Lifecycle events emitted by RuntimeHost. The TUI subscribes to keep
/// its session tabs in sync with backend agent state.
#[derive(Debug, Clone)]
pub enum HostEvent {
    /// An agent was just registered with the host's relay. The cause
    /// says who asked for the instance — frontends use it to decide
    /// focus and view behaviour instead of heuristic name matching.
    AgentRegistered {
        path: agentik_types::AgentPath,
        info: crate::control::AgentInfo,
        cause: ActivationCause,
    },
    /// An agent was shut down and removed from the registry. Under
    /// virtual-actor mode the identity survives in storage for every
    /// cause; `cause` distinguishes passivation from eviction.
    AgentUnregistered { path: String, cause: RemovalCause },
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
    /// An identity was renamed in storage (old path → new path). Emitted
    /// by the host pump on the storage handler's behalf so every
    /// connected frontend — not just the one that issued the rename —
    /// can rebind its views. Follows an `AgentUnregistered` with cause
    /// `Evicted` for the old path when a live instance existed.
    IdentityRenamed {
        agent_id: uuid::Uuid,
        old_path: String,
        new_path: String,
    },
    /// An identity was deleted from storage. Follows an
    /// `AgentUnregistered` with cause `Evicted` when a live instance
    /// existed. Views over the identity have nothing left to show.
    IdentityDeleted { agent_id: uuid::Uuid, path: String },
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
    /// A host command was processed, a background spawn completed and
    /// registered, or the idle-passivation sweep ran. No payload —
    /// observers react via the `Host` stream.
    Command,
}

/// Outcome of a background activation task, reported back to the host
/// pump through the registration channel.
///
/// Virtual-actor mode: deliveries to a dormant identity spawn a
/// background task that materializes the instance from its stored
/// record; the pump registers successes and fails everything buffered
/// for the path on errors.
pub(crate) enum SpawnNotice {
    /// The instance spawned successfully — register it and flush the
    /// activation buffer for its path.
    Activated {
        handle: super::agent_handle::AgentHandle,
        info: crate::control::AgentInfo,
        cause: super::events::ActivationCause,
    },
    /// The identity could not be activated (no record, model unavailable,
    /// spawn error).
    Failed { path: String, reason: String },
}

/// One delivery held in the host's activation buffer while its target
/// agent activates. Ordering is the activation race-safety mechanism:
/// buffered deliveries land in arrival order, after the instance is
/// registered, on the single pump.
pub(super) struct PendingDelivery {
    pub(super) text: String,
    pub(super) delegation_id: Option<uuid::Uuid>,
    pub(super) origin: agentik_types::MessageOrigin,
    pub(super) reply_tx: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
}
