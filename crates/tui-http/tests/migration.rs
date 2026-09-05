//! P4 迁移链路的端到端验证（真服务）。
//!
//! 与 `agent_runtime.rs` 里的 oneshot 测试不同，这里跑的是**真实 TCP 栈**：
//! `tui_http::start` 起真服务、真 RuntimeHost（tempdir agent.db）、真
//! `AnthropicApiClient`——模型槽指向本地假 Anthropic provider（真 SSE 线格式
//! 字节），由生产 eventsource 解析链消费。全程不触用户真实 config.db /
//! bib.db / API key。
//!
//! 按 P4 前端的真实调用序驱动（useChatInit / useChatSender）：
//! bib 旧 blob 落库 → 模式探测 → /threads/import → /messages 回读 →
//! chat turn（context 字段）→ WAL 落库后转写完整 → bib blob 原样保留。
#![cfg(feature = "runtime-host")]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentik_sdk::Anthropic;
use agentik_sdk::model::Model;
use agentik_sdk::provider::client::AnthropicApiClient;
use arc_swap::ArcSwapOption;
use bib_base::BibShared;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 假 provider 的脚本回复——断言它会出现在 SSE 流与转写里。
const SCRIPTED_REPLY: &str = "数据集规模约 4 万条蛋白质结构。";

// ---------------------------------------------------------------------------
// 假 Anthropic provider：标准 Anthropic SSE 线格式（与 SDK 测试夹具同构）
// ---------------------------------------------------------------------------

/// 拼 `message_start → content_block_start → delta → stop → message_delta
/// (end_turn) → message_stop` 的完整 SSE 帧。delta 里的文本经 serde_json
/// 转义，避免手写 JSON 转义出错。
fn sse_reply(reply: &str) -> String {
    let frame = |event: &str, data: serde_json::Value| {
        format!("event: {event}\ndata: {}\n\n", data.to_string())
    };
    let mut body = String::new();
    body.push_str(&frame(
        "message_start",
        serde_json::json!({
            "type": "message_start",
            "message": {
                "id": "msg_e2e_1", "type": "message", "role": "assistant",
                "model": "e2e-model", "content": [], "stop_reason": null,
                "stop_sequence": null,
                "usage": {"input_tokens": 12, "output_tokens": 1},
            },
        }),
    ));
    body.push_str(&frame(
        "content_block_start",
        serde_json::json!({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": ""},
        }),
    ));
    body.push_str(&frame(
        "content_block_delta",
        serde_json::json!({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": reply},
        }),
    ));
    body.push_str(&frame(
        "content_block_stop",
        serde_json::json!({"type": "content_block_stop", "index": 0}),
    ));
    body.push_str(&frame(
        "message_delta",
        serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn", "stop_sequence": null},
            "usage": {"output_tokens": 6},
        }),
    ));
    body.push_str(&frame(
        "message_stop",
        serde_json::json!({"type": "message_stop"}),
    ));
    body
}

/// 起 `POST /v1/messages` 假服务：读掉请求（记录净荷供断言），回 canned SSE。
async fn spawn_fake_anthropic(reply: &str, seen: Arc<Mutex<Vec<String>>>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let sse = sse_reply(reply);
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let sse = sse.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                // 读完整请求（头 + content-length 的净荷），供事后断言
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    match socket.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if let Some(head_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                                let len = head
                                    .lines()
                                    .find_map(|line| {
                                        let (key, value) = line.split_once(':')?;
                                        key.eq_ignore_ascii_case("content-length")
                                            .then(|| value.trim().parse::<usize>().ok())?
                                    })
                                    .unwrap_or(0);
                                if buf.len() >= head_end + 4 + len {
                                    break;
                                }
                            }
                        }
                    }
                }
                seen.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf).into_owned());
                let response = format!(
                    "HTTP/1.1 200 OK\r\n\
                     content-type: text/event-stream\r\n\
                     connection: close\r\n\
                     \r\n{sse}"
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    addr
}

// ---------------------------------------------------------------------------
// 原始 HTTP 客户端（含 chunked 解码——SSE 端点走 transfer-encoding: chunked）
// ---------------------------------------------------------------------------

/// 去掉 chunked 分帧，得到净荷字节。
fn dechunk(mut input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let Some(pos) = input.windows(2).position(|w| w == b"\r\n") else {
            break;
        };
        let Ok(size) =
            usize::from_str_radix(std::str::from_utf8(&input[..pos]).unwrap_or("0").trim(), 16)
        else {
            break;
        };
        input = &input[pos + 2..];
        if size == 0 {
            break;
        }
        let end = size.min(input.len());
        out.extend_from_slice(&input[..end]);
        input = &input[end..];
        if input.len() >= 2 {
            input = &input[2..]; // 块尾 CRLF
        }
    }
    out
}

