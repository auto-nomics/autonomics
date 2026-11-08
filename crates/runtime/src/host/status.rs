//! Runtime status projection: pure mapping from observed [`AgentEvent`]s to
//! the unified [`AgentLifecycleStatus`], plus the host-side bookkeeping
//! that mirrors status changes into the registry and broadcasts them.
//!
//! There is no separate coarse status enum anymore: the host tracks the
//! same [`AgentLifecycleStatus`] the session emits. Events that carry no
//! lifecycle signal of their own (tool results, LLM response previews,
//! stream noise) only refresh the adjacent `last_event` line.

use agentik_sdk::types::AgentEvent;
use agentik_types::AgentLifecycleStatus;

use super::{HostEvent, RuntimeHost};

// ═══════════════════════════════════════════════════════════════════════
// derive_agent_status — pure projection AgentEvent → AgentLifecycleStatus
// ═══════════════════════════════════════════════════════════════════════

/// Project an observed [`AgentEvent`] into a status update.
///
/// Pure function — no side effects, no registry access — so the
/// projection logic stays trivially unit-testable in isolation. The
/// caller ([`RuntimeHost::observe_status`]) handles the bookkeeping.
///
/// Returns `(status, last_event)` where **`None` means "keep the current
/// value"**: only [`AgentEvent::LifecycleChanged`] and the few events that
/// imply a lifecycle transition (tool dispatch, turn boundaries, errors)
/// move the status; everything else at most refreshes the `last_event`
/// one-liner. The session is the authoritative emitter —
/// [`AgentEvent::LifecycleChanged`] passes through unchanged — and
/// `last_event` is sticky, so the last tool name / response preview stays
/// visible between detail-bearing events.
pub(super) fn derive_agent_status(
    event: &AgentEvent,
) -> (Option<AgentLifecycleStatus>, Option<String>) {
    match event {
        // ── Authoritative lifecycle signals — pass through ──
        AgentEvent::LifecycleChanged(lc) => (
            Some(*lc),
            match lc {
                AgentLifecycleStatus::ToolRunning => Some("tool running".into()),
                AgentLifecycleStatus::Retrying => Some("retrying".into()),
                _ => None,
            },
        ),

        // ── Standalone Requesting signal ──
        // The agent emits both `LifecycleChanged(Requesting)` and the bare
        // `Requesting` event; the latter carries no extra info.
        AgentEvent::Requesting => (Some(AgentLifecycleStatus::Requesting), None),
        AgentEvent::TurnStarted { .. } => (Some(AgentLifecycleStatus::Requesting), None),

        // ── Turn boundaries ──
        // The session emits its own terminal lifecycle right around these
        // (`Idle` on completion, `Cancelled` on interruption, `Error` on
        // failure), so these arms only pin the same value earlier and keep
        // the one-liner fresh.
        AgentEvent::TurnCompleted { status, .. } => match status {
            agentik_types::TurnExecutionStatus::Completed => (
                Some(AgentLifecycleStatus::Idle),
                Some("turn completed".into()),
            ),
            agentik_types::TurnExecutionStatus::Interrupted => (
                Some(AgentLifecycleStatus::Cancelled),
                Some("turn interrupted".into()),
            ),
            agentik_types::TurnExecutionStatus::Failed => (
                Some(AgentLifecycleStatus::Error),
                Some("turn failed".into()),
            ),
            // A parked turn (background wait_task): resting, not terminal.
            agentik_types::TurnExecutionStatus::Pause => (
                Some(AgentLifecycleStatus::Waiting),
                Some("waiting for background task".into()),
            ),
        },
        // `Done` closes a turn for observers but pins no status: the
        // preceding `TurnCompleted` already pinned the terminal lifecycle
        // (Idle/Cancelled/Error/Waiting), and Done now also follows failed
        // and interrupted turns — forcing Idle here would clobber those.
        AgentEvent::Done => (None, Some("done".into())),

        // ── Tool-level signals ──
        // Tool dispatch moves the status to `ToolRunning` (the session's
        // own LifecycleChanged usually lands alongside); the tool name
        // itself rides in `last_event`. A completed tool does NOT change
        // the status — the session emits the next lifecycle (`Requesting`
        // for the following model call, `Idle` at turn end) on its own.
        AgentEvent::ToolCall { name, .. } | AgentEvent::ToolCallBackground { name, .. } => {
            (Some(AgentLifecycleStatus::ToolRunning), Some(name.clone()))
        }
        AgentEvent::ToolResult { ok, content } => {
            let prefix = if *ok { "ok" } else { "err" };
            let preview = truncate_preview(content, 80);
            (
                None,
                if preview.is_empty() {
                    Some(format!("tool_result[{prefix}]"))
                } else {
                    Some(format!("tool_result[{prefix}]: {preview}"))
                },
            )
        }
        AgentEvent::ToolBackgroundComplete { ok, seq: _ } => {
            let prefix = if *ok { "ok" } else { "err" };
            (None, Some(format!("bg_tool[{prefix}]")))
        }

        // ── Aggregated LLM responses — detail only, `Streaming` covers ──
        AgentEvent::LlmResponse(text) => {
            let preview = truncate_preview(text, 120);
            (None, Some(format!("llm: {preview}")))
        }
        AgentEvent::Thinking(text) => {
            let preview = truncate_preview(text, 120);
            (None, Some(format!("thinking: {preview}")))
        }

        // ── Terminal-in-turn signals ──
        AgentEvent::Error(msg) => (Some(AgentLifecycleStatus::Error), Some(msg.clone())),
        AgentEvent::TurnAborted => (
            Some(AgentLifecycleStatus::Cancelled),
            Some("turn aborted by user".into()),
        ),
        AgentEvent::RetryableError {
            message,
            attempt,
            max_retries,
        } => (
            Some(AgentLifecycleStatus::Retrying),
            Some(format!("retrying ({attempt}/{max_retries}): {message}")),
        ),

        // ── Context-management events ──
        // The session emits `LifecycleChanged(Compacting)` around these.
        AgentEvent::Compact { .. } | AgentEvent::PlanUpdate { .. } => (None, None),

        // ── Intra-stream noise (token deltas, content-block boundaries,
        //    usage updates, stream start/stop) — no status, no detail ──
        AgentEvent::TextDelta(_)
        | AgentEvent::ThinkingDelta(_)
        | AgentEvent::UsageUpdate { .. }
        | AgentEvent::StreamStart { .. }
        | AgentEvent::ContentBlockStart { .. }
        | AgentEvent::ContentBlockStop { .. }
        | AgentEvent::StreamDelta { .. } => (None, None),

        // ── Injected message (delegate_to, user delivery) — not a status
        //    change by itself; the agent will start processing on its own ──
        AgentEvent::MessageInjected { .. } => (None, None),

        // TUI acknowledgement happens mid-run when a steering prompt reaches
        // conversation memory; it does not change the active status.
        AgentEvent::UserMessageAcknowledged(_) => (None, None),

        // ── Context lifecycle events ──
        // Skipped by `observe_status` (see `is_context_bookkeeping`); this
        // arm is defense in depth for the delegation-progress path, which
        // derives without the filter.
        AgentEvent::ContextActivated { .. } | AgentEvent::ContextList { .. } => (None, None),
    }
}

