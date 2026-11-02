/// Unified agent lifecycle status — the single source of truth across all
/// layers (core, runtime host, TUI, GUI).
///
/// The lifecycle is driven by the [`Session`](agentik_core::Session) and
/// propagated to observers via [`AgentEvent::LifecycleChanged`]. The
/// runtime host tracks this enum directly (no separate coarse projection):
/// `AgentInfo.status` and `HostEvent::AgentStatusChanged` carry it, with
/// the tool name / error detail riding in the adjacent `last_event` line.
/// Consumers should track status from the lifecycle event rather than
/// inferring it from streaming events.
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

    /// Short lowercase tag suitable for log lines, delegation progress
    /// records, and frontend status labels. Spelled to match the labels
    /// the former host-side coarse projection produced (e.g. `toolrunning`,
    /// not `tool_running`) so existing frontend label maps keep working.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Requesting => "requesting",
            Self::Streaming => "streaming",
            Self::ToolRunning => "toolrunning",
            Self::Retrying => "retrying",
            Self::Error => "error",
            Self::Compacting => "compacting",
            Self::Waiting => "waiting",
            Self::Cancelled => "cancelled",
            Self::Aborted => "aborted",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tags are part of the wire contract: the GUI maps them to icons
    /// and the delegation progress records persist them. Renaming a tag
    /// here must be a conscious frontend-affecting change.
    #[test]
    fn tags_cover_every_variant() {
        let cases = [
            (AgentLifecycleStatus::Idle, "idle"),
            (AgentLifecycleStatus::Requesting, "requesting"),
            (AgentLifecycleStatus::Streaming, "streaming"),
            (AgentLifecycleStatus::ToolRunning, "toolrunning"),
            (AgentLifecycleStatus::Retrying, "retrying"),
            (AgentLifecycleStatus::Error, "error"),
            (AgentLifecycleStatus::Compacting, "compacting"),
            (AgentLifecycleStatus::Waiting, "waiting"),
            (AgentLifecycleStatus::Cancelled, "cancelled"),
            (AgentLifecycleStatus::Aborted, "aborted"),
        ];
        for (status, tag) in cases {
            assert_eq!(status.tag(), tag);
        }
    }
}
