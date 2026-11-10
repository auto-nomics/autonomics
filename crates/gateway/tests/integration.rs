//! End-to-end tests for the gateway daemon: a real axum server on an
//! ephemeral port, a mock model, and a real `GatewayClient` + SSE pump.
//! No network beyond loopback, no API keys — the same reliability
//! keystone the headless tests established, now over the wire.

use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use gateway::client::{EventPump, GatewayClient, GatewayFrame};
use gateway::proto::HostEventView;
use gateway::testing::{TEST_TIMEOUT, start_mock_gateway};

/// Collect pump frames until `pred` matches; fails on timeout.
async fn wait_for_frame<F>(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<GatewayFrame>,
    pred: F,
) -> GatewayFrame
where
    F: Fn(&GatewayFrame) -> bool,
{
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            let frame = rx.recv().await.expect("pump channel open");
            if pred(&frame) {
                return frame;
            }
        }
    })
    .await
    .expect("frame arrived within timeout")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn context_hydration_uses_bound_session_without_activating_dormant_agent() {
    use agentik_core::storage::AgentRecord;
    use agentik_sdk::AgentMessageExt;
    use uuid::Uuid;

    let daemon = start_mock_gateway("unused").await;
    let client = daemon.client();
    let storage = daemon.infra.storage.as_ref();
    let id = Uuid::new_v4();
    let path = "/root/parent/methylation_method";
    storage
        .upsert_agent(AgentRecord {
            id,
            name: path.into(),
            config_json: serde_json::json!({}),
            created_at: 1,
            last_active: 1,
        })
        .await
        .unwrap();
    let bound = Uuid::new_v4();
    storage.start_session(id, bound).await.unwrap();
    storage
        .append_message(
            bound,
            &agentik_sdk::types::messages::Message::user("bound conversation"),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    let newer = Uuid::new_v4();
    storage.start_session(id, newer).await.unwrap();
    storage
        .append_message(
            newer,
            &agentik_sdk::types::messages::Message::user("unrelated newer conversation"),
        )
        .await
        .unwrap();
    storage.bind_agent_context(id, bound).await.unwrap();

    // TUI and GUI use this same context response and history endpoint.
    let context = client.list_contexts(path).await.unwrap();
    assert_eq!(context.agent_id, id);
    assert_eq!(context.context.unwrap().id, bound);
    let messages = client.session_history(id, bound).await.unwrap();
    assert_eq!(messages.len(), 1);
    assert!(
        serde_json::to_string(&messages)
            .unwrap()
            .contains("bound conversation")
    );
    assert!(client.state().await.unwrap().agents.is_empty());
    let found = client
        .search_storage_agents("methylation_method", 100)
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, id);

    let empty_id = Uuid::new_v4();
    storage
        .upsert_agent(AgentRecord {
            id: empty_id,
            name: "/root/empty".into(),
            config_json: serde_json::json!({}),
            created_at: 2,
            last_active: 2,
        })
        .await
        .unwrap();
    let empty = client.list_contexts("/root/empty").await.unwrap();
    assert_eq!(empty.agent_id, empty_id);
    assert!(empty.context.is_none());
    assert_eq!(
        client
            .list_contexts("/root/missing")
            .await
            .unwrap_err()
            .status(),
        Some(404)
    );
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spawn_chat_turn_and_shutdown_over_the_wire() {
    let gateway_daemon = start_mock_gateway("The answer is 2.").await;
    let client = gateway_daemon.client();

    // Hydration first.
    let state = client.state().await.unwrap();
    assert!(state.agents.is_empty());
    // researcher + developer + browser + curator (AgentKind::ALL).
    assert_eq!(state.profiles.len(), agentik_core::AgentKind::ALL.len());

    // Start the event pump.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    EventPump::spawn(client.clone(), state.last_seq, tx);

    // Spawn an agent from the first kind.
    let path = client
        .spawn_agent("worker", "/root", "researcher", None, None)
        .await
        .unwrap();
    assert!(
        path.ends_with("worker"),
        "spawn returns the full path: {path}"
    );

    // Registration arrives as a host frame.
    let registered = wait_for_frame(&mut rx, |f| {
        matches!(
            f,
            GatewayFrame::Sequenced {
                inner: gateway::client::FrameInner::Host(HostEventView::AgentRegistered { .. }),
                ..
            }
        )
    })
    .await;
    let GatewayFrame::Sequenced {
        seq: registered_seq,
        inner: gateway::client::FrameInner::Host(HostEventView::AgentRegistered { info, cause, .. }),
    } = registered
    else {
        unreachable!()
    };
    // The capability info carries the live model — frontends read this
    // field to render the agent's model without extra round-trips.
    assert_eq!(info.model.as_deref(), Some("mock-model"));
    assert_eq!(info.context_length, Some(8192));
    // A gateway spawn is operator-initiated: focus may steal.
    assert_eq!(cause, gateway::proto::ActivationCauseView::UserInitiated);

    // Deliver a user message; the scripted mock model completes the turn.
    client.deliver_message(&path, "What is 1+1?").await.unwrap();
    let completed = wait_for_frame(&mut rx, |f| {
        matches!(
            f,
            GatewayFrame::Sequenced {
                inner: gateway::client::FrameInner::Agent {
                    event: agentik_types::AgentEvent::TurnCompleted { .. },
                    ..
                },
                ..
            }
        )
    })
    .await;

    // Every frame carries an increasing seq — the hub's total order.
    let GatewayFrame::Sequenced {
        seq: completed_seq, ..
    } = completed
    else {
        unreachable!()
    };
    assert!(completed_seq > registered_seq);

    // /state now lists the live agent, and the driver's context cache has
    // the agent's sole context (requested at registration, refreshed after
    // each completed turn).
    let refreshed = tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            let snapshot = client.state().await.unwrap();
            if snapshot.agents.iter().any(|a| a.path == path)
                && snapshot.contexts.contains_key(&path)
            {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("agent + contexts visible in /state");
    assert!(refreshed.contexts.contains_key(&path));
    let state_agent = refreshed
        .agents
        .iter()
        .find(|a| a.path == path)
        .expect("agent listed in /state");
    assert_eq!(state_agent.model.as_deref(), Some("mock-model"));
    let context = client.list_contexts(&path).await.unwrap();
    assert_eq!(Some(context.agent_id), state_agent.agent_id);
    assert_eq!(context.context.unwrap().id, refreshed.contexts[&path].id);

    // Hot-swap the model through the same control path the gateway's
    // PUT /agents/{name}/model handler uses, then expect the change
    // event and a refreshed /state (the relay stores asynchronously, so
    // poll until the snapshot flips).
    gateway_daemon
        .infra
        .host_control
        .as_ref()
        .expect("host control installed")
        .set_agent_model(
            &path,
            gateway::testing::renamed_mock_model("mock-model-b", 4096),
        );
    let changed = wait_for_frame(&mut rx, |f| {
        matches!(
            f,
            GatewayFrame::Sequenced {
                inner: gateway::client::FrameInner::Host(HostEventView::AgentModelChanged {
                    spec,
                    context_length,
                    ..
                }),
                ..
            } if spec == "mock-model-b" && *context_length == 4096
        )
    })
    .await;
    assert!(matches!(
        changed,
        GatewayFrame::Sequenced { seq, .. } if seq > registered_seq
    ));
    let swapped = tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            let snapshot = client.state().await.unwrap();
            if snapshot
                .agents
                .iter()
                .any(|a| a.path == path && a.model.as_deref() == Some("mock-model-b"))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(swapped.is_ok(), "/state reflects the swapped model");

    let records = client.list_storage_agents().await.unwrap();
    let record = records
        .iter()
        .find(|record| record.name == path)
        .expect("worker agent record persisted");
    let stored_sessions = client.list_stored_sessions(record.id).await.unwrap();
    assert!(
        stored_sessions.iter().any(|session| session.active),
        "stored session list includes the live session"
    );

    let initial_config = client.agent_runtime_config(&path).await.unwrap();
    assert!(!initial_config.effective.use_memory);
    assert!(!initial_config.effective.generate_memory);
    let saved_config = client
        .set_agent_runtime_config(
            &path,
            agentik_core::AgentRuntimeOverrides {
                use_memory: Some(true),
                generate_memory: Some(false),
                reasoning_effort: None,
            },
        )
        .await
        .unwrap();
    assert!(saved_config.effective.use_memory);
    assert!(!saved_config.effective.generate_memory);
    let refreshed_config = client.agent_runtime_config(&path).await.unwrap();
    assert!(refreshed_config.effective.use_memory);
    assert!(!refreshed_config.effective.generate_memory);

    // Replay: reconnect with Last-Event-ID below the terminal frame and
    // observe the replayed TurnCompleted again (dedup is the client's
    // job — the raw stream must still deliver it).
    let replay_response = client.connect_events(registered_seq).await.unwrap();
    let mut replay = replay_response.bytes_stream().eventsource();
    let saw_turn_completed = tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            match replay.next().await {
                Some(Ok(event)) => {
                    if event.event == "agent" && event.data.contains("\"TurnCompleted\"") {
                        return true;
                    }
                }
                _ => return false,
            }
        }
    })
    .await
    .expect("replay delivers the missed frame");
    assert!(saw_turn_completed);

    // Graceful shutdown over the wire.
    gateway_daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rename_storage_agent_evicts_the_live_instance() {
    let gateway_daemon = start_mock_gateway("ok").await;
    let client = gateway_daemon.client();
    let state = client.state().await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    EventPump::spawn(client.clone(), state.last_seq, tx);

    let path = client
        .spawn_agent("worker", "/root", "researcher", None, None)
        .await
        .unwrap();
    wait_for_frame(&mut rx, |f| {
        matches!(
            f,
            GatewayFrame::Sequenced {
                inner: gateway::client::FrameInner::Host(HostEventView::AgentRegistered { .. }),
                ..
            }
        )
    })
    .await;

    let record = client
        .list_storage_agents()
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.name == path)
        .expect("spawned identity persisted");

    // Rename the identity: storage flips to the new path and the live
    // instance running under the old path is evicted (unregistered) —
    // otherwise it would keep emitting under a path no record claims and
    // a later re-activation could double-activate the identity.
    let new_path = "/root/renamed".to_string();
    client
        .rename_agent_record(record.id, &new_path)
        .await
        .unwrap();

    // Eviction of the old-path instance carries cause `evicted`…
    let unregistered = wait_for_frame(&mut rx, |f| {
        matches!(
            f,
            GatewayFrame::Sequenced {
                inner: gateway::client::FrameInner::Host(HostEventView::AgentUnregistered {
                    path: p,
                    cause: gateway::proto::RemovalCauseView::Evicted,
                }),
                ..
            } if *p == path
        )
    })
    .await;
    assert!(matches!(unregistered, GatewayFrame::Sequenced { .. }));
    // …followed by the identity event every connected frontend uses to
    // rebind its views (not just the client that issued the PATCH).
    let renamed = wait_for_frame(&mut rx, |f| {
        matches!(
            f,
            GatewayFrame::Sequenced {
                inner: gateway::client::FrameInner::Host(HostEventView::IdentityRenamed {
                    agent_id,
                    new_path: np,
                    ..
                }),
                ..
            } if *agent_id == record.id && *np == new_path
        )
    })
    .await;
    assert!(matches!(renamed, GatewayFrame::Sequenced { .. }));

    let gone = tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            let snapshot = client.state().await.unwrap();
            if !snapshot.agents.iter().any(|a| a.path == path) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(gone.is_ok(), "/state no longer lists the old path");

    let records = client.list_storage_agents().await.unwrap();
    assert!(
        records.iter().any(|record| record.name == new_path),
        "storage lists the renamed path"
    );

    gateway_daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bearer_token_gates_the_api() {
    let gateway_daemon = start_mock_gateway("x").await;

    // Without the token: 401.
    let unauthenticated = GatewayClient::new(&gateway_daemon.addr.to_string(), None).unwrap();
    let err = unauthenticated.state().await.unwrap_err();
    assert_eq!(err.status().unwrap_or(0), 401);

    // With the token: fine.
    gateway_daemon.client().state().await.unwrap();
    gateway_daemon.stop().await;
}

