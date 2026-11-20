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

/// Resources face: write a file through the REST VFS endpoint, discover it
/// via MCP `resources/list`, read it back via `resources/read`.
#[tokio::test]
async fn mcp_resources_list_and_read_vfs() {
    let server = start_mock_gateway("unused").await;
    let client = reqwest::Client::new();
    let base = format!("http://{}/mcp", server.addr);

    // Seed a file through the daemon's own write channel.
    reqwest::Client::new()
        .put(format!("http://{}/api/v1/vfs/text", server.addr))
        .bearer_auth(&server.token)
        .json(&json!({"path": "/mcp-e2e-note.md", "contents": "# hello\n"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // Initialize an MCP session.
    let response = client
        .post(&base)
        .bearer_auth(&server.token)
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
        .unwrap()
        .error_for_status()
        .unwrap();
    let session_id = response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("session id")
        .to_string();

    let request = |body: serde_json::Value| {
        let client = client.clone();
        let url = base.clone();
        let token = server.token.clone();
        let session = session_id.clone();
        async move {
            client
                .post(&url)
                .bearer_auth(&token)
                .header("Accept", "application/json, text/event-stream")
                .header("mcp-session-id", &session)
                .json(&body)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .text()
                .await
                .unwrap()
        }
    };

    // resources/list: root page contains the seeded file.
    let body = request(json!({"jsonrpc": "2.0", "id": 2, "method": "resources/list"})).await;
    assert!(
        body.contains("local:///mcp-e2e-note.md"),
        "listing missing the file: {body}"
    );

    // resources/read: by canonical local:// URI.
    let body = request(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "resources/read",
        "params": {"uri": "local:///mcp-e2e-note.md"}
    }))
    .await;
    assert!(body.contains("# hello"), "read missing content: {body}");
    assert!(
        body.contains("text/markdown"),
        "read missing mime type: {body}"
    );

    // resources/read: unknown path surfaces as a JSON-RPC resource-not-found
    // error (HTTP 200 — protocol errors ride in the response body).
    let body = request(json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "resources/read",
        "params": {"uri": "local:///definitely-missing.txt"}
    }))
    .await;
    assert!(
        body.contains("definitely-missing"),
        "error should name the missing resource: {body}"
    );
    server.stop().await;
}