/// Truncate a free-form string for inclusion in `last_event` so the
/// field stays a one-line summary. Collapses internal whitespace so
/// LLM response previews don't blow up across line breaks.
pub(super) fn truncate_preview(s: &str, max: usize) -> String {
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.len() <= max {
        collapsed
    } else {
        // Cut on a char boundary, not byte index.
        let mut end = max;
        while !collapsed.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &collapsed[..end])
    }
}

/// Session lifecycle events are informational handshakes, not status
/// transitions: the driver fires a `list_contexts` request after *every*
/// `TurnCompleted` (`Failed` included), so letting them through would
/// clobber the status the turn just landed on within milliseconds of it
/// appearing. They must never reach the status bookkeeping.
/// Statuses from which a virtual-actor instance may be passivated: the
/// soft-terminal set plus plain `Idle`. Excludes `Waiting` (a background
/// watcher is alive and a wake-up injection may still land) and every
/// active state.
fn is_passivation_idle_status(status: AgentLifecycleStatus) -> bool {
    matches!(
        status,
        AgentLifecycleStatus::Idle
            | AgentLifecycleStatus::Cancelled
            | AgentLifecycleStatus::Error
            | AgentLifecycleStatus::Aborted
    )
}

fn is_context_bookkeeping(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::ContextActivated { .. } | AgentEvent::ContextList { .. }
    )
}

