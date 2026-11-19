//! HTTP-level contract for the daemon's MCP endpoint (`/mcp`):
//! bearer-token gating, `initialize` handshake, and `tools/list`
//! exposing the data-engine tool face.
use gateway::testing::start_mock_gateway;
use serde_json::json;

#[tokio::test]
async fn mcp_endpoint_rejects_missing_token() {
    let server = start_mock_gateway("unused").await;
    let response = reqwest::Client::new()
        .post(format!("http://{}/mcp", server.addr))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    server.stop().await;
}

#[tokio::test]
async fn mcp_initialize_then_list_tools() {
    let server = start_mock_gateway("unused").await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/mcp", server.addr);

    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}
        }
    });

    let response = client
        .post(&url)
        .bearer_auth(&server.token)
        .header("Accept", "application/json, text/event-stream")
        .json(&init)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{:?}", response.status());
    // Depending on negotiated defaults the reply is JSON or an SSE frame;
    // both carry the initialize result with the server identity.
    let session_id = response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = response.text().await.unwrap();
    assert!(body.contains("autonomics"), "body: {body}");

    // A session was established for the pre-2026-07-28 protocol version.
    let session_id = session_id.expect("initialize must return Mcp-Session-Id");

    let response = client
        .post(&url)
        .bearer_auth(&server.token)
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{:?}", response.status());
    let body = response.text().await.unwrap();
    for expected in ["run_flow", "dag_shell", "get_node_spec"] {
        assert!(body.contains(expected), "missing {expected} in: {body}");
    }
    server.stop().await;
}
