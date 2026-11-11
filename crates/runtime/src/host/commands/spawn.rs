//! Lifecycle commands: `Spawn` (with the idempotent live-instance resume
//! and the panic-safe background spawn), `Shutdown`, and `CancelAgent` —
//! plus the child-scoping that gates agent-initiated control.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use agentik_core::profile::AgentProfileConfig;
use agentik_sdk::model::Model;
use arc_swap::ArcSwapOption;
use futures::FutureExt;

use super::super::RuntimeHost;
use super::super::events::{ActivationCause, RemovalCause, SpawnNotice};
use super::super::relay::AgentCommand;
use super::capability_from_kind;
use super::resolution::not_a_direct_child_message;

impl RuntimeHost {
    /// `HostCommand::Spawn` — derive the child path from the caller's
    /// path, resume a live instance in place, or spawn in the background.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn handle_spawn(
        &mut self,
        name: String,
        caller_path: agentik_types::AgentPath,
        config: AgentProfileConfig,
        model_override: Option<Model>,
        operator: bool,
        reply_tx: tokio::sync::oneshot::Sender<std::result::Result<String, String>>,
    ) {
        let cause = if operator {
            ActivationCause::UserInitiated
        } else {
            ActivationCause::ToolChild
        };
        // Derive child path from caller's path + the LLM-provided segment.
        let child_path = match caller_path.join(&name) {
            Ok(p) => p,
            Err(e) => {
                let _ = reply_tx.send(Err(format!("invalid agent name `{name}`: {e}")));
                return;
            }
        };
        // Idempotent activate: an already-live identity is
        // returned as-is, not an error. Virtual-actor resume
        // (frontend re-opens a tab whose instance the daemon
        // still holds — close-tab no longer shuts down) flows
        // through this same call. The replayed AgentRegistered
        // event makes the reply indistinguishable from a fresh
        // spawn, so frontends populate the reopened tab.
        if let Some(entry) = self.agents.get(child_path.as_str()) {
            let info = self.snapshot_info(entry);
            let _ = reply_tx.send(Ok(child_path.as_str().to_string()));
            self.emit_host_event(super::super::events::HostEvent::AgentRegistered {
                path: child_path.clone(),
                info,
                cause,
            });
            tracing::info!(agent = %child_path, "spawn hit a live instance — resumed in place");
            return;
        }
        // Resolve model: use override if provided, else global model.
        let model = match (model_override, self.model.as_ref()) {
            (Some(m), _) => Arc::new(ArcSwapOption::from_pointee(Some(m))),
            (None, Some(g)) => g.clone(),
            (None, None) => {
                let _ = reply_tx.send(Err("No model configured on host.".into()));
                return;
            }
        };

        let infra = self.infra.clone();
        let reg_tx = self.registration_tx.clone();
        let info = capability_from_kind(child_path.name(), child_path.as_str(), config.kind);
        let path_for_spawn = child_path.clone();

        self.infra.runtime_handle.spawn(async move {
            let result = AssertUnwindSafe(async {
                infra
                    .spawn_agent(&path_for_spawn, &config, model, None)
                    .await
            })
            .catch_unwind()
            .await;
            match result {
                Ok(Ok(handle)) => {
                    let registered_path = handle.path.as_str().to_string();
                    let _ = reply_tx.send(Ok(registered_path));
                    let _ = reg_tx.send(SpawnNotice::Activated {
                        handle,
                        info,
                        cause,
                    });
                }
                Ok(Err(e)) => {
                    let _ = reply_tx.send(Err(e.to_string()));
                }
                Err(panic_payload) => {
                    let msg = panic_payload
                        .downcast_ref::<&'static str>()
                        .map(|s| (*s).to_string())
                        .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "<panic in spawn_agent>".to_string());
                    tracing::error!(
                        target: "spawn_safe",
                        task = "host::spawn_agent",
                        panic = %msg,
                        "spawn_agent task panicked"
                    );
                    let _ = reply_tx.send(Err(format!("internal panic: {msg}")));
                }
            }
        });
    }

    /// `HostCommand::Shutdown` — operator path is unrestricted; agents
    /// may shut down only their own direct children.
    pub(super) fn handle_shutdown(
        &mut self,
        name: String,
        caller_path: Option<String>,
        evict: bool,
        reply_tx: Option<tokio::sync::oneshot::Sender<std::result::Result<(), String>>>,
    ) {
        let cause = if evict {
            RemovalCause::Evicted
        } else {
            RemovalCause::Explicit
        };
        let reply = |r: std::result::Result<(), String>| {
            if let Some(tx) = reply_tx {
                let _ = tx.send(r);
            }
        };
        match (self.resolve_agent(&name), caller_path.as_deref()) {
            // Operator path — unrestricted (gateway/TUI semantics).
            (resolved, None) => {
                let resolved = resolved.unwrap_or(name);
                self.shutdown_agent_with(&resolved, cause);
                reply(Ok(()));
            }
            // Agent path with an unresolvable target: same message
            // as the not-a-child denial so callers cannot probe the
            // registry for the existence of foreign agents.
            (None, Some(_)) => {
                reply(Err(not_a_direct_child_message(&name)));
            }
            (Some(resolved), Some(caller)) => {
                match self.validate_child_control(caller, &resolved, "shut down") {
                    Err(error) => reply(Err(error)),
                    Ok(()) => {
                        self.shutdown_agent_with(
                            &resolved,
                            super::super::events::RemovalCause::Explicit,
                        );
                        reply(Ok(()));
                    }
                }
            }
        }
    }

    /// `HostCommand::CancelAgent` — interrupt the current turn. Same
    /// child-scoping as shutdown; the agent itself survives.
    pub(super) fn handle_cancel_agent(
        &mut self,
        name: String,
        caller_path: Option<String>,
        reply_tx: Option<tokio::sync::oneshot::Sender<std::result::Result<(), String>>>,
    ) {
        let reply = |r: std::result::Result<(), String>| {
            if let Some(tx) = reply_tx {
                let _ = tx.send(r);
            }
        };
        match (self.resolve_agent(&name), caller_path.as_deref()) {
            // Operator path — unrestricted (gateway/TUI semantics).
            (resolved, None) => {
                let resolved = resolved.unwrap_or(name);
                self.send_agent_command(&resolved, AgentCommand::Cancel);
                reply(Ok(()));
            }
            // Same denial as the not-a-child case: no registry probe.
            (None, Some(_)) => {
                reply(Err(not_a_direct_child_message(&name)));
            }
            (Some(resolved), Some(caller)) => {
                match self.validate_child_control(caller, &resolved, "interrupt") {
                    Err(error) => reply(Err(error)),
                    Ok(()) => {
                        self.send_agent_command(&resolved, AgentCommand::Cancel);
                        reply(Ok(()));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod spawn_tests {
    //! Spawn-payload persistence and per-kind resource lifecycle.

    use std::sync::Arc;

    use agentik_core::profile::{AgentKind, AgentProfileConfig};
    use agentik_sdk::model::Model;
    use arc_swap::ArcSwapOption;

    use super::super::test_support::config;
    use crate::host::RuntimeHost;

    /// Guard for the spawn payload narrowing: the per-agent config handed to
    /// `spawn_agent` — including `preferred_model` — must land in
    /// `agents.config_json` so restarts restore the preference.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_persists_preferred_model_into_agent_config() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = RuntimeHost::open(&config(&dir)).await.unwrap();
        let path = agentik_types::AgentPath::try_from("/root/researcher").unwrap();
        let config = AgentProfileConfig {
            kind: AgentKind::Researcher,
            preferred_model: Some("mock:mock-model".into()),
            runtime: agentik_core::AgentRuntimeOverrides {
                use_memory: Some(false),
                generate_memory: None,
                reasoning_effort: None,
            },
        };
        let _handle = host
            .spawn_agent(
                &path,
                &config,
                Arc::new(ArcSwapOption::from_pointee(None)),
                None,
            )
            .await
            .unwrap();

        // The record is upserted asynchronously by the agent's run loop.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let storage = host.infra().storage.clone();
        let record = storage
            .get_agent_by_name(path.as_str())
            .await
            .unwrap()
            .expect("agent record missing after spawn");
        let restored = AgentProfileConfig::from_json(&record.config_json);
        assert_eq!(restored.kind, AgentKind::Researcher);
        assert_eq!(restored.preferred_model.as_deref(), Some("mock:mock-model"));
        assert_eq!(restored.runtime.use_memory, Some(false));

        host.shutdown_all_agents_and_wait().await;
    }

    /// A browser-kind agent gets a private Chromium session allocated at
    /// spawn time. The session's strong `Arc` lives in the agent's tool
    /// list, the host pool keeps only a `Weak`, and the session dies with
    /// the agent — spawning a browser child and shutting it down must not
    /// leak the browser process or the pool entry.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn browser_agent_session_lifecycle() {
        fn automation_chromium_count() -> usize {
            std::process::Command::new("pgrep")
                .args(["-f", "enable-automation"])
                .output()
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .count()
                })
                .unwrap_or(0)
        }

        let dir = tempfile::tempdir().unwrap();
        let mut host = RuntimeHost::open(&config(&dir)).await.unwrap();
        let path = agentik_types::AgentPath::try_from("/root/browser").unwrap();
        let profile = AgentProfileConfig::new(AgentKind::Browser);
        let handle = host
            .spawn_agent(
                &path,
                &profile,
                Arc::new(ArcSwapOption::from_pointee(None)),
                None,
            )
            .await
            .unwrap();
        // Register with the relay so Shutdown actually reaches the agent
        // task (and its tool list) during shutdown_all_agents_and_wait.
        let info =
            super::capability_from_kind(handle.path.name(), handle.path.as_str(), profile.kind);
        host.register_agent(handle, info);

        // Tool assembly registered the browser tool group; the second
        // assembly call must reuse the live session from the pool rather
        // than launching a second Chromium.
        let tools = host
            .infra()
            .tools_from_profile(&path, AgentKind::Browser)
            .await
            .unwrap();
        let names: Vec<String> = tools.iter().map(|t| t.definition.name.clone()).collect();
        for expected in [
            "browser_navigate",
            "browser_click",
            "browser_type_text",
            "browser_screenshot",
            "browser_evaluate",
            "browser_download",
            "browser_wait_for_download",
            "browser_list_downloads",
            "browser_list_tabs",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "missing tool `{expected}`"
            );
        }

        // Downloads routinely outlast a turn, so `browser_download` must be
        // dispatched as a background task; a sync registration would block
        // the agent on the toolset timeout and lose the transfer.
        let download = tools
            .iter()
            .find(|t| t.definition.name == "browser_download")
            .expect("browser_download tool is registered");
        assert_eq!(
            download.implementation.execution_mode(),
            agentik_core::tools::ExecutionMode::Async,
            "browser_download must run in the background"
        );
        // This local tool list holds its own strong Arcs — drop it so the
        // agent's tool list is the only remaining owner before shutdown.
        drop(tools);

        // The pool entry is alive while the agent runs.
        assert!(
            host.infra()
                .browsers
                .lock()
                .await
                .session_alive("/root/browser"),
            "pool should hold a live session while the agent runs"
        );
        assert!(
            automation_chromium_count() > 0,
            "expected a chromium process for the browser agent"
        );

        host.shutdown_all_agents_and_wait().await;

        // The agent task has joined; its tool list (the strong Arc owner)
        // is dropped, so the pool entry dangles and the process is killed.
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let alive = host
                    .infra()
                    .browsers
                    .lock()
                    .await
                    .session_alive("/root/browser");
                if !alive {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("session Arc should drop after the agent shuts down");
    }
}
