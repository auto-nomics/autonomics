/// Unified agent lifecycle status — the single source of truth across all
/// layers (core, runtime, TUI).
///
/// The lifecycle is driven by the [`Session`](agentik_core::Session) and
/// propagated to observers via [`AgentEvent::LifecycleChanged`]. Consumers
/// should track status from that event rather than inferring it from
/// streaming events.
///
/// ## State diagram
///
/// ```text
///   ┌──────────────────────────────────────────────────────────┐
///   │                                                          ▼
///  Idle ──→ Requesting ──→ Streaming ──→ (tool calls?) ──→ Requesting
///   ▲          │                │                  │
///   │          │                │                 yes
///   │          ▼                ▼                  │
///   │      Retrying         Compacting             │
///   │          │                │                  │
///   │          ▼                ▼                  │
///   │      Requesting       Requesting             │
///   │                                               │
///   └──────────── Error ◀────────────────────── no ─┘
/// ```
///
/// `Error` is a "soft terminal" state — it persists until the next message
/// re-enters `Requesting`, giving the user time to read the error.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum AgentLifecycleStatus {
    /// Agent is idle — ready to accept and immediately process new messages.
    #[default]
    Idle,

    /// Agent has sent a request to the LLM API and is awaiting the first
    /// streamed token.
    #[serde(alias = "RUNNING")] // backward-compat: old snapshots stored RUNNING
    Requesting,

    /// Agent is receiving streamed output from the LLM.
    Streaming,

    /// A retryable error occurred; the agent is in exponential back-off
    /// before the next attempt.
    Retrying,

    /// A fatal error occurred in the current turn. The status persists until
    /// the next message re-enters `Requesting`.
    Error,

    /// Agent is performing context compaction (summarizing old messages to
    /// free context-window space).
    Compacting,

    /// Legacy terminal state from older versions. New code never sets this;
    /// kept for deserialization of old snapshots. Functionally equivalent
    /// to [`Idle`](Self::Idle).
    #[serde(alias = "ABORTED")]
    Aborted,
}

impl AgentLifecycleStatus {
    /// Returns `true` when the agent is actively processing (not idle, not
    /// in a terminal/error state). Used by the session loop to decide whether
    /// to continue iterating.
    pub fn is_active(self) -> bool {
        !matches!(
            self,
            Self::Idle | Self::Error | Self::Aborted
        )
    }

    /// Returns `true` when the agent is idle and ready for new work.
    pub fn is_idle(self) -> bool {
        matches!(self, Self::Idle | Self::Aborted)
    }
}
