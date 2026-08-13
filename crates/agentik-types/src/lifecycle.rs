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
///   ┌────────────────────────────────────────────────────────────────┐
///   │                                                                ▼
///  Idle ──→ Requesting ──→ Streaming ──→ (tool calls?) ──→ ToolRunning ──→ Requesting
///   ▲          │                │                  │                        │
///   │          │                │                 yes                       │
///   │          ▼                ▼                  │                        │
///   │      Retrying         Compacting             │                        │
///   │          │                │                  │                        │
///   │          ▼                ▼                  │                        │
///   │      Requesting       Requesting             │                        │
///   │                                                 │                     │
///   │         Cancelled ◀────── user Ctrl+C ─────────┘                     │
///   │            │                    ▲                                    │
///   │            │ (next message)     │ wait_task on running task           │
///   │            │                    │ exits loop + spawns watcher         │
///   │            │                    │ watcher injects msg to resume       │
///   │            │                    │ Ctrl+C also cancels watcher         │
///   └────────────┴──→ Requesting     ──→ Waiting ──→ Requesting            │
///
///   Error ◀─── fatal failure (persists until next message)
/// ```
///
/// `Cancelled` is a soft-terminal state: the user intentionally interrupted
/// the current turn. It is semantically distinct from `Error` (system
/// failure) and behaves like `Idle` for interaction purposes — the user can
/// immediately send a new message, which transitions back to `Requesting`.
///
/// `Error` is also a soft-terminal state — it persists until the next
/// message re-enters `Requesting`, giving the user time to read the error.
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

    /// Agent has dispatched tool calls and is waiting for their results.
    /// This state covers the entire duration of tool execution — from the
    /// moment tools are invoked until results are returned. It gives the TUI
    /// a distinct "tool running" indicator so long-running tools don't
    /// appear as "requesting" or "streaming".
    ///
    /// `Waiting` (for `wait_task`) overrides this to convey a more specific
    /// "blocked on background task" meaning.
    ToolRunning,

    /// A retryable error occurred; the agent is in exponential back-off
    /// before the next attempt.
    Retrying,

    /// A fatal error occurred in the current turn. The status persists until
    /// the next message re-enters `Requesting`.
    Error,

    /// Agent is performing context compaction (summarizing old messages to
    /// free context-window space).
    Compacting,

    /// Agent has called `wait_task` on a still-running background task and
    /// the session loop has exited (like `Idle`). A background watcher is
    /// monitoring the task; when it completes or times out, the watcher
    /// injects a message that re-enters `run_session`. The agent is fully
    /// responsive — Ctrl+C cancels the wait and transitions to `Cancelled`.
    /// New messages are enqueued and will be processed when the watcher
    /// fires or the user sends a new message.
    Waiting,

    /// The user intentionally interrupted the current turn (Ctrl+C).
    /// Semantically distinct from `Error`: the agent didn't fail, the user
    /// chose to stop. Behaves like `Idle` for interaction — the user can
    /// immediately send a new message. A conversation marker is injected so
    /// the LLM knows the previous turn was interrupted.
    Cancelled,

    /// Legacy terminal state from older versions. New code never sets this;
    /// kept for deserialization of old snapshots. Functionally equivalent
    /// to [`Idle`](Self::Idle).
    #[serde(alias = "ABORTED")]
    Aborted,
}

impl AgentLifecycleStatus {
    /// Returns `true` when the agent is actively processing (not idle, not
    /// in a terminal/error/cancelled state, not waiting). Used by the session
    /// loop to decide whether to continue iterating.
    pub fn is_active(self) -> bool {
        !matches!(
            self,
            Self::Idle | Self::Error | Self::Cancelled | Self::Waiting | Self::Aborted
        )
    }

    /// Returns `true` when the agent is idle and ready for new work.
    pub fn is_idle(self) -> bool {
        matches!(self, Self::Idle | Self::Aborted)
    }
}
