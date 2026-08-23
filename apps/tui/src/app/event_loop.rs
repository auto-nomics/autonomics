//! Async terminal/render event loop.

use super::terminal::PANIC_OCCURRED;
use super::*;

impl App {
    /// Async main loop driven by `tokio::select!` with a fixed-rate render tick.
    ///
    /// **Design**: state mutation and rendering are strictly separated.
    ///
    /// - **Event branches** (terminal, agent, app) only mutate state and set
    ///   the `dirty` flag. They never render.
    /// - **Render tick** fires at a fixed interval (~60 fps) but only redraws
    ///   when `dirty` is true *or* the agent is active (animation frames).
    ///   When idle and no events arrive, the loop parks on `select!` and
    ///   consumes zero CPU.
    ///
    /// Ratatui's internal buffer-diff ensures only changed cells are written
    /// to the terminal. The first interval tick completes immediately, so the
    /// initial frame is drawn right away (the constructor sets `dirty = true`).
    pub(super) async fn run_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> std::io::Result<()> {
        use crossterm::event::EventStream;
        use tokio::time::{self, Duration, MissedTickBehavior};
        use tokio_stream::StreamExt;

        let mut event_stream = EventStream::new();

        // Fixed-rate render clock. The first tick completes immediately
        // (guaranteeing the initial frame), then fires every ~16 ms.
        // `Skip` discards ticks that arrive while we were busy handling
        // events, preventing burst-renders after a long event handler.
        let mut render_tick = time::interval(Duration::from_millis(16));
        render_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            if self.should_quit {
                break Ok(());
            }

            // Pre-extract host to avoid multiple `&mut self.host` borrows
            // in the select! branches below.
            //
            // Drain pending host commands BEFORE waiting on the select!.
            // During streaming, agent events flood the biased select! and
            // starve the host-command branch (recv_and_process_command),
            // so CancelAgent / DeliverMessage commands sent from key
            // handlers pile up unprocessed. Draining here ensures every
            // command is handled promptly on each loop iteration.
            if let Some(host) = self.host.as_mut() {
                host.try_process_commands();
            }

            let host_ptr = self.host.as_mut().map(|h| h as *mut RuntimeHost);

            tokio::select! {
                biased;

                // ── Terminal input (keys, mouse, paste, resize) ──
                maybe_event = event_stream.next() => {
                    match maybe_event {
                        Some(Ok(event)) => {
                            let delta = self.handle_event(&event);
                            if delta != 0 {
                                self.apply_scroll_delta(delta);
                            }
                            // Most terminal events mutate state or at least require
                            // a redraw (e.g. resize). Setting dirty unconditionally
                            // is simpler and the worst case is one extra frame.
                            self.dirty = true;
                        }
                        Some(Err(e)) => {
                            // crossterm read error — log so it's not silently
                            // swallowed (the old code dropped `Err` entirely).
                            tracing::warn!(error = %e, "crossterm event read error");
                        }
                        None => {
                            // Stream exhausted (background thread died).
                            // Recreate immediately.
                            tracing::warn!("crossterm EventStream ended; recreating");
                            event_stream = EventStream::new();
                        }
                    }
                }

                // ── Agent streaming events (host-managed) ──
                // Consume events from RuntimeHost's multiplexed channel.
                // This covers ALL agents — both TUI-spawned (registered
                // via host) and tool-spawned. Events are routed to the
                // session tab matching the agent name.
                maybe_agent = async {
                    if let Some(p) = host_ptr {
                        unsafe { (*p).recv_any().await }
                    } else if let Some(handle) =
                        self.handles.get_mut(self.state.active_agent_idx)
                    {
                        // Fallback when no host is available — wrap as tagged.
                        handle.recv_event().await.map(|e| (String::new(), e))
                    } else {
                        std::future::pending::<Option<runtime::TaggedEvent>>().await
                    }
                } => {
                    if let Some((agent_name, event)) = maybe_agent {
                        // Route event to the matching session tab by name.
                        let target_idx = if !agent_name.is_empty() {
                            self.state
                                .sessions
                                .iter()
                                .position(|s| s.name == agent_name)
                                .unwrap_or(self.state.active_agent_idx)
                        } else {
                            self.state.active_agent_idx
                        };

                        let is_session_list = matches!(event, AgentEvent::SessionList { .. });
                        // Done / TurnAborted / Error all signal the end of a
                        // turn — after applying, check for pending queued
                        // messages the user typed while the agent was busy.
                        let may_have_pending = matches!(
                            event,
                            AgentEvent::Done
                                | AgentEvent::TurnAborted
                                | AgentEvent::Error(_)
                        ) || matches!(
                            event,
                            AgentEvent::LifecycleChanged(AgentStatus::Waiting)
                        );
                        if matches!(
                            event,
                            AgentEvent::SessionActivated { .. }
                                | AgentEvent::SessionPaused { .. }
                                | AgentEvent::SessionClosed { .. }
                                | AgentEvent::SessionList { .. }
                        ) {
                            state::apply_session_event(&mut self.state, event, target_idx);
                        } else {
                            // Route to the correct tab's tab_state.
                            let tab_state = self
                                .state
                                .sessions
                                .get_mut(target_idx)
                                .map(|s| {
                                    if s.active_sub_session_idx < s.sub_sessions.len() {
                                        &mut s.sub_sessions[s.active_sub_session_idx].tab_state
                                    } else {
                                        &mut s.pending_tab_state
                                    }
                                });
                            if let Some(ts) = tab_state {
                                state::apply_event(ts, event);
                            }
                        }

                        // After SessionList arrives, spawn background history
                        // loads for sessions that have empty tab_state.messages.
                        if is_session_list {
                            self.spawn_session_history_loads();
                        }

                        // ── Drain pending message queue ──
                        // When the agent finishes a response cycle (Done /
                        // Error → Idle), deliver any messages the user typed
                        // while it was busy. Each message triggers a new turn;
                        // the agent processes them sequentially.
                        if may_have_pending {
                            let agent_name = self
                                .state
                                .sessions
                                .get(target_idx)
                                .map(|s| s.name.clone());
                            let pending: Vec<String> = self
                                .state
                                .sessions
                                .get_mut(target_idx)
                                .map(|s| {
                                    if s.active_sub_session_idx < s.sub_sessions.len() {
                                        s.sub_sessions[s.active_sub_session_idx]
                                            .tab_state
                                            .drain_pending_queue()
                                    } else {
                                        s.pending_tab_state.drain_pending_queue()
                                    }
                                })
                                .unwrap_or_default();
                            if !pending.is_empty() {
                                tracing::info!(
                                    count = pending.len(),
                                    "draining pending message queue after agent idle"
                                );
                                if let Some(name) = agent_name {
                                    if let Some(host) = self.host.as_ref() {
                                        for msg in pending {
                                            host.control().deliver_message(&name, msg);
                                        }
                                    }
                                }
                            }
                        }

                        self.dirty = true;
                    } else {
                        // Active agent channel closed — don't quit, just mark.
                        tracing::warn!("active agent event channel closed");
                    }
                }

                // ── Host commands from agent tools (event-driven) ──
                // Wakes only when an agent tool sends a HostCommand
                // (list_agents, route_task, delegate_to, etc.).
                _ = async {
                    if let Some(p) = host_ptr {
                        unsafe { (*p).recv_and_process_command().await; }
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    self.dirty = true;
                }

                // ── Host lifecycle events (agent registered / unregistered) ──
                // Wakes when a new agent is registered with the host (e.g. by
                // the spawn_agent tool) or shut down. Keeps the TUI's session
                // list in sync with RuntimeHost's agent registry.
                host_event = async {
                    if let Some(p) = host_ptr {
                        unsafe { (*p).recv_event().await }
                    } else {
                        std::future::pending::<Option<runtime::HostEvent>>().await
                    }
                } => {
                    if let Some(ev) = host_event {
                        self.apply_host_event(ev);
                        self.dirty = true;
                    }
                }

                // ── App internal events ──
                maybe_app = self.app_event_rx.recv() => {
                    match maybe_app {
                        Some(event) => self.handle_app_event(event),
                        None => self.should_quit = true,
                    }
                    self.dirty = true;
                }

                // ── Fixed-rate render tick ──
                _ = render_tick.tick() => {
                    // If a background thread panicked, the global panic
                    // hook has already restored the terminal. Stop drawing
                    // immediately so we don't write TUI escape codes onto
                    // the restored terminal and garble the panic output.
                    if PANIC_OCCURRED.load(std::sync::atomic::Ordering::SeqCst) {
                        self.should_quit = true;
                        break Ok(());
                    }

                    let active_status = self.state.active_status();
                    let show_animation = active_status.is_active()
                        || active_status == AgentStatus::Waiting;
                    if !show_animation {
                        self.clear_cancel_pending();
                    } else {
                        let ts = self.state.active_tab_state_mut();
                        ts.frame = ts.frame.wrapping_add(1);
                    }

                    let agent_active = active_status.is_active()
                        || active_status == AgentStatus::Waiting;
                    if self.dirty || agent_active {
                        terminal.draw(|f| self.render(f))?;
                        self.dirty = false;
                    }
                }
            }
        }
    }
}
