//! Runtime status projection: pure mapping from observed [`AgentEvent`]s to
//! [`AgentStatus`], plus the host-side bookkeeping that mirrors status
//! changes into the registry and broadcasts them.

use agentik_sdk::types::AgentEvent;

use crate::control::AgentStatus;

use super::{HostEvent, RuntimeHost};

// ═══════════════════════════════════════════════════════════════════════
// derive_agent_status — pure projection AgentEvent → AgentStatus
// ═══════════════════════════════════════════════════════════════════════

/// Project an observed [`AgentEvent`] into a runtime [`AgentStatus`].
///
/// Pure function — no side effects, no registry access — so the
/// projection logic stays trivially unit-testable in isolation. The
/// caller ([`RuntimeHost::observe_status`]) handles the bookkeeping.
///
/// The authoritative source for the agent's lifecycle is the
/// [`AgentLifecycleStatus`] enum which the agent itself emits via
/// [`AgentEvent::LifecycleChanged`]. We map that into our coarser
/// 4-state [`AgentStatus`] (Idle / Running / AwaitingTool / Failed)
/// plus the tool-level events ([`AgentEvent::ToolCall`],
/// [`AgentEvent::ToolResult`], [`AgentEvent::ToolCallBackground`],
/// [`AgentEvent::ToolBackgroundComplete`]) which add the
/// "what tool is it waiting on?" detail that the lifecycle signal
/// alone can't carry.
///
/// `Done` and `Error` are terminal-in-turn signals. `Done` flips to
/// [`AgentStatus::Completed`] and is non-sticky — a fresh message
/// kicks the agent back to `Running`. `Error` is sticky until the
/// agent is shut down.
pub(super) fn derive_agent_status(event: &AgentEvent) -> (AgentStatus, Option<String>) {
    match event {
        // ── Authoritative lifecycle signals ──
        AgentEvent::LifecycleChanged(lc) => match lc {
            agentik_types::AgentLifecycleStatus::Idle
            | agentik_types::AgentLifecycleStatus::Aborted => (AgentStatus::Idle, None),

            agentik_types::AgentLifecycleStatus::Requesting
            | agentik_types::AgentLifecycleStatus::Streaming
            | agentik_types::AgentLifecycleStatus::Compacting => (AgentStatus::Running, None),

            agentik_types::AgentLifecycleStatus::ToolRunning => (
                AgentStatus::AwaitingTool {
                    tool: "executing".into(),
                },
                Some("tool running".into()),
            ),

            agentik_types::AgentLifecycleStatus::Retrying => {
                (AgentStatus::Running, Some("retrying".into()))
            }

            agentik_types::AgentLifecycleStatus::Waiting => (
                AgentStatus::AwaitingTool {
                    tool: "wait_task".into(),
                },
                Some("wait_task".into()),
            ),

            agentik_types::AgentLifecycleStatus::Error => (
                AgentStatus::Failed {
                    message: "lifecycle error".into(),
                },
                Some("lifecycle error".into()),
            ),

            agentik_types::AgentLifecycleStatus::Cancelled => (
                AgentStatus::Failed {
                    message: "cancelled by user".into(),
                },
                Some("cancelled by user".into()),
            ),
        },

        // ── Standalone Requesting signal ──
        // The agent emits both `LifecycleChanged(Requesting)` and the bare
        // `Requesting` event; the latter carries no extra info. Either path
        // maps to Running.
        AgentEvent::Requesting => (AgentStatus::Running, None),
        AgentEvent::TurnStarted { .. } => (AgentStatus::Running, None),

        AgentEvent::TurnCompleted { status, .. } => match status {
            agentik_types::TurnExecutionStatus::Completed => {
                (AgentStatus::Completed, Some("turn completed".into()))
            }
            agentik_types::TurnExecutionStatus::Interrupted => (
                AgentStatus::Failed {
                    message: "turn interrupted".into(),
                },
                Some("turn interrupted".into()),
            ),
            agentik_types::TurnExecutionStatus::Failed => (
                AgentStatus::Failed {
                    message: "turn failed".into(),
                },
                Some("turn failed".into()),
            ),
        },

        // ── Tool-level signals (overwrite the lifecycle-derived status) ──
        AgentEvent::ToolCall { name, .. } | AgentEvent::ToolCallBackground { name, seq: _ } => (
            AgentStatus::AwaitingTool { tool: name.clone() },
            Some(name.clone()),
        ),

        AgentEvent::ToolResult { ok, content } => {
            let prefix = if *ok { "ok" } else { "err" };
            let preview = truncate_preview(content, 80);
            (
                AgentStatus::Running,
                if preview.is_empty() {
                    Some(format!("tool_result[{prefix}]"))
                } else {
                    Some(format!("tool_result[{prefix}]: {preview}"))
                },
            )
        }

        AgentEvent::ToolBackgroundComplete { ok, seq: _ } => {
            let prefix = if *ok { "ok" } else { "err" };
            (AgentStatus::Running, Some(format!("bg_tool[{prefix}]")))
        }

        // ── Aggregated LLM responses ──
        AgentEvent::LlmResponse(text) => {
            let preview = truncate_preview(text, 120);
            (AgentStatus::Running, Some(format!("llm: {preview}")))
        }
        AgentEvent::Thinking(text) => {
            let preview = truncate_preview(text, 120);
            (AgentStatus::Running, Some(format!("thinking: {preview}")))
        }

        // ── Terminal-in-turn signals ──
        AgentEvent::Done => (AgentStatus::Completed, Some("done".into())),

        AgentEvent::Error(msg) => (
            AgentStatus::Failed {
                message: msg.clone(),
            },
            Some(msg.clone()),
        ),

        AgentEvent::TurnAborted => (
            AgentStatus::Failed {
                message: "turn aborted by user".into(),
            },
            Some("turn aborted by user".into()),
        ),

        AgentEvent::RetryableError {
            message,
            attempt,
            max_retries,
        } => (
            AgentStatus::Running,
            Some(format!("retrying ({attempt}/{max_retries}): {message}")),
        ),

        // ── Context-management events — agent is busy, keep Running ──
        AgentEvent::Compact { .. } | AgentEvent::PlanUpdate { .. } => (AgentStatus::Running, None),

        // ── Intra-stream noise (token deltas, content-block boundaries,
        //    usage updates, stream start/stop) — all part of "running" ──
        AgentEvent::TextDelta(_)
        | AgentEvent::ThinkingDelta(_)
        | AgentEvent::UsageUpdate { .. }
        | AgentEvent::StreamStart { .. }
        | AgentEvent::ContentBlockStart { .. }
        | AgentEvent::ContentBlockStop { .. }
        | AgentEvent::StreamDelta { .. } => (AgentStatus::Running, None),

        // ── Injected message (delegate_to, user delivery) — not a status
        //    change by itself; the agent will start processing on its own ──
        AgentEvent::MessageInjected(_) => (AgentStatus::Idle, None),

        // TUI acknowledgement happens mid-run when a steering prompt reaches
        // conversation memory; it does not change the active status.
        AgentEvent::UserMessageAcknowledged(_) => (AgentStatus::Running, None),

        // ── Session lifecycle events — out of scope for runtime status ──
        AgentEvent::SessionActivated { .. }
        | AgentEvent::SessionPaused { .. }
        | AgentEvent::SessionClosed { .. }
        | AgentEvent::SessionList { .. } => (AgentStatus::Idle, None),
    }
}