/// The browser frontend must never carry the daemon's bearer token:
/// no HTML bootstrap, no JS global, no console output. The browser
/// authenticates with an `HttpOnly` session cookie instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browser_never_sees_the_bearer_token() {
    let gateway_daemon = start_mock_gateway("x").await;
    let base = format!("http://{}", gateway_daemon.addr);
    let http = reqwest::Client::new();
    let token = &gateway_daemon.token;

    // The served page carries no credential at all.
    let page = http.get(&base).send().await.unwrap().text().await.unwrap();
    assert!(!page.contains(token), "the page leaked the bearer token");
    assert!(
        !page.contains("__AUTONOMICS_GATEWAY_TOKEN__"),
        "the page still bootstraps a token global"
    );

    // The login endpoint does not echo the token either.
    let body = serde_json::json!({ "token": token });
    let response = http
        .post(format!("{base}/api/session"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let set_cookie = response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .expect("session cookie")
        .to_str()
        .unwrap()
        .to_owned();
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
    assert!(set_cookie.contains("SameSite=Strict"), "{set_cookie}");
    let text = response.text().await.unwrap();
    assert!(!text.contains(token), "the login response echoed the token");

    // The cookie alone opens the API.
    let cookie = set_cookie
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned();
    let authorized = http
        .get(format!("{base}/api/v1/state"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(authorized.status(), 200);

    // Without it: 401 — the cookie is the only browser credential.
    let anonymous = http
        .get(format!("{base}/api/v1/state"))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);

    gateway_daemon.stop().await;
}

/// A wrong token must not mint a session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_login_rejects_a_wrong_token() {
    let gateway_daemon = start_mock_gateway("x").await;
    let base = format!("http://{}", gateway_daemon.addr);
    let http = reqwest::Client::new();

    let response = http
        .post(format!("{base}/api/session"))
        .json(&serde_json::json!({ "token": "not-the-token" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert!(
        response
            .headers()
            .get(reqwest::header::SET_COOKIE)
            .is_none(),
        "a rejected login must not set a cookie"
    );

    gateway_daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn swagger_docs_expose_the_gateway_api() {
    let gateway_daemon = start_mock_gateway("x").await;
    let base = format!("http://{}", gateway_daemon.addr);
    let http = reqwest::Client::new();

    let openapi: serde_json::Value = http
        .get(format!("{base}/api/v1/api-docs/openapi.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(openapi["info"]["title"], "Autonomics Gateway API");
    assert_eq!(openapi["paths"].as_object().unwrap().len(), 54);
    let expected_paths = [
        "/api/v1/gateway/status",
        "/api/v1/state",
        "/api/v1/profiles",
        "/api/v1/agents",
        "/api/v1/agents/{name}/messages",
        "/api/v1/agents/{name}/cancel",
        "/api/v1/agents/{name}/compact",
        "/api/v1/agents/{name}/shutdown",
        "/api/v1/agents/{name}/model",
        "/api/v1/agents/{name}/config",
        "/api/v1/agents/{name}/dag",
        "/api/v1/agents/{name}/flow",
        "/api/v1/agents/{name}/context",
        "/api/v1/agents/{agent_id}/sessions/{session_id}/history",
        "/api/v1/agents/{agent_id}/sessions/{session_id}/plan",
        "/api/v1/storage/agents",
        "/api/v1/storage/agents/{id}",
        "/api/v1/storage/agents/{id}/sessions",
        "/api/v1/model-config",
        "/api/v1/model-config/provider",
        "/api/v1/model-config/model",
        "/api/v1/model-config/active-model",
        "/api/v1/model-config/chatgpt/login",
        "/api/v1/model-config/chatgpt/refresh",
        "/api/v1/model-config/providers/{name}/catalog",
        "/api/v1/settings",
        "/api/v1/plugins",
        "/api/v1/plugin-projects",
        "/api/v1/plugins/environments",
        "/api/v1/plugins/environments/dockerhub",
        "/api/v1/plugins/environments/dockerhub/tag",
        "/api/v1/plugins/environments/approve",
        "/api/v1/plugins/environments/{id}",
        "/api/v1/events",
        "/api/v1/skills/library",
        "/api/v1/skills/library/{name}",
        "/api/v1/vfs/list",
        "/api/v1/vfs/stat",
        "/api/v1/vfs/mounts",
        "/api/v1/vfs/file",
        "/api/v1/vfs/text",
        "/api/v1/vfs/mkdir",
        "/api/v1/vfs/entry",
        "/api/v1/vfs/move",
        "/api/v1/vfs/copy",
        "/api/v1/plugin-projects/{name}/nodes",
        "/api/v1/plugin-projects/{name}/dev-session",
        "/api/v1/plugin-projects/{name}/validate",
        "/api/v1/plugin-projects/{name}/activate",
        "/api/v1/plugin-projects/{name}/report",
        "/api/v1/plugin-projects/{name}/nodes/{kind}/test-run",
        "/api/v1/plugins/{name}/uninstall",
        "/api/v1/plugins/{name}/rollback",
        "/api/v1/plugins/{name}/fork",
    ];
    for path in expected_paths {
        assert!(
            openapi["paths"].as_object().unwrap().contains_key(path),
            "missing OpenAPI path: {path}"
        );
    }
    assert_eq!(
        openapi["components"]["securitySchemes"]["bearer_auth"]["type"],
        "http"
    );
    assert!(
        openapi["paths"]
            .as_object()
            .unwrap()
            .values()
            .flat_map(serde_json::Value::as_object)
            .flat_map(|operations| operations.values())
            .all(|operation| operation["security"][0]["bearer_auth"].is_array())
    );

    let swagger = http.get(format!("{base}/swagger-ui")).send().await.unwrap();
    assert!(swagger.status().is_success());
    assert!(swagger.text().await.unwrap().contains("swagger-ui"));

    // Documentation is metadata-only; operational endpoints stay guarded.
    let unauthorized = http
        .get(format!("{base}/api/v1/state"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);

    gateway_daemon.stop().await;
}

// ── skill library endpoints ───────────────────────────────────────────

#[tokio::test]
async fn skill_library_lists_installed_tiers_with_detail() {
    let gw = start_mock_gateway("ok").await;
    let client = gw.client();
    let manager = gw.infra.skills.clone();

    // The builtin tier is always present; nothing else installed yet.
    let library = client.skill_library().await.unwrap();
    assert!(
        library.iter().all(|s| s.installed),
        "fresh state: only installed (builtin) rows"
    );
    assert!(library.iter().any(|s| s.tier == "builtin"));

    // Install one skill into the global tier through the manager (the
    // same path autonomics-skills install takes) and re-list.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("demo-skill");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: demo-skill\ndescription: Demo skill for library listing.\n---\nStep one.\n",
    )
    .unwrap();
    manager.install_local(&dir).unwrap();

    let library = client.skill_library().await.unwrap();
    assert!(library.iter().any(|s| s.tier == "builtin"));
    let row = library
        .iter()
        .find(|s| s.name == "demo-skill")
        .expect("installed skill listed");
    assert_eq!(row.tier, "global");
    assert!(row.installed);

    // Detail of the installed row: metadata plus the SKILL.md body.
    let detail = client.skill_library_detail("demo-skill").await.unwrap();
    assert!(detail.installed);
    assert_eq!(detail.tier, "global");
    assert!(detail.body.contains("Step one."));

    // Unknown names 404 rather than fabricating a row.
    let missing = client.skill_library_detail("no-such-skill").await;
    assert!(missing.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spawn_rejects_unknown_agent_kind() {
    let gateway_daemon = start_mock_gateway("unused").await;
    let client = gateway_daemon.client();

    let error = client
        .spawn_agent("worker", "/root", "wizard", None, None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("unknown agent kind"),
        "unexpected error: {error}"
    );
}

/// The served catalog must carry the SDK's built-in provider presets —
/// thin clients (the GUI) have no in-process registry and would otherwise
/// show no built-in models at all (only storage rows such as ChatGPT's).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_config_catalog_serves_builtin_presets() {
    use agentik_sdk::provider::registry;

    let gateway_daemon = start_mock_gateway("unused").await;
    let client = gateway_daemon.client();

    let catalog = client.model_catalog().await.unwrap();

    // Every built-in provider type is listed, configured or not; the test
    // database is fresh, so all rows are credential-less placeholders.
    let types = registry::known_provider_types();
    assert_eq!(
        catalog.providers.len(),
        types.len(),
        "one row per known provider type"
    );
    assert!(
        catalog
            .providers
            .iter()
            .all(|p| p.id < 0 && p.api_key.is_empty()),
        "placeholder rows use negative ids and carry no credentials"
    );

    // Presets are served for the registry types (zai's GLM flagship among
    // them), marked non-custom, and grouped by provider then model name.
    let zai = registry::preset_models(&agentik_sdk::model::ProviderType::from("zai"))
        .unwrap()
        .len();
    assert_eq!(
        catalog
            .models
            .iter()
            .filter(|m| m.provider_name == "zai")
            .count(),
        zai,
        "zai presets are served in full"
    );
    assert!(
        catalog
            .models
            .iter()
            .any(|m| m.provider_name == "zai" && !m.custom && m.model_name.starts_with("glm")),
        "zai's GLM presets appear"
    );
    assert!(
        catalog.models.iter().all(|m| !m.custom),
        "preset rows are not custom"
    );
    let keys: Vec<(&str, &str)> = catalog
        .models
        .iter()
        .map(|m| (m.provider_name.as_str(), m.model_name.as_str()))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "grouped by provider then model name");

    gateway_daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_compute_target_keeps_history_and_requires_another_active_target() {
    let daemon = start_mock_gateway("ok").await;
    let client = daemon.client();
    let initial = client.compute_status().await.unwrap();
    let saved = client.save_compute_target_json(&serde_json::json!({"expected_revision":initial.state.revision,"id":"other","name":"Other","enabled":true,"backend":{"type":"local"}})).await.unwrap();
    let selected = client
        .select_compute_target_by_id(saved.revision, "other".into())
        .await
        .unwrap();
    assert!(
        client
            .delete_compute_target("other", selected.revision)
            .await
            .is_err()
    );
    let selected = client
        .select_compute_target_by_id(selected.revision, "local".into())
        .await
        .unwrap();
    let deleted = client
        .delete_compute_target("other", selected.revision)
        .await
        .unwrap();
    assert!(deleted.deleted_target_ids.contains("other"));
    assert!(deleted.targets.iter().any(|target| target.id == "other"));
    assert!(
        client
            .select_compute_target_by_id(deleted.revision, "other".into())
            .await
            .is_err()
    );
    daemon.stop().await;
}

// ── HPC site config ─────────────────────────────────────────────────────

/// GET serves the effective snapshot; PUT persists the profile atomically,
/// flags `pending_restart`, and leaves the live config untouched until a
/// restart. Invalid values are rejected without writing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hpc_config_round_trip_marks_pending_restart() {
    let gateway_daemon = start_mock_gateway("ok").await;
    let client = gateway_daemon.client();
    let before = client.hpc_config().await.unwrap();
    assert_eq!(before["target_id"], "local");
    assert_eq!(before["editable"], false);
    let bad = client.update_hpc_config(&serde_json::json!({"target_id":"local","target_revision":before["target_revision"],"expected_revision":before["registry_revision"],"config":{"ssh_host":"example"}})).await;
    assert!(bad.is_err(), "a local target cannot be edited as Slurm");
    gateway_daemon.stop().await;
}

/// Unknown profile fields fail closed — a typo must not silently disable a
/// production setting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hpc_config_rejects_unknown_fields() {
    let gateway_daemon = start_mock_gateway("ok").await;
    let client = gateway_daemon.client();
    let err = client
        .update_hpc_config(&serde_json::json!({"nonsense_field": 1}))
        .await;
    assert!(err.is_err(), "unknown profile fields must be rejected");

    gateway_daemon.stop().await;
}

// ── VFS browser endpoints ─────────────────────────────────────────────

/// Read-side VFS endpoints over the wire: list, stat, mounts, text and
/// the streaming file endpoint (full + Range + 416 + UTF-8 refusal).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn vfs_read_endpoints_list_stat_stream_and_bound_text() {
    let gateway_daemon = start_mock_gateway("x").await;
    let base = format!("http://{}", gateway_daemon.addr);
    let http = reqwest::Client::new();
    let token = &gateway_daemon.token;

    // Seed fixtures straight through the daemon's file storage.
    gateway_daemon
        .infra
        .file_storage
        .write_bytes("/vfs-e2e/hello.txt", b"hello vfs browser".to_vec())
        .await
        .unwrap();
    gateway_daemon
        .infra
        .file_storage
        .write_bytes("/vfs-e2e/blob.bin", vec![0xff, 0xfe, 0x00, 0x80])
        .await
        .unwrap();

    // list / contains the fixture directory.
    let list: serde_json::Value = http
        .get(format!("{base}/api/v1/vfs/list"))
        .query(&[("path", "/")])
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<&str> = list["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"/vfs-e2e"),
        "expected /vfs-e2e in root listing, got {names:?}"
    );

    // Nested listing works and reports leaf names.
    let nested: serde_json::Value = http
        .get(format!("{base}/api/v1/vfs/list"))
        .query(&[("path", "/vfs-e2e")])
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let paths: Vec<&str> = nested["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"/vfs-e2e/hello.txt"), "{paths:?}");
    let hello = nested["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == "/vfs-e2e/hello.txt")
        .unwrap();
    assert_eq!(hello["name"], "hello.txt");
    assert_eq!(hello["is_dir"], false);
    assert_eq!(hello["uri"], "local://vfs-e2e/hello.txt");

    // stat reports size and the covering mount's read-only flag.
    let stat: serde_json::Value = http
        .get(format!("{base}/api/v1/vfs/stat"))
        .query(&[("path", "local://vfs-e2e/hello.txt")])
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stat["size"], 17);
    assert_eq!(stat["uri"], "local://vfs-e2e/hello.txt");
    assert_eq!(stat["read_only"], false);
    assert!(stat["mount"].is_object(), "root mount should cover it");

    // mounts is non-empty and carries the declarative fields.
    let mounts: serde_json::Value = http
        .get(format!("{base}/api/v1/vfs/mounts"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(mounts["count"].as_u64().unwrap() >= 1);
    assert!(mounts["mounts"][0]["path"].is_string());

    // text returns UTF-8 contents.
    let text: serde_json::Value = http
        .get(format!("{base}/api/v1/vfs/text"))
        .query(&[("path", "local://vfs-e2e/hello.txt")])
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(text["contents"], "hello vfs browser");
    assert_eq!(text["truncated"], false);

    // text truncates at max_bytes.
    let truncated: serde_json::Value = http
        .get(format!("{base}/api/v1/vfs/text"))
        .query(&[("path", "/vfs-e2e/hello.txt"), ("max_bytes", "5")])
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(truncated["contents"], "hello");
    assert_eq!(truncated["truncated"], true);
    assert_eq!(truncated["size"], 17);

    // text refuses binary content with 400.
    let binary = http
        .get(format!("{base}/api/v1/vfs/text"))
        .query(&[("path", "/vfs-e2e/blob.bin")])
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(binary.status(), 400);

    // file streams the full body with a length header.
    let full = http
        .get(format!("{base}/api/v1/vfs/file"))
        .query(&[("path", "local://vfs-e2e/hello.txt")])
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(full.status(), 200);
    assert_eq!(
        full.headers().get(reqwest::header::CONTENT_LENGTH).unwrap(),
        "17"
    );
    assert_eq!(full.bytes().await.unwrap(), b"hello vfs browser".as_slice());

    // Range slice → 206 with the right bytes and Content-Range.
    let ranged = http
        .get(format!("{base}/api/v1/vfs/file"))
        .query(&[("path", "local://vfs-e2e/hello.txt")])
        .header(reqwest::header::RANGE, "bytes=6-8")
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(ranged.status(), 206);
    assert_eq!(
        ranged
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .unwrap(),
        "bytes 6-8/17"
    );
    assert_eq!(ranged.bytes().await.unwrap(), b"vfs".as_slice());

    // Suffix range form.
    let suffix = http
        .get(format!("{base}/api/v1/vfs/file"))
        .query(&[("path", "local://vfs-e2e/hello.txt")])
        .header(reqwest::header::RANGE, "bytes=-7")
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(suffix.status(), 206);
    assert_eq!(suffix.bytes().await.unwrap(), b"browser".as_slice());

    // Unsatisfiable range → 416.
    let unsatisfiable = http
        .get(format!("{base}/api/v1/vfs/file"))
        .query(&[("path", "local://vfs-e2e/hello.txt")])
        .header(reqwest::header::RANGE, "bytes=100-200")
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(unsatisfiable.status(), 416);

    // Download disposition carries the file name.
    let download = http
        .get(format!("{base}/api/v1/vfs/file"))
        .query(&[("path", "/vfs-e2e/hello.txt"), ("download", "true")])
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        download
            .headers()
            .get(reqwest::header::CONTENT_DISPOSITION)
            .unwrap(),
        "attachment; filename=\"hello.txt\""
    );

    // Missing paths are 404 on the per-file endpoints. (`list` keeps
    // the vbash `ls` semantics: an empty page, not an error, because
    // some backends cannot distinguish a missing prefix from an empty
    // directory.)
    for endpoint in ["stat", "text", "file"] {
        let missing = http
            .get(format!("{base}/api/v1/vfs/{endpoint}"))
            .query(&[("path", "/vfs-e2e/missing.txt")])
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), 404, "endpoint {endpoint}");
    }

    gateway_daemon.stop().await;
}

/// Write-side VFS endpoints over the wire: mkdir → upload → conflict
/// save → download byte-parity → move → delete, plus the read-only
/// 403 (via the /plugins/active identity mount when present).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn vfs_write_endpoints_manage_files_and_refuse_read_only_mounts() {
    let gateway_daemon = start_mock_gateway("x").await;
    let base = format!("http://{}", gateway_daemon.addr);
    let http = reqwest::Client::new();
    let token = &gateway_daemon.token;
    let auth = |request: reqwest::RequestBuilder| request.bearer_auth(token);

    // mkdir creates the working directory.
    let made = auth(http.post(format!("{base}/api/v1/vfs/mkdir")))
        .json(&serde_json::json!({ "path": "/vfs-w2e/docs" }))
        .send()
        .await
        .unwrap();
    assert_eq!(made.status(), 200);

    // Binary upload streams raw bytes.
    let uploaded = auth(http.put(format!("{base}/api/v1/vfs/file")))
        .query(&[("path", "/vfs-w2e/docs/blob.bin")])
        .body([0u8, 1, 2, 255, 254, 128, 7].to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(uploaded.status(), 200);
    assert_eq!(
        uploaded.json::<serde_json::Value>().await.unwrap()["size"],
        7
    );

    // Text save creates a file; a second save with a stale original 409s.
    let saved = auth(http.put(format!("{base}/api/v1/vfs/text")))
        .json(&serde_json::json!({
            "path": "/vfs-w2e/docs/notes.md",
            "contents": "v1",
            "original": null,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200);

    let conflicted = auth(http.put(format!("{base}/api/v1/vfs/text")))
        .json(&serde_json::json!({
            "path": "/vfs-w2e/docs/notes.md",
            "contents": "v2-stale",
            "original": "not-what-is-stored",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(conflicted.status(), 409);

    let rewritten = auth(http.put(format!("{base}/api/v1/vfs/text")))
        .json(&serde_json::json!({
            "path": "/vfs-w2e/docs/notes.md",
            "contents": "v2",
            "original": "v1",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(rewritten.status(), 200);

    // Download parity for both the uploaded binary and the saved text.
    let blob = auth(http.get(format!("{base}/api/v1/vfs/file")))
        .query(&[("path", "/vfs-w2e/docs/blob.bin")])
        .send()
        .await
        .unwrap();
    assert_eq!(blob.status(), 200);
    assert_eq!(
        blob.bytes().await.unwrap().to_vec(),
        vec![0u8, 1, 2, 255, 254, 128, 7]
    );
    let text = auth(http.get(format!("{base}/api/v1/vfs/text")))
        .query(&[("path", "/vfs-w2e/docs/notes.md")])
        .send()
        .await
        .unwrap();
    assert_eq!(
        text.json::<serde_json::Value>().await.unwrap()["contents"],
        "v2"
    );

    // move then delete round out the lifecycle.
    let moved = auth(http.post(format!("{base}/api/v1/vfs/move")))
        .json(&serde_json::json!({ "src": "/vfs-w2e/docs/blob.bin", "dst": "/vfs-w2e/docs/blob-moved.bin" }))
        .send()
        .await
        .unwrap();
    assert_eq!(moved.status(), 200);

    // Non-empty directory refuses without recursive.
    let refused = auth(http.delete(format!("{base}/api/v1/vfs/entry")))
        .query(&[("path", "/vfs-w2e/docs")])
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 409);

    let removed = auth(http.delete(format!("{base}/api/v1/vfs/entry")))
        .query(&[("path", "/vfs-w2e/docs"), ("recursive", "true")])
        .send()
        .await
        .unwrap();
    assert_eq!(removed.status(), 200);
    let gone = auth(http.get(format!("{base}/api/v1/vfs/stat")))
        .query(&[("path", "/vfs-w2e/docs")])
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 404);

    // The virtual root is untouchable.
    let root = auth(http.delete(format!("{base}/api/v1/vfs/entry")))
        .query(&[("path", "/"), ("recursive", "true")])
        .send()
        .await
        .unwrap();
    assert_eq!(root.status(), 400);

    // Read-only mounts reject every mutation. The harness manifest
    // always carries a read-only mount (e.g. /plugins/active); if the
    // layout ever changes, fall back to any read_only entry from
    // /vfs/mounts.
    let mounts: serde_json::Value = auth(http.get(format!("{base}/api/v1/vfs/mounts")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let read_only_path = mounts["mounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["read_only"] == true)
        .map(|m| m["path"].as_str().unwrap().to_string());
    if let Some(mount_path) = read_only_path {
        let denied = auth(http.post(format!("{base}/api/v1/vfs/mkdir")))
            .json(&serde_json::json!({ "path": format!("{mount_path}/nope") }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            denied.status(),
            403,
            "write into read-only mount {mount_path}"
        );
        let denied_text = auth(http.put(format!("{base}/api/v1/vfs/text")))
            .json(&serde_json::json!({ "path": format!("{mount_path}/nope.txt"), "contents": "x" }))
            .send()
            .await
            .unwrap();
        assert_eq!(denied_text.status(), 403);
        // Smuggling out of a read-only mount via move is refused too.
        let smuggle = auth(http.post(format!("{base}/api/v1/vfs/move")))
            .json(&serde_json::json!({ "src": mount_path, "dst": "/vfs-w2e/smuggled" }))
            .send()
            .await
            .unwrap();
        assert!(
            smuggle.status() == 403 || smuggle.status() == 404 || smuggle.status() == 400,
            "move out of read-only mount answered {}",
            smuggle.status()
        );
    }

    gateway_daemon.stop().await;
}

/// Plugin dev endpoints over the wire against a real RuntimeHost rsi:
/// scaffolded creation, validation NeedsFix, report 404→200, dev-session
/// status degrading to `unknown` without podman, unknown project 404.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plugin_dev_endpoints_scaffold_validate_and_report() {
    let gateway_daemon = start_mock_gateway("x").await;
    let base = format!("http://{}", gateway_daemon.addr);
    let http = reqwest::Client::new();
    let token = &gateway_daemon.token;
    let auth = |request: reqwest::RequestBuilder| request.bearer_auth(token);

    // Approve an environment directly through the daemon's registry (the
    // HTTP approval path resolves tags against Docker Hub, which the
    // integration suite must not depend on).
    gateway_daemon
        .infra
        .rsi
        .environment_registry()
        .approve(
            "sh-dev",
            plugin_rsi::Environment {
                reference: "docker.io/library/debian@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
                interpreters: vec!["sh".into()],
            },
        )
        .unwrap();

    // Create → scaffolded project with one demo node.
    let created = auth(http.post(format!("{base}/api/v1/plugin-projects")))
        .json(&serde_json::json!({
            "name": "wire-plugin",
            "environment_id": "sh-dev",
            "purpose": "wire scaffold test",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 200);
    let view: serde_json::Value = created.json().await.unwrap();
    assert!(view["image"].as_str().unwrap().contains("@sha256:"));

    let files = auth(http.get(format!("{base}/api/v1/plugin-projects/wire-plugin/files")))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    let names = serde_json::to_string(&files).unwrap();
    assert!(names.contains("demo.sh"), "scaffold script: {names}");
    assert!(names.contains("README.md"), "scaffold readme: {names}");

    // No report yet → 404; after a validation run → 200 with gates.
    let missing = auth(http.get(format!("{base}/api/v1/plugin-projects/wire-plugin/report")))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);

    // Validation of the untouched scaffold passes the six gates.
    let validated = auth(http.post(format!(
        "{base}/api/v1/plugin-projects/wire-plugin/validate"
    )))
    .send()
    .await
    .unwrap();
    assert_eq!(validated.status(), 200);
    let outcome: serde_json::Value = validated.json().await.unwrap();
    assert_eq!(
        outcome["outcome"], "passed",
        "scaffold must validate: {outcome}"
    );

    let report = auth(http.get(format!("{base}/api/v1/plugin-projects/wire-plugin/report")))
        .send()
        .await
        .unwrap();
    assert_eq!(report.status(), 200);
    let report: serde_json::Value = report.json().await.unwrap();
    assert!(report["gates"].as_array().unwrap().len() >= 6);

    // Activation needs a clean git tree; the scaffold is committed, so it
    // succeeds end to end and the node is registered.
    let activated = auth(http.post(format!(
        "{base}/api/v1/plugin-projects/wire-plugin/activate"
    )))
    .send()
    .await
    .unwrap();
    assert_eq!(activated.status(), 200);
    let activation: serde_json::Value = activated.json().await.unwrap();
    assert_eq!(
        activation["activated"], true,
        "scaffold must activate: {activation}"
    );

    let plugins = auth(http.get(format!("{base}/api/v1/plugins")))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    let wire = plugins["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|plugin| plugin["name"] == "wire-plugin")
        .expect("activated plugin in /plugins");
    assert!(wire["registered"].as_bool().unwrap(), "hot reload: {wire}");
    assert!(wire["kinds"].to_string().contains("wire_plugin"));

    // Dev-session status without a usable podman degrades to `unknown`
    // (still 200, detail explains why) instead of erroring.
    let session = auth(http.get(format!(
        "{base}/api/v1/plugin-projects/wire-plugin/dev-session"
    )))
    .send()
    .await
    .unwrap();
    assert_eq!(session.status(), 200);
    let session: serde_json::Value = session.json().await.unwrap();
    assert!(
        session["state"] == "absent"
            || session["state"] == "unknown"
            || session["state"] == "running",
        "dev-session probe must degrade, got: {session}"
    );

    // Unknown projects are 404 across the new surface.
    for path in [
        "dev-session",
        "validate",
        "activate",
        "report",
        "nodes/x/test-run",
    ] {
        // Valid bodies per endpoint so the 404 comes from the project
        // lookup, not from body validation.
        let body = match path {
            "dev-session" => r#"{"action":"start"}"#,
            "nodes/x/test-run" => r#"{"inputs":{},"params":{}}"#,
            _ => "{}",
        };
        let request = if path == "report" {
            auth(http.get(format!(
                "{base}/api/v1/plugin-projects/missing-plugin/{path}"
            )))
        } else {
            auth(http.post(format!(
                "{base}/api/v1/plugin-projects/missing-plugin/{path}"
            )))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
        };
        let missing = request.send().await.unwrap();
        assert_eq!(missing.status(), 404, "path {path}");
    }

    gateway_daemon.stop().await;
}

/// The dev terminal WebSocket is guarded like every other /api/v1
/// route: an unauthenticated handshake is refused before the upgrade.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plugin_terminal_websocket_requires_authentication() {
    let gateway_daemon = start_mock_gateway("x").await;
    let base = format!("http://{}", gateway_daemon.addr);

    // A plain GET without credentials answers 401 — no upgrade.
    let unauthenticated = reqwest::Client::new()
        .get(format!("{base}/api/v1/plugin-projects/any/terminal"))
        .header(reqwest::header::CONNECTION, "Upgrade")
        .header(reqwest::header::UPGRADE, "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), 401);

    // Unknown projects are refused with 404 even with credentials (the
    // handshake is checked before the exec session would start).
    let missing = reqwest::Client::new()
        .get(format!(
            "{base}/api/v1/plugin-projects/missing-plugin/terminal"
        ))
        .header(reqwest::header::CONNECTION, "Upgrade")
        .header(reqwest::header::UPGRADE, "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .bearer_auth(&gateway_daemon.token)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);

    gateway_daemon.stop().await;
}