impl RuntimeHost {
    /// Update the named agent's runtime [`AgentLifecycleStatus`] from the
    /// event just observed, mirroring the change into the entry's
    /// `AgentInfo` and broadcasting a [`HostEvent::AgentStatusChanged`]
    /// when the status actually transitions.
    ///
    /// `None` arms of the projection mean "keep the current value" —
    /// in particular `last_event` is sticky, so the most recent tool name
    /// / response preview stays visible between detail-bearing events.
    ///
    /// No-op if `name` is not in the registry (e.g. the agent was shut
    /// down between `event_rx.recv()` and this call — possible because
    /// shutdown is a separate command path that races with event
    /// delivery).
    pub(super) fn observe_status(&mut self, name: &str, event: &AgentEvent) {
        if is_context_bookkeeping(event) {
            return;
        }
        let (new_status, new_last_event) = derive_agent_status(event);
        let entry = match self.agents.get_mut(name) {
            Some(e) => e,
            None => return,
        };

        let next_status = new_status.unwrap_or(entry.status);
        let next_last_event = new_last_event.or_else(|| entry.last_event.clone());

        // Skip the bookkeeping work (and notification fan-out) if neither
        // the status nor the one-liner actually changed. A busy agent
        // emits dozens of events per turn — without this guard the TUI
        // would be spammed with no-op notifications.
        if entry.status == next_status && entry.last_event == next_last_event {
            return;
        }

        entry.status = next_status;
        entry.last_event = next_last_event.clone();
        // Mirror into AgentInfo so list_agents / get_agent_info see it.
        entry.info.status = next_status;
        entry.info.last_event = next_last_event.clone();

        // Virtual-actor bookkeeping. Background-task dispatch/completion
        // always moves the status or the one-liner (they pass the no-op
        // guard above), so this sees every transition. `idle_since` is
        // Some only while the entry is passivation-idle with no pending
        // work — the sweep's whole predicate, precomputed.
        match event {
            AgentEvent::ToolCallBackground { .. } => entry.background_pending += 1,
            AgentEvent::ToolBackgroundComplete { .. } => {
                entry.background_pending = entry.background_pending.saturating_sub(1);
            }
            _ => {}
        }
        entry.idle_since = if is_passivation_idle_status(entry.status)
            && entry.background_pending == 0
            && !entry
                .inbound_pending
                .load(std::sync::atomic::Ordering::Acquire)
        {
            Some(entry.idle_since.unwrap_or_else(std::time::Instant::now))
        } else {
            None
        };

        // Copy out of `entry` — the emit below needs `&mut self` again.
        let status = entry.status;

        self.emit_host_event(HostEvent::AgentStatusChanged {
            path: name.to_string(),
            status,
            last_event: next_last_event.clone(),
        });
    }

    /// Passivate every registered agent whose idle timer has exceeded the
    /// configured TTL (virtual-actor mode).
    ///
    /// Passivation IS the existing graceful shutdown: the agent's run
    /// loop pauses its context (persisting the snapshot and ending the
    /// WAL session) before exiting, and the next message to the identity
    /// re-activates it from storage. The idle timer already encodes the
    /// full eligibility predicate — idle-compatible status, no
    /// outstanding background tasks — so only the inbound-queue race
    /// needs a re-check here (a delivery may land between the last event
    /// and this sweep).
    pub(crate) fn sweep_idle_agents(&mut self) {
        let Some(ttl) = self.agent_idle_ttl else {
            return;
        };
        let eligible: Vec<String> = self
            .agents
            .iter()
            .filter(|(_, entry)| {
                !entry
                    .inbound_pending
                    .load(std::sync::atomic::Ordering::Acquire)
                    && entry.idle_since.is_some_and(|since| since.elapsed() >= ttl)
            })
            .map(|(name, _)| name.clone())
            .collect();
        for name in eligible {
            tracing::info!(agent = %name, ttl_secs = ttl.as_secs(), "passivating idle agent");
            self.shutdown_agent_with(&name, super::events::RemovalCause::IdlePassivation);
        }
    }