/// 发一次性 HTTP/1.1 请求（Connection: close），读尽响应，返回 (状态码, 净荷)。
async fn http(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let connect = tokio::time::timeout(Duration::from_secs(30), TcpStream::connect(addr));
    let mut stream = connect.await.expect("connect timeout").expect("connect");
    let body_bytes = body.map(str::as_bytes).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\
         content-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body_bytes.len(),
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.write_all(body_bytes).await.unwrap();

    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(30), stream.read_to_end(&mut raw))
        .await
        .expect("response timeout")
        .unwrap();
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("http head terminator");
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .expect("status line")
        .parse()
        .unwrap();
    let rest = &raw[head_end + 4..];
    let payload = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        dechunk(rest)
    } else {
        rest.to_vec()
    };
    (status, String::from_utf8_lossy(&payload).into_owned())
}

async fn get_messages(addr: SocketAddr, thread_id: &str) -> Vec<(String, String)> {
    let (_, body) = http(
        addr,
        "GET",
        &format!("/api/v1/agent/threads/{thread_id}/messages"),
        None,
    )
    .await;
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    parsed["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|message| {
            (
                message["role"].as_str().unwrap_or_default().to_owned(),
                message["text"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 迁移链路
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_blob_migrates_into_runtime_thread_end_to_end() {
    // ── 假 provider：真 Anthropic SSE 线格式，走生产 AnthropicApiClient 解析 ──
    let provider_requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let provider_addr = spawn_fake_anthropic(SCRIPTED_REPLY, provider_requests.clone()).await;

    // ── 真 RuntimeHost（tempdir agent.db）+ 真模型槽 + 真 TCP 服务 ──
    // builder 构造让全部派生路径（agent.db 等）落 tempdir：`Default` 会
    // 预先从 $HOME/env 解析，事后只改 state_dir 字段不会重派生 agent_db。
    let dir = tempfile::tempdir().unwrap();
    let config = runtime::RuntimeConfig::builder()
        .data_dir(dir.path().join("data"))
        .state_dir(dir.path().join("state"))
        .build();
    let mut host = runtime::RuntimeHost::open(&config).await.unwrap();

    let anthropic = Anthropic::new(
        "e2e-test-key",
        format!("http://127.0.0.1:{}", provider_addr.port()),
    )
    .unwrap();
    let model: Arc<ArcSwapOption<Model>> =
        Arc::new(ArcSwapOption::from_pointee(Model::with_client(
            agentik_core::testing::dummy_model_info("e2e-model"),
            AnthropicApiClient::new(anthropic),
        )));

    let shared = BibShared::open_in_memory().await.unwrap();
    let router = tui_http::ApiRouterBuilder::new(shared)
        .model(model)
        .host(host.infra())
        .build();
    let server = tui_http::start(router, "127.0.0.1:0").await.unwrap();
    let addr = server.addr();

    // ① 旧世界：前端 persistMessages 把整包 blob 写进 bib_meta
    let (status, body) = http(
        addr,
        "POST",
        "/api/v1/bib/chat",
        Some(
            r#"{
                "scope": "standalone",
                "payload": {
                    "conversations": [{
                        "id": "conv-1",
                        "title": "旧对话",
                        "created_at": "2026-09-01T10:00:00Z",
                        "updated_at": "2026-09-01T10:05:00Z",
                        "messages": [
                            {"role": "user", "content": "这篇论文的方法是什么？", "id": "m1"},
                            {"role": "assistant", "content": "方法是基于对比学习的蛋白质结构预测。", "id": "m2"}
                        ]
                    }],
                    "active_conversation_id": "conv-1"
                }
            }"#,
        ),
    )
    .await;
    assert_eq!(status, 200, "seed legacy bib blob: {body}");

    // ② 前端读回旧历史（useChatInit 的水合源）
    let (status, body) = http(addr, "GET", "/api/v1/bib/chat?scope=standalone", None).await;
    assert_eq!(status, 200, "read legacy blob: {body}");
    assert!(
        body.contains("这篇论文的方法是什么？"),
        "旧历史可读: {body}"
    );

    // ③ 模式探测：新前端据此选 runtime 链路
    let (status, body) = http(addr, "GET", "/api/v1/agent", None).await;
    assert_eq!(status, 200, "probe: {body}");
    assert!(
        body.contains("\"mode\":\"runtime\""),
        "probe → runtime: {body}"
    );

    // ④ P4 迁移：useChatInit 预导入（toImportMessages 归一后的载荷形状）
    let (status, body) = http(
        addr,
        "POST",
        "/api/v1/agent/threads/import",
        Some(
            r#"{
                "agent_type": "homepage",
                "title": "旧对话",
                "messages": [
                    {"role": "user", "content": "这篇论文的方法是什么？"},
                    {"role": "assistant", "content": "方法是基于对比学习的蛋白质结构预测。"}
                ]
            }"#,
        ),
    )
    .await;
    assert_eq!(status, 200, "import legacy transcript: {body}");
    let thread_id = serde_json::from_str::<serde_json::Value>(&body).unwrap()["thread_id"]
        .as_str()
        .expect("thread_id")
        .to_owned();

    // ⑤ 迁移即持久化：import 响应返回前旧消息已可读（durable before response）
    let messages = get_messages(addr, &thread_id).await;
    assert_eq!(
        messages.len(),
        2,
        "imported transcript readable: {messages:?}"
    );
    assert_eq!(
        messages[0],
        ("user".into(), "这篇论文的方法是什么？".into())
    );
    assert_eq!(
        messages[1],
        (
            "assistant".into(),
            "方法是基于对比学习的蛋白质结构预测。".into()
        )
    );

    // ⑥ 追问一 turn：动态上下文走每轮 context 字段
    let (status, body) = http(
        addr,
        "POST",
        &format!("/api/v1/agent/threads/{thread_id}/chat"),
        Some(
            r#"{
                "message": "那它的数据集规模多大？",
                "agent_type": "homepage",
                "context": "【当前文献】Test Paper"
            }"#,
        ),
    )
    .await;
    assert_eq!(status, 200, "chat turn: {body}");
    assert!(body.contains("event: text_delta"), "SSE 文本帧: {body}");
    assert!(
        body.contains(SCRIPTED_REPLY),
        "脚本回复经生产解析链落地: {body}"
    );
    assert!(body.contains("event: done"), "done 帧: {body}");

    // ⑦ WAL 异步落库：转写 = 迁移的 2 条 + 本轮 user/assistant
    let messages = {
        let mut messages = Vec::new();
        for _ in 0..20 {
            messages = get_messages(addr, &thread_id).await;
            if messages.len() == 4 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        messages
    };
    assert_eq!(
        messages.len(),
        4,
        "full transcript after turn: {messages:?}"
    );
    // 迁移前缀原样
    assert_eq!(
        messages[0],
        ("user".into(), "这篇论文的方法是什么？".into())
    );
    assert_eq!(
        messages[1],
        (
            "assistant".into(),
            "方法是基于对比学习的蛋白质结构预测。".into()
        )
    );
    // 本轮 user 消息 = 【当前上下文】前导 + context + 问题
    assert_eq!(messages[2].0, "user");
    assert!(
        messages[2]
            .1
            .starts_with("【当前上下文】\n【当前文献】Test Paper\n\n"),
        "context 前导渲染: {:?}",
        messages[2].1
    );
    assert!(messages[2].1.ends_with("那它的数据集规模多大？"));
    // 本轮 assistant = 脚本回复全文
    assert_eq!(messages[3], ("assistant".into(), SCRIPTED_REPLY.into()));

    // ⑧ 强证据：agent 上行给 provider 的请求带着迁移来的旧对话 + 本轮输入
    //（session 上下文真正流到了模型，而不是只有 UI 在动）
    let provider_requests = provider_requests.lock().unwrap().clone();
    assert_eq!(
        provider_requests.len(),
        1,
        "单次模型调用: {provider_requests:#?}"
    );
    assert!(
        provider_requests[0].contains("这篇论文的方法是什么？")
            && provider_requests[0].contains("那它的数据集规模多大？"),
        "旧对话与新输入都进入了模型上下文"
    );

    // ⑨ bib blob 未被迁移破坏（UI 结构锚点 + legacy 降级源）
    let (status, body) = http(addr, "GET", "/api/v1/bib/chat?scope=standalone", None).await;
    assert_eq!(status, 200);
    assert!(
        body.contains("conversations") && body.contains("这篇论文的方法是什么？"),
        "legacy blob 保留: {body}"
    );

    server.shutdown().await.unwrap();
    host.shutdown_all_agents_and_wait().await;
}
