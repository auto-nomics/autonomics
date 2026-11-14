//! Production Gateway + RuntimeHost + SDK tool loop, with scripted model decisions.
//! This proves transport, registration, vision attachment and revision/review
//! consistency; it does not evaluate scientific reasoning or visual selection.
use agentik_types::messages::ContentBlock;
use gateway::{
    client::{EventPump, FrameInner, GatewayFrame},
    testing::{TEST_TIMEOUT, scripted_figure_model, start_mock_gateway_with_model},
};
use serde_json::{Value, json};
async fn api(
    gw: &gateway::testing::TestGateway,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Value {
    let mut request = reqwest::Client::new()
        .request(
            method,
            format!("http://{}/api/v1/visualization{path}", gw.addr),
        )
        .bearer_auth(&gw.token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    request
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}
async fn turn(
    client: &gateway::GatewayClient,
    agent: &str,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<GatewayFrame>,
    handoff: Value,
) -> usize {
    client
        .deliver_message(
            agent,
            format!(
                "Canvas handoff\n{}",
                serde_json::to_string_pretty(&handoff).unwrap()
            ),
        )
        .await
        .unwrap();
    tokio::time::timeout(TEST_TIMEOUT, async {
        let mut calls = 0;
        loop {
            if let GatewayFrame::Sequenced {
                inner: FrameInner::Agent { agent: path, event },
                ..
            } = rx.recv().await.expect("gateway event pump remains open")
            {
                if path != agent {
                    continue;
                }
                match event {
                    agentik_types::AgentEvent::ToolCall { .. } => calls += 1,
                    agentik_types::AgentEvent::ToolResult { ok, content } => {
                        assert!(ok, "{content}")
                    }
                    agentik_types::AgentEvent::TurnCompleted { status, .. } => {
                        assert_eq!(
                            status,
                            agentik_types::agent_events::TurnExecutionStatus::Completed
                        );
                        return calls;
                    }
                    _ => {}
                }
            }
        }
    })
    .await
    .expect("production turn completed over SSE")
}
const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scientific_figure_conversation_directly_edits_canvas_through_gateway() {
    use reqwest::Method;
    let (model, trace) = scripted_figure_model();
    let gw = start_mock_gateway_with_model(model).await;
    let client = gw.client();
    api(
        &gw,
        Method::POST,
        "/projects",
        Some(json!({"id":"study","title":"Study"})),
    )
    .await;
    let figure=api(&gw,Method::POST,"/projects/study/figures",Some(json!({"title":"Figure","format":"excalidraw/v2","document":{"elements":[],"files":{}}}))).await;
    let path = format!("/projects/study/figures/{}", figure["id"].as_str().unwrap());
    let state = client.state().await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    EventPump::spawn(client.clone(), state.last_seq, tx);
    let agent = client
        .spawn_agent("figure", "/root", "illustrator", None, None)
        .await
        .unwrap();
    let h = |request: &str, revision: i64| json!({"project_id":"study","figure_id":figure["id"],"observed_revision":revision,"request":request});
    assert_eq!(
        turn(
            &client,
            &agent,
            &mut rx,
            h("Help draft a figure; please clarify its structure.", 1)
        )
        .await,
        2
    );
    assert_eq!(
        api(&gw, Method::GET, &path, None).await["metadata"]["revision"],
        1
    );
    let answer = "Show generic A, B and C, with explicit A to B and B to C relations.";
    assert_eq!(turn(&client, &agent, &mut rx, h(answer, 1)).await, 3);
    let before = api(&gw, Method::GET, &path, None).await;
    api(
        &gw,
        Method::PUT,
        &format!("{path}/revisions/2/preview"),
        Some(json!({"dataURL":PNG})),
    )
    .await;
    assert_eq!(
        turn(
            &client,
            &agent,
            &mut rx,
            h("Rename B only; preserve A, C and their geometry.", 2)
        )
        .await,
        3
    );
    let edited = api(&gw, Method::GET, &path, None).await;
    assert_eq!(edited["metadata"]["revision"], 3);
    for element in before["scene"]["document"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["id"] != "label-1")
    {
        assert_eq!(
            edited["scene"]["document"]["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == element["id"]),
            Some(element)
        );
    }
    let current = api(&gw, Method::GET, &path, None).await;
    assert_eq!(
        current["scene"]["document"]["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "label-1")
            .unwrap()["text"],
        "B revised"
    );
    let mark=api(&gw,Method::POST,&format!("{path}/markups"),Some(json!({"expected_revision":3,"note":"Remove B and its two explicit relationships; do not invent A to C.","strokes":[{"kind":"cross","points":[[0,0],[20,20]]}],"dataURL":PNG}))).await;
    let mut request = h(
        "Apply the saved review mark and preserve unrelated components.",
        3,
    );
    request["markups"] = json!([mark]);
    assert_eq!(turn(&client, &agent, &mut rx, request).await, 3);
    let final_scene = api(&gw, Method::GET, &path, None).await;
    let elements = final_scene["scene"]["document"]["elements"]
        .as_array()
        .unwrap();
    assert_eq!(elements.len(), 4);
    assert!(!elements.iter().any(|e| e["type"] == "arrow"));
    assert_eq!(
        api(&gw, Method::GET, &format!("{path}/markups"), None).await[0]["status"],
        "open"
    );
    assert_eq!(
        api(&gw, Method::GET, &format!("{path}/revisions/3"), None).await["document"],
        current["scene"]["document"]
    );
    let snapshot = client.state().await.unwrap();
    let identity = snapshot
        .agents
        .iter()
        .find(|a| a.path == agent)
        .unwrap()
        .agent_id
        .unwrap();
    let context = snapshot
        .contexts
        .get(&agent)
        .expect("gateway hydrated the actor session")
        .id;
    let history = client.session_history(identity, context).await.unwrap();
    assert_eq!(
        history
            .iter()
            .flat_map(|m| &m.content)
            .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
            .count(),
        11
    );
    assert_eq!(
        history
            .iter()
            .flat_map(|m| &m.content)
            .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
            .count(),
        11
    );
    assert_eq!(
        history
            .iter()
            .flat_map(|m| &m.content)
            .filter(
                |b| matches!(b,ContentBlock::Text{text} if text.starts_with("Canvas handoff\n"))
            )
            .count(),
        4
    );
    {
        let trace = trace.lock().unwrap();
        assert_eq!(trace.step, 15);
        assert_eq!(trace.vision_results, 2);
        assert_eq!(trace.actions.len(), 11);
    }
    gw.stop().await;
}