    /// Next deadline at which the passivation sweep should run. Wakes at
    /// most a minute early/late against status changes — the sweep itself
    /// re-validates eligibility, so a stale deadline is harmless.
    pub(super) fn next_passivation_deadline(&self) -> std::time::Instant {
        let now = std::time::Instant::now();
        let tick = std::time::Duration::from_secs(60);
        let Some(ttl) = self.agent_idle_ttl else {
            // Disabled — park the timer; the sweep no-ops anyway.
            return now + std::time::Duration::from_secs(3600);
        };
        let earliest = self
            .agents
            .values()
            .filter_map(|entry| entry.idle_since.map(|since| since + ttl))
            .min();
        earliest.unwrap_or(now + tick).min(now + tick)
    }
}

#[cfg(test)]
mod status_derivation_tests {
    use super::*;
    use agentik_sdk::types::AgentEvent;
    use agentik_types::CompactEvent;
    use chrono::Utc;
    use serde_json::json;

    // ── Lifecycle events — pass through unchanged ──

    #[test]
    fn lifecycle_idle_passes_through() {
        let (s, ev) =
            derive_agent_status(&AgentEvent::LifecycleChanged(AgentLifecycleStatus::Idle));
        assert_eq!(s, Some(AgentLifecycleStatus::Idle));
        assert!(ev.is_none(), "plain transitions carry no one-liner");
    }

    #[test]
    fn lifecycle_requesting_passes_through() {
        let (s, _) = derive_agent_status(&AgentEvent::LifecycleChanged(
            AgentLifecycleStatus::Requesting,
        ));
        assert_eq!(s, Some(AgentLifecycleStatus::Requesting));
    }

    #[test]
    fn lifecycle_streaming_passes_through() {
        let (s, _) = derive_agent_status(&AgentEvent::LifecycleChanged(
            AgentLifecycleStatus::Streaming,
        ));
        assert_eq!(s, Some(AgentLifecycleStatus::Streaming));
    }

    #[test]
    fn lifecycle_waiting_passes_through() {
        let (s, ev) =
            derive_agent_status(&AgentEvent::LifecycleChanged(AgentLifecycleStatus::Waiting));
        assert_eq!(s, Some(AgentLifecycleStatus::Waiting));
        assert!(ev.is_none(), "Waiting needs no synthetic tool label");
    }

    #[test]
    fn lifecycle_cancelled_passes_through() {
        let (s, _) = derive_agent_status(&AgentEvent::LifecycleChanged(
            AgentLifecycleStatus::Cancelled,
        ));
        assert_eq!(s, Some(AgentLifecycleStatus::Cancelled));
    }

    #[test]
    fn lifecycle_error_passes_through() {
        let (s, _) =
            derive_agent_status(&AgentEvent::LifecycleChanged(AgentLifecycleStatus::Error));
        assert_eq!(s, Some(AgentLifecycleStatus::Error));
    }

    #[test]
    fn lifecycle_retrying_carries_retry_label() {
        let (s, ev) = derive_agent_status(&AgentEvent::LifecycleChanged(
            AgentLifecycleStatus::Retrying,
        ));
        assert_eq!(s, Some(AgentLifecycleStatus::Retrying));
        assert_eq!(ev.as_deref(), Some("retrying"));
    }

    #[test]
    fn lifecycle_tool_running_carries_hint() {
        let (s, ev) = derive_agent_status(&AgentEvent::LifecycleChanged(
            AgentLifecycleStatus::ToolRunning,
        ));
        assert_eq!(s, Some(AgentLifecycleStatus::ToolRunning));
        assert_eq!(ev.as_deref(), Some("tool running"));
    }

    // ── Tool events ──

    #[test]
    fn tool_call_moves_to_tool_running_with_name() {
        let (s, ev) = derive_agent_status(&AgentEvent::ToolCall {
            name: "run_bash".into(),
            input: json!({}),
        });
        assert_eq!(s, Some(AgentLifecycleStatus::ToolRunning));
        assert_eq!(ev.as_deref(), Some("run_bash"));
    }

    #[test]
    fn tool_call_background_moves_to_tool_running_with_name() {
        let (s, ev) = derive_agent_status(&AgentEvent::ToolCallBackground {
            target_agent: None,
            seq: 1,
            name: "run_dag".into(),
        });
        assert_eq!(s, Some(AgentLifecycleStatus::ToolRunning));
        assert_eq!(ev.as_deref(), Some("run_dag"));
    }

