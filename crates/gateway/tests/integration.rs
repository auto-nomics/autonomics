//! End-to-end tests for the gateway daemon: a real axum server on an
//! ephemeral port, a mock model, and a real `GatewayClient` + SSE pump.
//! No network beyond loopback, no API keys — the same reliability
//! keystone the headless tests established, now over the wire.

use std::time::Duration;

use agentik_core::memory::{
    Knowledge, KnowledgeEvidenceInput, KnowledgePath, KnowledgeSkillProposalInput, MemoryStore,
};
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
async fn spawn_chat_turn_and_shutdown_over_the_wire() {
    let gateway_daemon = start_mock_gateway("The answer is 2.").await;
    let client = gateway_daemon.client();

    // Hydration first.
    let state = client.state().await.unwrap();
    assert!(state.agents.is_empty());
    // researcher + developer + browser (AgentKind::ALL).
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
        ..
    } = registered
    else {
        unreachable!()
    };

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

    // /state now lists the live agent, and the driver's session cache has
    // the agent's sessions (requested at registration, refreshed when the
    // turn started on a session the cache hadn't seen).
    let refreshed = tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            let snapshot = client.state().await.unwrap();
            if snapshot.agents.iter().any(|a| a.path == path)
                && snapshot
                    .sessions
                    .get(&path)
                    .is_some_and(|list| !list.is_empty())
            {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("agent + sessions visible in /state");
    assert!(!refreshed.sessions[&path].is_empty());

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
    client.shutdown_gateway().await.unwrap();
    gateway_daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kms_worker_snapshot_exposes_resident_workers() {
    let gateway_daemon = start_mock_gateway("ok").await;
    let workers = gateway_daemon.client().kms_worker_statuses().await.unwrap();
    let mut names = workers
        .into_iter()
        .map(|worker| worker.worker)
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        vec![
            "consolidate_knowledge".to_string(),
            "diagnose_knowledge_node".to_string(),
            "ingest_observation".to_string(),
            "propose_skill_revision".to_string(),
            "repair_knowledge_node".to_string(),
            "session_extract".to_string(),
        ]
    );
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
    assert_eq!(openapi["paths"].as_object().unwrap().len(), 46);
    let expected_paths = [
        "/api/v1/gateway/status",
        "/api/v1/gateway/shutdown",
        "/api/v1/state",
        "/api/v1/agents",
        "/api/v1/agents/{name}/messages",
        "/api/v1/agents/{name}/cancel",
        "/api/v1/agents/{name}/compact",
        "/api/v1/agents/{name}/shutdown",
        "/api/v1/agents/{name}/model",
        "/api/v1/agents/{name}/config",
        "/api/v1/agents/{name}/dag",
        "/api/v1/agents/{name}/sessions",
        "/api/v1/agents/{name}/sessions/{id}/activate",
        "/api/v1/agents/{name}/sessions/{id}/close",
        "/api/v1/agents/{name}/sessions/{id}/title",
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
        "/api/v1/plugins/environments",
        "/api/v1/plugins/environments/dockerhub",
        "/api/v1/plugins/environments/dockerhub/tag",
        "/api/v1/plugins/environments/approve",
        "/api/v1/plugins/environments/{id}",
        "/api/v1/events",
        "/api/v1/kms/evidence",
        "/api/v1/kms/knowledge",
        "/api/v1/kms/skills",
        "/api/v1/kms/skills/{name}",
        "/api/v1/kms/skill-proposals",
        "/api/v1/kms/skill-proposals/{id}/approve",
        "/api/v1/kms/skill-proposals/{id}/reject",
        "/api/v1/skills/library",
        "/api/v1/skills/library/{name}",
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

#[tokio::test]
async fn skill_library_lists_installed_and_proposed_with_detail() {
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

    // Retain the old filesystem proposal view as a compatibility read, but
    // distillation alone must leave the proposal pending rather than install.
    for body in ["fix a", "fix b", "fix c"] {
        manager
            .observation_hub()
            .record(evolution_core::ObservationInput {
                kind: evolution_core::ObservationKind::Failure,
                source: evolution_core::ObservationSource::Agent,
                summary: "lib boom".into(),
                body: body.into(),
                node_kind: Some("sql".into()),
                error: Some("lib error 7".into()),
            })
            .await
            .unwrap();
    }
    let report = manager.distill().await.unwrap();
    assert_eq!(report.proposals_written.len(), 1, "{report:?}");

    // Detail of the proposed row: body + its evidence chain remain visible.
    let name = client
        .skill_library()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.tier == "proposed" && s.proposal_status.as_deref() == Some("pending"))
        .unwrap()
        .name;
    let detail = client.skill_library_detail(&name).await.unwrap();
    assert!(!detail.installed);
    assert_eq!(detail.tier, "proposed");
    assert!(!detail.body.is_empty());
    let proposal = detail
        .proposal
        .expect("installed row carries its change record");
    assert_eq!(proposal.status, "pending");
    assert_eq!(proposal.observation_count, 3, "evidence chain size");
    assert_eq!(detail.evidence_count, 3);

    // Unknown names 404 rather than fabricating a row.
    let missing = client.skill_library_detail("no-such-skill").await;
    assert!(missing.is_err());
}

#[tokio::test]
async fn kms_skill_proposals_support_human_approval_and_rejection() {
    let gw = start_mock_gateway("ok").await;
    let client = gw.client();
    let store = gw.infra.kms.store();
    let evidence = store
        .upsert_evidence(KnowledgeEvidenceInput {
            source_kind: "skill_failure".to_string(),
            source_id: "failure-1".to_string(),
            evidence_kind: "eval_failure".to_string(),
            summary: "eval case smoke fails".to_string(),
            body: "The workflow produced the wrong row count.".to_string(),
            anchor_path: Some("/procedures/approve-flow".to_string()),
            error: Some("rows_min mismatch".to_string()),
        })
        .await
        .unwrap();
    let input = |name: &str, body: &str| KnowledgeSkillProposalInput {
        skill_name: name.to_string(),
        title: name.replace('-', " "),
        body_md: body.to_string(),
        metadata_json: serde_json::json!({
            "description": body,
            "tags": ["kms"]
        }),
        node_path: format!("/procedures/{name}"),
        rationale: "Repeated failures require review.".to_string(),
        source_evidence_ids: vec!["evidence-1".to_string()],
        artifacts: vec![agentik_core::memory::KnowledgeSkillArtifactInput {
            relative_path: "workflow/main.toml".to_string(),
            content: "description = 'reviewed'\n".to_string(),
        }],
    };
    let approve = store
        .create_skill_proposal(input("approve-flow", "Approve this revision."))
        .await
        .unwrap()
        .unwrap();
    let reject = store
        .create_skill_proposal(input("reject-flow", "Reject this revision."))
        .await
        .unwrap()
        .unwrap();

    let rows = client.kms_skill_proposals().await.unwrap();
    let evidence_rows = client.kms_evidence().await.unwrap();
    let evidence_row = evidence_rows
        .iter()
        .find(|row| row.id == evidence.id)
        .unwrap();
    assert_eq!(evidence_row.status, "pending");
    assert_eq!(evidence_row.evidence_kind, "eval_failure");
    assert_eq!(
        evidence_row.anchor_path.as_deref(),
        Some("/procedures/approve-flow")
    );
    let approve_row = rows.iter().find(|row| row.id == approve.id).unwrap();
    assert_eq!(approve_row.status, "pending");
    assert_eq!(approve_row.skill_name, "approve-flow");
    assert!(approve_row.body_md.contains("Approve this revision."));
    assert_eq!(approve_row.artifacts[0].relative_path, "workflow/main.toml");

    let approved = client.approve_kms_skill_proposal(approve.id).await.unwrap();
    assert_eq!(approved.status, "approved");
    assert!(approved.skill_version_id.is_some());
    let rejected = client.reject_kms_skill_proposal(reject.id).await.unwrap();
    assert_eq!(rejected.status, "rejected");
    assert!(rejected.skill_version_id.is_none());

    let rows = client.kms_skill_proposals().await.unwrap();
    assert!(
        rows.iter()
            .any(|row| row.id == approve.id && row.status == "approved")
    );
    assert!(
        rows.iter()
            .any(|row| row.id == reject.id && row.status == "rejected")
    );
    assert!(
        client
            .reject_kms_skill_proposal(reject.id)
            .await
            .unwrap_err()
            .status()
            == Some(409)
    );

    let versions = store.list_skill_versions(100).await.unwrap();
    assert!(
        versions
            .iter()
            .any(|version| version.skill_name == "approve-flow" && version.status == "active")
    );
    assert!(
        !versions
            .iter()
            .any(|version| version.skill_name == "reject-flow")
    );

    let skills = client.kms_skills().await.unwrap();
    let listed = skills
        .iter()
        .find(|skill| skill.skill_name == "approve-flow")
        .unwrap();
    assert_eq!(listed.version, 1);
    assert_eq!(listed.artifact_paths, vec!["workflow/main.toml"]);
    let detail = client.kms_skill_detail("approve-flow").await.unwrap();
    assert_eq!(detail.id, approved.skill_version_id.unwrap());
    assert!(detail.body_md.contains("Approve this revision."));
    assert_eq!(detail.artifacts[0].content, "description = 'reviewed'");
    assert_eq!(detail.history.len(), 1);
    assert!(client.kms_skill_detail("no-such-kms-skill").await.is_err());

    gw.stop().await;
}

#[tokio::test]
async fn kms_knowledge_lists_the_durable_wiki_tree() {
    let gw = start_mock_gateway("ok").await;
    let client = gw.client();
    let store = gw.infra.kms.store();
    let path =
        KnowledgePath::normalize("/engineering/testing/release-checks").expect("valid wiki path");
    let created = store
        .create_knowledge(Knowledge::leaf(
            path,
            "Release checks".to_string(),
            "Run smoke tests before release.".to_string(),
            0.92,
            "source-hash",
            1_700_000_000,
        ))
        .await
        .unwrap();

    let rows = client.kms_knowledge().await.unwrap();
    let article = rows
        .iter()
        .find(|row| row.id == created.id)
        .expect("article is listed");
    assert_eq!(article.path, "/engineering/testing/release-checks");
    assert_eq!(article.title, "Release checks");
    assert_eq!(article.status, "active");
    assert!(!article.structural);
    assert!(article.body_md.contains("smoke tests"));
    assert!(article.parent_id.is_some());
    assert!(
        rows.iter()
            .any(|row| row.path == "/engineering" && row.structural)
    );
    assert!(
        rows.iter()
            .any(|row| row.path == "/engineering/testing" && row.structural)
    );

    gw.stop().await;
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
