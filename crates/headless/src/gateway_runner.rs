//! Gateway-backed headless runner — the execution path of `autonomics run`.
//!
//! The contract: one prompt, the stable [`RunEvent`] JSONL schema,
//! exit-code semantics 0/1/2/3. The prompt is submitted to the resident
//! gateway daemon (auto-spawned when absent); the run loop here only
//! orchestrates spawn → prompt → event translation → cleanup.
//!
//! Identity: the caller names the agent, which maps to `/root/<name>`;
//! the daemon's storage restores the same agent_id across runs and
//! `--session` resume keeps working. Only when a concurrent run already
//! holds that path does the spawn fall back to a unique suffix (resume
//! is meaningless in that contention anyway).

use std::time::{Duration, Instant};

use agentik_core::AgentRuntimeOverrides;
use agentik_types::AgentEvent;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use gateway::client::ParsedFrame;
use gateway::proto::{HostEventView, StoredSession};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::processor::OutputProcessor;
use crate::{RunError, RunSummary, Terminal, TranslationState, event::*, resolve_kind};

/// How long to wait for the daemon's `AgentRegistered` frame (carrying
/// the restored agent id) before proceeding without it.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_HEADLESS_AGENT_NAME: &str = "headless";

/// How long to keep reading after the terminal turn event, so the
/// compatibility `Done` / `Error` events land in the output too (they
/// may still be in flight on the SSE stream).
const TERMINAL_DRAIN: Duration = Duration::from_millis(300);

/// Everything `run_via_gateway` needs to execute one prompt.
#[derive(Clone, Default)]
pub struct GatewayRunConfig {
    /// Invocation id used by the external event stream and run manifest.
    pub run_id: Uuid,
    /// The prompt delivered as the single user message of the run.
    pub prompt: String,
    /// Agent name segment; the runtime path is `/root/<name>`.
    /// `None` uses the stable legacy name `headless`.
    pub agent_name: Option<String>,
    /// Agent kind to spawn (`researcher`/`developer`); `None` defaults to
    /// researcher.
    pub profile: Option<String>,
    /// Runtime overrides sent with the spawn request.
    pub agent_runtime: AgentRuntimeOverrides,
    /// Model override as `provider:model`; `None` uses the daemon's
    /// active model.
    pub model: Option<String>,
    /// Resume this session instead of the agent's active one.
    pub session: Option<Uuid>,
    /// Wall-clock budget; on expiry the run is cancelled (exit code 2).
    pub timeout: Option<Duration>,
    /// Cooperative cancellation requested by the embedding frontend.
    pub cancel: CancellationToken,
}

/// Run one prompt through the resident gateway daemon, streaming
/// translated events into `processor`. See the module docs for the
/// contract.
pub async fn run_via_gateway<P: OutputProcessor>(
    config: GatewayRunConfig,
    processor: &mut P,
) -> Result<RunSummary, RunError> {
    if config.cancel.is_cancelled() {
        return Err(RunError::Cancelled);
    }
    // ── Connect (auto-starting the daemon) ───────────────────────────
    gateway::manager::ensure_running()
        .await
        .map_err(RunError::Gateway)?;
    let token = gateway::manager::read_token();
    let addr = gateway::daemon::env_addr();
    let client = gateway::GatewayClient::new(&addr, token.as_deref())
        .map_err(|e| RunError::Gateway(e.to_string()))?;
    run_via_gateway_with_client(client, config, processor).await
}

/// List persisted sessions for a named headless agent identity.
///
/// This reads storage through the daemon rather than spawning an agent, so it
/// works before the first run and does not require a model to be configured.
pub async fn list_sessions_via_gateway(
    profile: Option<String>,
    agent_name: Option<String>,
) -> Result<Vec<StoredSession>, RunError> {
    let agent_path = named_agent_path(agent_name.as_deref())?;
    gateway::manager::ensure_running()
        .await
        .map_err(RunError::Gateway)?;
    let token = gateway::manager::read_token();
    let addr = gateway::daemon::env_addr();
    let client = gateway::GatewayClient::new(&addr, token.as_deref())
        .map_err(|e| RunError::Gateway(e.to_string()))?;

    let _kind = resolve_kind(profile.as_deref())?;
    let agents = client
        .list_storage_agents()
        .await
        .map_err(|e| RunError::Gateway(e.to_string()))?;
    let Some(record) = agents.iter().find(|record| record.name == agent_path) else {
        return Ok(Vec::new());
    };
    client
        .list_stored_sessions(record.id)
        .await
        .map_err(|e| RunError::Gateway(e.to_string()))
}