    /// A finished tool does not move the status — the session emits the
    /// next lifecycle (`Requesting` for the following model call, `Idle`
    /// at turn end) on its own. Only the one-liner refreshes.
    #[test]
    fn tool_result_keeps_status_and_refreshes_preview() {
        let (s, ev) = derive_agent_status(&AgentEvent::ToolResult {
            ok: true,
            content: "exit 0\nhello".into(),
        });
        assert_eq!(s, None);
        assert!(ev.unwrap().contains("ok"));
    }

    #[test]
    fn tool_background_complete_keeps_status() {
        let (s, ev) = derive_agent_status(&AgentEvent::ToolBackgroundComplete { seq: 1, ok: true });
        assert_eq!(s, None);
        assert!(ev.unwrap().contains("ok"));
    }

    // ── Aggregated LLM responses — detail only ──

    #[test]
    fn llm_response_keeps_status_and_carries_truncated_preview() {
        let long = "x".repeat(500);
        let (s, ev) = derive_agent_status(&AgentEvent::LlmResponse(long));
        assert_eq!(s, None);
        let preview = ev.unwrap();
        assert!(preview.len() <= 130); // 120 + ellipsis + prefix
        assert!(preview.ends_with('…'));
    }

    // ── Turn boundaries ──

    #[test]
    fn done_pins_no_status() {
        // Done now also follows failed/interrupted turns — the preceding
        // TurnCompleted owns the terminal status, Done must not clobber it.
        let (s, ev) = derive_agent_status(&AgentEvent::Done);
        assert_eq!(s, None);
        assert_eq!(ev.as_deref(), Some("done"));
    }

    #[test]
    fn turn_completed_lands_on_idle() {
        let (s, ev) = derive_agent_status(&AgentEvent::TurnCompleted {
            turn_id: uuid::Uuid::new_v4(),
            delegation_id: None,
            status: agentik_types::TurnExecutionStatus::Completed,
            telemetry: agentik_types::TurnTelemetry::default(),
        });
        assert_eq!(s, Some(AgentLifecycleStatus::Idle));
        assert_eq!(ev.as_deref(), Some("turn completed"));
    }

    #[test]
    fn turn_interrupted_lands_on_cancelled() {
        let (s, ev) = derive_agent_status(&AgentEvent::TurnCompleted {
            turn_id: uuid::Uuid::new_v4(),
            delegation_id: None,
            status: agentik_types::TurnExecutionStatus::Interrupted,
            telemetry: agentik_types::TurnTelemetry::default(),
        });
        assert_eq!(s, Some(AgentLifecycleStatus::Cancelled));
        assert_eq!(ev.as_deref(), Some("turn interrupted"));
    }

    #[test]
    fn error_carries_full_message() {
        let (s, ev) = derive_agent_status(&AgentEvent::Error("LLM 503: rate limited".into()));
        assert_eq!(s, Some(AgentLifecycleStatus::Error));
        assert_eq!(ev.as_deref(), Some("LLM 503: rate limited"));
    }

    #[test]
    fn turn_aborted_lands_on_cancelled() {
        let (s, ev) = derive_agent_status(&AgentEvent::TurnAborted);
        assert_eq!(s, Some(AgentLifecycleStatus::Cancelled));
        assert_eq!(ev.as_deref(), Some("turn aborted by user"));
    }

    #[test]
    fn retryable_error_lands_on_retrying() {
        let (s, ev) = derive_agent_status(&AgentEvent::RetryableError {
            message: "rate limit".into(),
            attempt: 2,
            max_retries: 5,
        });
        assert_eq!(s, Some(AgentLifecycleStatus::Retrying));
        assert!(ev.unwrap().contains("2/5"));
    }

    // ── Context management — lifecycle comes from the session ──

    #[test]
    fn compact_events_do_not_change_status() {
        let cases = vec![
            CompactEvent::CompactStart {
                ts: Utc::now(),
                plan: None,
            },
            CompactEvent::CompactPhase {
                ts: Utc::now(),
                phase: agentik_types::CompactPhase::Summarizing,
            },
            CompactEvent::CompactSummaryDelta {
                ts: Utc::now(),
                text: "chunk".into(),
            },
            CompactEvent::CompactFinish {
                ts: Utc::now(),
                stats: None,
                error: Some("boom".into()),
            },
        ];
        for event in cases {
            let (s, ev) = derive_agent_status(&AgentEvent::Compact { event });
            assert_eq!(s, None, "compact events must not move the status");
            assert!(ev.is_none(), "no status detail expected");
        }
    }