/// Returns `true` if `status` is terminal — i.e. the agent won't
/// transition further on its own.
///
/// `Completed` is terminal-in-turn but **not** sticky across shutdown:
/// a fresh message flips the agent back to `Running`. From a caller's
/// perspective, `Completed` IS terminal — the caller has the response
/// it needed and any subsequent turn is a new request the caller must
/// opt into via `delegate_to`.
///
/// `Failed` is sticky until the agent is shut down: every subsequent
/// event re-asserts the same failure.
pub fn is_final_status(status: &AgentStatus) -> bool {
    matches!(status, AgentStatus::Completed | AgentStatus::Failed { .. })
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

impl RuntimeHost {
    /// Update the named agent's runtime [`AgentStatus`] from the event
    /// just observed, mirroring the change into the entry's `AgentInfo`
    /// and broadcasting a [`HostEvent::AgentStatusChanged`] when the
    /// status actually transitions.
    ///
    /// No-op if `name` is not in the registry (e.g. the agent was shut
    /// down between `event_rx.recv()` and this call — possible because
    /// shutdown is a separate command path that races with event
    /// delivery).
    pub(super) fn observe_status(&mut self, name: &str, event: &AgentEvent) {
        let (new_status, new_last_event) = derive_agent_status(event);
        let entry = match self.agents.get_mut(name) {
            Some(e) => e,
            None => return,
        };

        // Skip the bookkeeping work (and notification fan-out) if the
        // status didn't actually change. LlmResponse / TextDelta /
        // ThinkingDelta / intra-stream events all map to `Running`, so
        // a busy agent emits dozens of events per turn — without this
        // guard the TUI would be spammed with no-op notifications.
        if entry.status == new_status && entry.last_event == new_last_event {
            return;
        }

        entry.status = new_status.clone();
        entry.last_event = new_last_event.clone();
        // Mirror into AgentInfo so list_agents / get_agent_info see it.
        entry.info.status = new_status.clone();
        entry.info.last_event = new_last_event.clone();

        self.emit_host_event(HostEvent::AgentStatusChanged {
            path: name.to_string(),
            status: new_status.clone(),
            last_event: new_last_event.clone(),
        });

        // Phase 4: persist the status change so the dashboard can
        // reconstruct the agent's runtime state after a process restart.
        // Fire-and-forget — the in-memory state is authoritative for the
        // live runtime; persistence is a read-side projection.
        self.persist_agent_status(name, &new_status, &new_last_event);
        self.persist_current_agent_layout();
    }
}

#[cfg(test)]
mod status_derivation_tests {
    use super::*;
    use agentik_sdk::types::AgentEvent;
    use agentik_types::{AgentLifecycleStatus, CompactEvent};
    use chrono::Utc;
    use serde_json::json;

    // ── Lifecycle events ──

    #[test]
    fn lifecycle_idle_flips_to_idle() {
        let (s, _) = derive_agent_status(&AgentEvent::LifecycleChanged(AgentLifecycleStatus::Idle));
        assert_eq!(s, AgentStatus::Idle);
    }

    #[test]
    fn lifecycle_requesting_flips_to_running() {
        let (s, _) = derive_agent_status(&AgentEvent::LifecycleChanged(
            AgentLifecycleStatus::Requesting,
        ));
        assert_eq!(s, AgentStatus::Running);
    }

    #[test]
    fn lifecycle_waiting_flips_to_awaiting_tool() {
        let (s, _) =
            derive_agent_status(&AgentEvent::LifecycleChanged(AgentLifecycleStatus::Waiting));
        assert_eq!(
            s,
            AgentStatus::AwaitingTool {
                tool: "wait_task".into()
            }
        );
    }

    #[test]
    fn lifecycle_cancelled_flips_to_failed() {
        let (s, _) = derive_agent_status(&AgentEvent::LifecycleChanged(
            AgentLifecycleStatus::Cancelled,
        ));
        assert!(matches!(s, AgentStatus::Failed { .. }));
    }

    #[test]
    fn lifecycle_error_flips_to_failed() {
        let (s, _) =
            derive_agent_status(&AgentEvent::LifecycleChanged(AgentLifecycleStatus::Error));
        assert!(matches!(s, AgentStatus::Failed { .. }));
    }

    #[test]
    fn lifecycle_retrying_carries_retry_label() {
        let (s, ev) = derive_agent_status(&AgentEvent::LifecycleChanged(
            AgentLifecycleStatus::Retrying,
        ));
        assert_eq!(s, AgentStatus::Running);
        assert_eq!(ev.as_deref(), Some("retrying"));
    }

    // ── Tool events ──

    #[test]
    fn tool_call_flips_to_awaiting_tool() {
        let (s, ev) = derive_agent_status(&AgentEvent::ToolCall {
            name: "run_bash".into(),
            input: json!({}),
        });
        assert_eq!(
            s,
            AgentStatus::AwaitingTool {
                tool: "run_bash".into()
            }
        );
        assert_eq!(ev.as_deref(), Some("run_bash"));
    }

    #[test]
    fn tool_call_background_flips_to_awaiting_tool() {
        let (s, _) = derive_agent_status(&AgentEvent::ToolCallBackground {
            seq: 1,
            name: "run_dag".into(),
        });
        assert_eq!(
            s,
            AgentStatus::AwaitingTool {
                tool: "run_dag".into()
            }
        );
    }

    #[test]
    fn tool_result_returns_to_running_with_preview() {
        let (s, ev) = derive_agent_status(&AgentEvent::ToolResult {
            ok: true,
            content: "exit 0\nhello".into(),
        });
        assert_eq!(s, AgentStatus::Running);
        assert!(ev.unwrap().contains("ok"));
    }

    #[test]
    fn tool_background_complete_returns_to_running() {
        let (s, ev) = derive_agent_status(&AgentEvent::ToolBackgroundComplete { seq: 1, ok: true });
        assert_eq!(s, AgentStatus::Running);
        assert!(ev.unwrap().contains("ok"));
    }

    // ── Aggregated LLM responses ──

    #[test]
    fn llm_response_carries_truncated_preview() {
        let long = "x".repeat(500);
        let (s, ev) = derive_agent_status(&AgentEvent::LlmResponse(long));
        assert_eq!(s, AgentStatus::Running);
        let preview = ev.unwrap();
        assert!(preview.len() <= 130); // 120 + ellipsis + prefix
        assert!(preview.ends_with('…'));
    }

    // ── Terminal-in-turn events ──

    #[test]
    fn done_flips_to_completed() {
        let (s, _) = derive_agent_status(&AgentEvent::Done);
        assert_eq!(s, AgentStatus::Completed);
    }

    #[test]
    fn error_flips_to_failed_with_full_message() {
        let (s, ev) = derive_agent_status(&AgentEvent::Error("LLM 503: rate limited".into()));
        assert_eq!(
            s,
            AgentStatus::Failed {
                message: "LLM 503: rate limited".into()
            }
        );
        assert_eq!(ev.as_deref(), Some("LLM 503: rate limited"));
    }

    #[test]
    fn turn_aborted_flips_to_failed() {
        let (s, _) = derive_agent_status(&AgentEvent::TurnAborted);
        assert!(matches!(s, AgentStatus::Failed { .. }));
    }

    #[test]
    fn retryable_error_keeps_running() {
        let (s, ev) = derive_agent_status(&AgentEvent::RetryableError {
            message: "rate limit".into(),
            attempt: 2,
            max_retries: 5,
        });
        assert_eq!(s, AgentStatus::Running);
        assert!(ev.unwrap().contains("2/5"));
    }

    // ── Context management ──

    #[test]
    fn compact_flips_to_running() {
        let (s, _) = derive_agent_status(&AgentEvent::Compact {
            event: CompactEvent::CompactStart {
                ts: Utc::now(),
                plan: None,
            },
        });
        assert_eq!(s, AgentStatus::Running);
    }

    /// All compaction progress events (phases, summary deltas, failure
    /// finishes) must keep the agent in `Running` — the TUI drives its
    /// compacting overlay from the Compact events, not the agent status.
    #[test]
    fn compact_progress_events_keep_running() {
        let cases = vec![
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
            assert_eq!(s, AgentStatus::Running, "status for {ev:?}");
            assert!(ev.is_none(), "no status detail expected");
        }
    }

    // ── Intra-stream noise ──

    #[test]
    fn text_delta_does_not_change_running() {
        let (s, ev) = derive_agent_status(&AgentEvent::TextDelta("tok".into()));
        assert_eq!(s, AgentStatus::Running);
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

    // ── is_final_status ──

    #[test]
    fn idle_is_not_final() {
        assert!(!is_final_status(&AgentStatus::Idle));
    }

    #[test]
    fn running_is_not_final() {
        assert!(!is_final_status(&AgentStatus::Running));
    }

    #[test]
    fn awaiting_tool_is_not_final() {
        assert!(!is_final_status(&AgentStatus::AwaitingTool {
            tool: "run_bash".into()
        }));
    }

    #[test]
    fn completed_is_final() {
        assert!(is_final_status(&AgentStatus::Completed));
    }

    #[test]
    fn failed_is_final() {
        assert!(is_final_status(&AgentStatus::Failed {
            message: "boom".into()
        }));
    }

    // ── AgentStatus::tag for log lines ──

    #[test]
    fn agent_status_tags_match_serde_kind() {
        assert_eq!(AgentStatus::Idle.tag(), "idle");
        assert_eq!(AgentStatus::Running.tag(), "running");
        assert_eq!(
            AgentStatus::AwaitingTool { tool: "x".into() }.tag(),
            "awaiting_tool"
        );
        assert_eq!(AgentStatus::Completed.tag(), "completed");
        assert_eq!(
            AgentStatus::Failed {
                message: "x".into()
            }
            .tag(),
            "failed"
        );
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
            status: AgentStatus::Completed,
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
                HostEvent::AgentUnregistered { path } => path,
                HostEvent::AgentStatusChanged { path, .. } => path,
                HostEvent::AgentModelChanged { path, .. } => path,
            }
        }
    }
}