/// Same run, against an explicit [`gateway::GatewayClient`] — the
/// `run_via_gateway` body after connection setup, split out so tests
/// (and future embedders) can drive a mock daemon without env games.
pub async fn run_via_gateway_with_client<P: OutputProcessor>(
    client: gateway::GatewayClient,
    config: GatewayRunConfig,
    processor: &mut P,
) -> Result<RunSummary, RunError> {
    let started = Instant::now();
    let agent_name = validated_agent_name(config.agent_name.as_deref())?;

    // ── Resolve kind + model from the daemon snapshot ────────────────
    let state = client
        .state()
        .await
        .map_err(|e| RunError::Gateway(e.to_string()))?;
    let kind = resolve_kind(config.profile.as_deref())?;
    let spawn_runtime = Some(config.agent_runtime.clone());
    // Spawn override = the user's --model only. `None` means "daemon
    // default" — the daemon resolves and attaches its own callbacks;
    // echoing the daemon's active spec back as an override would force a
    // redundant DB re-resolution.
    let spawn_model_spec = config.model.as_deref();
    // A missing model is environment misconfiguration (startup error,
    // exit 3), not a measured turn failure.
    if spawn_model_spec.is_none() && state.active_model_spec.is_none() {
        return Err(RunError::Gateway(
            "no active model configured — set one in the TUI Config tab or pass \
             --model provider_name:model_name"
                .to_string(),
        ));
    }
    // Display name for run.started: --model's part after the colon, else
    // the daemon's active spec (informational only).
    let name_part = |spec: &str| {
        spec.split_once(':')
            .map(|(_, name)| name.to_string())
            .unwrap_or_else(|| spec.to_string())
    };
    let model_name = config
        .model
        .as_deref()
        .map(name_part)
        .or_else(|| state.active_model_spec.as_deref().map(name_part));

    // ── Spawn at the named identity, with a contention fallback ─────
    let agent_path = match client
        .spawn_agent(
            &agent_name,
            "/root",
            kind.name(),
            spawn_runtime.clone(),
            spawn_model_spec,
        )
        .await
    {
        Ok(path) => path,
        Err(error) if error.to_string().contains("already exists") => {
            if config.session.is_some() {
                return Err(RunError::SessionSwitch {
                    session: config.session.expect("session checked above"),
                    message: "the named identity is already live, so this run \
                              received a new agent path and cannot resume that session"
                        .into(),
                });
            }
            // A concurrent run holds the requested path. A unique
            // identity can't share persisted sessions, but a contended
            // run can't resume them anyway.
            let fallback = unique_agent_name(&agent_name);
            tracing::warn!(
                fallback = %fallback,
                requested = %agent_name,
                "another run holds the named agent; spawning with a unique identity"
            );
            client
                .spawn_agent(
                    &fallback,
                    "/root",
                    kind.name(),
                    spawn_runtime.clone(),
                    spawn_model_spec,
                )
                .await
                .map_err(|e| RunError::Spawn {
                    message: e.to_string(),
                })?
        }
        Err(error) => {
            return Err(RunError::Spawn {
                message: error.to_string(),
            });
        }
    };

    // ── Event stream ──────────────────────────────────────────────────
    // Subscribe from the snapshot's seq: every event of this run was
    // published after it (the spawn happened after `state()`), so the
    // replay covers the registration without replaying daemon history.
    let mut last_seq = state.last_seq;
    let response = match client.connect_events(state.last_seq).await {
        Ok(response) => response,
        Err(e) => {
            let _ = client.shutdown_agent(&agent_path).await;
            return Err(RunError::Gateway(e.to_string()));
        }
    };
    let mut stream = response.bytes_stream().eventsource();

    // Registration frame carries the restored agent id (best effort —
    // the run proceeds without it when the frame is late).
    let agent_id = match tokio::time::timeout(REGISTRATION_TIMEOUT, async {
        loop {
            match stream.next().await {
                Some(Ok(event)) => {
                    let Some((seq, parsed)) = gateway::client::parse_frame(&event) else {
                        continue;
                    };
                    last_seq = last_seq.max(seq);
                    if let ParsedFrame::Host(HostEventView::AgentRegistered { path, info }) = parsed
                        && path == agent_path
                    {
                        return info.agent_id.unwrap_or_default();
                    }
                }
                _ => return Uuid::nil(),
            }
        }
    })
    .await
    {
        Ok(id) => id,
        Err(_) => Uuid::nil(),
    };

    // ── Optional session resume ───────────────────────────────────────
    // Fire the switch, then wait for the agent's `SessionActivated`
    // confirmation before delivering the prompt — otherwise a fast
    // prompt can beat the switch and land in the wrong session.
    if let Some(session) = config.session {
        if let Err(e) = client.switch_session(&agent_path, session).await {
            let _ = client.shutdown_agent(&agent_path).await;
            return Err(RunError::Send {
                message: e.to_string(),
            });
        }
        let activated = match tokio::time::timeout(REGISTRATION_TIMEOUT, async {
            loop {
                match stream.next().await {
                    Some(Ok(event)) => {
                        let Some((seq, ParsedFrame::Agent { agent, event })) =
                            gateway::client::parse_frame(&event)
                        else {
                            continue;
                        };
                        let _ = seq;
                        if agent != agent_path {
                            continue;
                        }
                        if let AgentEvent::SessionActivated { id, .. } = event
                            && id == session
                        {
                            return true;
                        }
                    }
                    _ => return false,
                }
            }
        })
        .await
        {
            Ok(true) => true,
            Ok(false) | Err(_) => false,
        };
        if !activated {
            let _ = client.shutdown_agent(&agent_path).await;
            return Err(RunError::SessionSwitch {
                session,
                message: "SessionActivated was not observed before delivery deadline".into(),
            });
        }
    }

    // ── Prompt delivery ───────────────────────────────────────────────
    if let Err(e) = client
        .deliver_message(&agent_path, config.prompt.clone())
        .await
    {
        let _ = client.shutdown_agent(&agent_path).await;
        return Err(RunError::Send {
            message: e.to_string(),
        });
    }

    // ── Event loop ────────────────────────────────────────────────────
    processor.process(&RunEvent::RunStarted(RunStartedEvent {
        run_id: config.run_id,
        agent_id,
        session_id: Uuid::nil(),
        profile: kind.name().to_string(),
        model: model_name,
    }));

    let mut translation = TranslationState::default();
    let deadline = config.timeout.map(|budget| started + budget);
    let terminal = loop {
        if config.cancel.is_cancelled() {
            let _ = client.cancel_agent(&agent_path).await;
            let telemetry = translation.fail_current_turn();
            translation.terminal = Some(Terminal::Cancelled);
            if !processor.is_broken() {
                processor.process(&RunEvent::TurnFailed(TurnFailedEvent {
                    turn_id: translation.current_turn_id,
                    message: "run cancelled".to_string(),
                    telemetry,
                }));
            }
            break Terminal::Cancelled;
        }
        if let Some(deadline) = deadline
            && Instant::now() >= deadline
        {
            let budget = config.timeout.expect("deadline implies timeout");
            // Cooperative cancel first: aborts the in-flight turn so the
            // daemon-side shutdown below doesn't wait on running tools.
            let _ = client.cancel_agent(&agent_path).await;
            let telemetry = translation.fail_current_turn();
            translation.terminal = Some(Terminal::Cancelled);
            if !processor.is_broken() {
                processor.process(&RunEvent::TurnFailed(TurnFailedEvent {
                    turn_id: translation.current_turn_id,
                    message: format!("run timed out after {:.1}s", budget.as_secs_f64()),
                    telemetry,
                }));
            }
            break Terminal::Cancelled;
        }

        // Idle-bounded read so the deadline is re-checked even when the
        // daemon is quiet.
        let next = tokio::time::timeout(Duration::from_millis(250), stream.next()).await;
        let event = match next {
            Err(_) => continue, // idle tick
            Ok(None) | Ok(Some(Err(_))) => {
                // Stream ended (daemon gone) — the run's delta stream is
                // gone with it.
                tracing::warn!("gateway event stream ended mid-run");
                break translation.terminal.take().unwrap_or_else(|| {
                    translation.fail_current_turn();
                    Terminal::Failed
                });
            }
            Ok(Some(Ok(event))) => event,
        };

        let Some((seq, parsed)) = gateway::client::parse_frame(&event) else {
            continue;
        };
        if seq <= last_seq {
            continue; // replay duplicate
        }
        last_seq = seq;

        match parsed {
            ParsedFrame::Agent { agent, event } if agent == agent_path => {
                for run_event in translation.translate(&agent, event) {
                    processor.process(&run_event);
                    if processor.is_broken() {
                        config.cancel.cancel();
                    }
                }
                if translation.terminal.is_some() {
                    // Drain the already-queued compatibility events
                    // (Done / Error) without blocking long.
                    let drain_until = tokio::time::Instant::now() + TERMINAL_DRAIN;
                    while tokio::time::Instant::now() < drain_until {
                        match tokio::time::timeout_at(drain_until, stream.next()).await {
                            Ok(Some(Ok(event))) => {
                                let Some((_, ParsedFrame::Agent { agent, event })) =
                                    gateway::client::parse_frame(&event)
                                else {
                                    continue;
                                };
                                if agent != agent_path {
                                    continue;
                                }
                                for run_event in translation.translate(&agent, event) {
                                    processor.process(&run_event);
                                    if processor.is_broken() {
                                        config.cancel.cancel();
                                    }
                                }
                            }
                            _ => break,
                        }
                    }
                    break translation.terminal.take().expect("checked above");
                }
            }
            ParsedFrame::Lag { missed, .. } => {
                // Events were dropped beyond the replay window — the run's
                // delta stream is compromised and the terminal event may
                // be gone. Fail loudly instead of hanging.
                let telemetry = translation.fail_current_turn();
                processor.process(&RunEvent::TurnFailed(TurnFailedEvent {
                    turn_id: translation.current_turn_id,
                    message: format!(
                        "gateway event stream lagged ({missed} events missed) — run aborted"
                    ),
                    telemetry,
                }));
                translation.terminal = Some(Terminal::Failed);
                break Terminal::Failed;
            }
            // Other agents' events, host events, notices — not this run's.
            _ => {}
        }
    };

    // ── Summary ───────────────────────────────────────────────────────
    let wall_time_secs = started.elapsed().as_secs_f64();
    let status = match &terminal {
        Terminal::Completed => RunStatus::Completed,
        Terminal::Failed => RunStatus::Failed,
        Terminal::Cancelled => RunStatus::Cancelled,
    };
    let usage = (translation.turns > 0).then_some(translation.run_usage);
    let turns = translation.turns;
    let tool_calls = translation.tool_calls;
    let telemetry = translation.run_telemetry;
    if !processor.is_broken() {
        processor.process(&RunEvent::RunEnded(RunEndedEvent {
            run_id: config.run_id,
            status,
            wall_time_secs,
            usage,
            turns,
            tool_calls,
            telemetry,
        }));
    }
    processor.finish();

    // Clean up the one-shot agent (the daemon flushes session snapshots
    // during shutdown; the stored agent record remains for the next
    // run's identity restore).
    let _ = client.shutdown_agent(&agent_path).await;

    Ok(RunSummary {
        run_id: config.run_id,
        profile: kind.name().to_string(),
        outcome: match terminal {
            Terminal::Completed => crate::processor::Outcome::Completed,
            Terminal::Failed => crate::processor::Outcome::Failed,
            Terminal::Cancelled => crate::processor::Outcome::Cancelled,
        },
        agent_path,
        last_message: processor.last_message().map(str::to_owned),
        wall_time_secs,
        usage,
        turns,
        tool_calls,
        telemetry,
    })
}

fn validated_agent_name(name: Option<&str>) -> Result<String, RunError> {
    let name = name.unwrap_or(DEFAULT_HEADLESS_AGENT_NAME);
    agentik_types::validate_segment(name)
        .map_err(|error| RunError::Spawn {
            message: format!("invalid agent name `{name}`: {error}"),
        })
        .map(|()| name.to_string())
}

fn named_agent_path(name: Option<&str>) -> Result<String, RunError> {
    Ok(format!("/root/{}", validated_agent_name(name)?))
}

fn unique_agent_name(name: &str) -> String {
    // AgentPath caps every segment at 32 ASCII characters.
    let base_len = name.len().min(23);
    format!(
        "{}_{}",
        &name[..base_len],
        &Uuid::new_v4().simple().to_string()[..8]
    )
}