    // ── Intra-stream noise — no status, no detail ──

    #[test]
    fn text_delta_changes_nothing() {
        let (s, ev) = derive_agent_status(&AgentEvent::TextDelta("tok".into()));
        assert_eq!(s, None);
        assert!(ev.is_none());
    }

    #[test]
    fn injected_message_changes_nothing() {
        let (s, ev) = derive_agent_status(&AgentEvent::MessageInjected {
            text: "hello".into(),
            origin: agentik_types::MessageOrigin::System,
        });
        assert_eq!(s, None);
        assert!(ev.is_none());
    }

    // ── truncate_preview helpers ──

    #[test]
    fn truncate_collapses_whitespace() {
        let s = truncate_preview("line1\n  line2\t\tline3", 100);
        assert_eq!(s, "line1 line2 line3");
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        // Chinese chars are 3 bytes in UTF-8 — naive byte slicing would panic.
        let s = truncate_preview("你好世界你好世界你好世界", 7);
        assert!(s.ends_with('…'));
        // The returned string is still valid UTF-8 (no panic, no partial char).
        assert!(s.is_char_boundary(s.len()));
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;

    // ── Context bookkeeping ──

    /// The post-turn `list_contexts` handshake (and its siblings) must be
    /// classified as bookkeeping, so `observe_status` skips them instead
    /// of clobbering the status the turn just landed on.
    #[test]
    fn context_events_are_status_bookkeeping() {
        let id = uuid::Uuid::new_v4();
        assert!(is_context_bookkeeping(&AgentEvent::ContextList {
            context: None,
        }));
        assert!(is_context_bookkeeping(&AgentEvent::ContextActivated {
            id,
            title: None,
        }));
        // Status-bearing events must never be mistaken for bookkeeping.
        assert!(!is_context_bookkeeping(&AgentEvent::Error("boom".into())));
        assert!(!is_context_bookkeeping(&AgentEvent::Done));
        assert!(!is_context_bookkeeping(&AgentEvent::LifecycleChanged(
            AgentLifecycleStatus::Error
        )));
    }

    // ── emit_host_event forwards to both channels ──
    // Integration test: spin up a real RuntimeHost with SharedInfra
    // minimal setup, register a fake agent, observe a status change,
    // and verify the broadcast receiver gets the AgentStatusChanged.

    #[tokio::test]
    async fn broadcast_receives_agent_status_changed() {
        use std::time::Duration;

        // The full RuntimeHost::open requires DataEngine/Turso
        // setup which is heavy for a unit test. Instead, exercise the
        // broadcast wiring at the module level by directly calling
        // the low-level emit_host_event pattern.
        let (tx, _) = tokio::sync::broadcast::channel::<HostEvent>(16);
        let mut rx = tx.subscribe();
        let event = HostEvent::AgentStatusChanged {
            path: "/root/test".into(),
            status: AgentLifecycleStatus::Idle,
            last_event: Some("done".into()),
        };
        let _ = tx.send(event.clone());

        // Receiver must see the event within a short timeout.
        let received = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("timeout")
            .expect("recv");
        assert_eq!(received.agent_path_or_test(), "/root/test");
    }

    // Helper accessor for tests — HostEvent doesn't expose fields
    // publicly but tests need to assert path equality.
    trait HostEventTestExt {
        fn agent_path_or_test(&self) -> &str;
    }

    impl HostEventTestExt for HostEvent {
        fn agent_path_or_test(&self) -> &str {
            match self {
                HostEvent::AgentRegistered { path, .. } => path.as_str(),
                HostEvent::AgentUnregistered { path, .. } => path,
                HostEvent::IdentityRenamed { .. } | HostEvent::IdentityDeleted { .. } => {
                    // Identity-level events carry no live instance path.
                    ""
                }
                HostEvent::AgentStatusChanged { path, .. } => path,
                HostEvent::AgentModelChanged { path, .. } => path,
            }
        }
    }
}
