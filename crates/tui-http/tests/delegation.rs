//! P5b 受限 delegation 的端到端验证（docs/design/web-agent-delegation.md §7）。
//!
//! 与 `migration.rs` 同一套路：真 TCP 栈 + 真 RuntimeHost（tempdir 派生库）+
//! 真 `AnthropicApiClient`，模型槽指向本地假 Anthropic provider。全程不触
//! 用户真实 config.db / bib.db / API key。
//!
//! 覆盖验收两条主线：
//! 1. **全链**：web 聊天 turn → spawn_agent（子代理落 `/root/web/homepage/`
//!    子树）→ delegate_to + wait_task → 子代理假模型回话 → 结果回流父会话
//!   （SSE tool 帧 + 最终文本 + /delegations completed 台账 + /agents live 行）。
//! 2. **越界**：host 侧预置 `/root/researcher`，父代理 delegate_to 它 → 宿主
//!    拒绝（deny-before-record，不落台账），可读错误经 tool_result 回到模型
//!    上下文（假 provider 的下一个请求体里可见）。
//!
//! 假 provider 按**请求体标记**脚本化（不依赖到达顺序）：每个阶段的前一轮
//! tool_result / 注入消息都携带唯一标记，分发函数据此挑 canned SSE 回复。
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

// ── 剧本标记：出现在用户消息 / 工具入参 / 脚本回复里，供 provider 分发与断言 ──
const MSG1_MARKER: &str = "E2E-P5B-MSG1";
const MSG2_MARKER: &str = "E2E-P5B-MSG2";
const CHILD_TASK_MARKER: &str = "E2E-P5B-CHILD-TASK";
const CHILD_REPLY_MARKER: &str = "E2E-P5B-CHILD-REPLY";
const PARENT_FINAL_MARKER: &str = "E2E-P5B-PARENT-FINAL";
const BOUNDARY_FINAL_MARKER: &str = "E2E-P5B-BOUNDARY-FINAL";

const CHILD_TASK: &str = "E2E-P5B-CHILD-TASK 请调研 RNA 结合蛋白的主流计算方法。";
const CHILD_REPLY: &str = "E2E-P5B-CHILD-REPLY 子代理完成调研，共 3 类方法。";

// ---------------------------------------------------------------------------
// 假 Anthropic provider：标准 Anthropic SSE 线格式（text / tool_use 两种块）
// ---------------------------------------------------------------------------

fn sse_frame(event: &str, data: serde_json::Value) -> String {
    format!("event: {event}\ndata: {}\n\n", data.to_string())
}

fn sse_message_head(msg_id: &str) -> String {
    sse_frame(
        "message_start",
        serde_json::json!({
            "type": "message_start",
            "message": {
                "id": msg_id, "type": "message", "role": "assistant",
                "model": "e2e-model", "content": [], "stop_reason": null,
                "stop_sequence": null,
                "usage": {"input_tokens": 12, "output_tokens": 1},
            },
        }),
    )
}

fn sse_tail(stop_reason: &str) -> String {
    let mut body = sse_frame(
        "message_delta",
        serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": stop_reason, "stop_sequence": null},
            "usage": {"output_tokens": 6},
        }),
    );
    body.push_str(&sse_frame(
        "message_stop",
        serde_json::json!({"type": "message_stop"}),
    ));
    body
}

/// 纯文本回复（子代理结果 / 父代理最终回答）。
fn sse_text(msg_id: &str, reply: &str) -> String {
    let mut body = sse_message_head(msg_id);
    body.push_str(&sse_frame(
        "content_block_start",
        serde_json::json!({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": ""},
        }),
    ));
    body.push_str(&sse_frame(
        "content_block_delta",
        serde_json::json!({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": reply},
        }),
    ));
    body.push_str(&sse_frame(
        "content_block_stop",
        serde_json::json!({"type": "content_block_stop", "index": 0}),
    ));
    body.push_str(&sse_tail("end_turn"));
    body
}

/// 工具调用回复（可并行多个 tool_use 块，input 单帧整发——解析端在
/// content_block_stop 时合帧解析 partial_json）。
fn sse_tools(msg_id: &str, tools: &[(&str, serde_json::Value)]) -> String {
    let mut body = sse_message_head(msg_id);
    for (index, (name, input)) in tools.iter().enumerate() {
        body.push_str(&sse_frame(
            "content_block_start",
            serde_json::json!({
                "type": "content_block_start", "index": index,
                "content_block": {
                    "type": "tool_use",
                    "id": format!("toolu_{msg_id}_{index}"),
                    "name": name,
                    "input": {},
                },
            }),
        ));
        body.push_str(&sse_frame(
            "content_block_delta",
            serde_json::json!({
                "type": "content_block_delta", "index": index,
                "delta": {"type": "input_json_delta", "partial_json": input.to_string()},
            }),
        ));
        body.push_str(&sse_frame(
            "content_block_stop",
            serde_json::json!({"type": "content_block_stop", "index": index}),
        ));
    }
    body.push_str(&sse_tail("tool_use"));
    body
}

/// 从请求体解析最近的后台任务号（"Task #N is running in background" 出现在
/// delegate_to 的启动 tool_result 里；多轮委派时取最后一个）。
fn latest_task_seq(body: &str) -> u64 {
    let mut seq = 1;
    for (_, [n]) in regex_matches(body) {
        if let Ok(parsed) = n.parse::<u64>() {
            seq = parsed;
        }
    }
    seq
}

/// 极简 `Task #N is running` 匹配（避免为测试引入 regex 依赖）。
fn regex_matches(body: &str) -> Vec<(usize, [&str; 1])> {
    const NEEDLE: &str = "Task #";
    const SUFFIX: &str = " is running";
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut from = 0;
    while let Some(rel) = body[from..].find(NEEDLE) {
        let start = from + rel + NEEDLE.len();
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > start && body[end..].starts_with(SUFFIX) {
            out.push((start, [&body[start..end]; 1]));
        }
        from = start;
    }
    out
}

/// 按请求体标记挑 canned 回复。分支顺序即剧本阶段（后面的阶段携带前面
/// 阶段的全部历史标记，必须先匹配更晚的阶段；跨轮标记——第一轮的
/// CHILD_REPLY / "Task #N is running" 会留在第二轮历史里——用「最后
/// 一条消息」的轮次定位区分，见 `last_message_is_user_with`）：
/// 1. 越界拒绝已回流（"outside your sandbox"，经 wait_task 结果或后台
///    完成注入）→ 边界轮终答
/// 2. 最后一条消息是第二轮 user 文本 → 委派 researcher（越界目标）
/// 3. 子代理结果在场且仍在第一轮 → 第一轮终答
/// 4. 只有子任务、无父会话历史 → 子代理自己的请求，回结果（慢——见
///    `spawn_fake_anthropic` 里的延迟说明）
/// 5. delegate_to 已启动（"Task #N is running"）→ wait_task 等该任务
///    （任务号从请求体解析，绝不硬编码）
/// 6. spawn 的 tool_result 在场 → 委派 helper
/// 7. 第一条用户消息在场 → spawn_agent helper
/// 其余（如 KMS 记忆整理的后台模型调用，请求体无任何标记）→ 空 JSON 数组
///
/// 注意 delegate_to 与 wait_task 分两轮发：同一助手消息里的并行工具在
/// agentik 侧按完成序回 tool_result，会与 tool_use 顺序错位（也会让
/// wait_task 抢在 delegate_to 建任务前执行）——真实模型同样是看到
/// "Task #N" 后下一轮再 wait_task。
fn reply_for(body: &str) -> String {
    if body.contains("outside your sandbox") {
        return sse_text("msg_b3", "E2E-P5B-BOUNDARY-FINAL 越界委派被宿主拒绝。");
    }
    if last_message_is_user_with(body, MSG2_MARKER) {
        return sse_tools(
            "msg_b1",
            &[(
                "delegate_to",
                serde_json::json!({"agent_name": "researcher", "task": "请整理研究计划"}),
            )],
        );
    }
    if body.contains(CHILD_REPLY_MARKER) && !body.contains(MSG2_MARKER) {
        return sse_text("msg_p3", "E2E-P5B-PARENT-FINAL 子代理结果已汇总。");
    }
    if body.contains(CHILD_TASK_MARKER) && !body.contains(MSG1_MARKER) {
        return sse_text("msg_c1", CHILD_REPLY);
    }
    if body.contains("is running in background") {
        return sse_tools(
            "msg_wait",
            &[("wait_task", serde_json::json!({"task": latest_task_seq(body)}))],
        );
    }
    if body.contains("spawned and registered") {
        return sse_tools(
            "msg_p2",
            &[(
                "delegate_to",
                serde_json::json!({"agent_name": "helper", "task": CHILD_TASK}),
            )],
        );
    }
    if body.contains(MSG1_MARKER) {
        return sse_tools("msg_p1", &[("spawn_agent", serde_json::json!({"agent_name": "helper"}))]);
    }
    sse_text("msg_other", "[]")
}

/// 子代理自己的模型请求（无父会话历史）。provider 对这一支延迟应答，
/// 保证父代理的 wait_task 先执行——子代理是同一假 provider 服务的一个
/// TCP 往返，快过父代理下一轮的话任务已 done，wait_task 直接带回结果，
/// SSE 里就没有 Waiting 暂停的 wait_task 帧了（验收要求该帧出现）。
fn is_child_turn(body: &str) -> bool {
    body.contains(CHILD_TASK_MARKER) && !body.contains(MSG1_MARKER)
}

/// 最后一条消息是否为携带 `marker` 的 user 文本。轮次定位用：新用户
/// 消息刚注入、还没有任何工具活动跟随时，它就是会话的最后一条消息——
/// 以此区分"第二轮刚开始"（该发起越界委派）与"第二轮委派已启动"
///（历史里仍有第一轮的 "Task #N is running" 陈旧文本，不能触发 wait）。
fn last_message_is_user_with(body: &str, marker: &str) -> bool {
    let payload = body.split_once("\r\n\r\n").map(|(_, p)| p).unwrap_or(body);
    let Ok(data) = serde_json::from_str::<serde_json::Value>(payload) else {
        return false;
    };
    data["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .map(|last| {
            last["role"] == "user"
                && last["content"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "text"
                            && block["text"].as_str().is_some_and(|t| t.contains(marker))
                    })
                })
        })
        .unwrap_or(false)
}

/// 起 `POST /v1/messages` 假服务：读完整请求（记录净荷供断言），按
/// `reply_for` 分发 canned SSE。
async fn spawn_fake_anthropic(seen: Arc<Mutex<Vec<String>>>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let seen = seen.clone();
            tokio::spawn(async move {
                // 读完整请求（头 + content-length 净荷），供事后断言
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
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
                let body = String::from_utf8_lossy(&buf).into_owned();
                {
                    let mut seen = seen.lock().unwrap();
                    seen.push(body.clone());
                }
                // 慢子代理（见 `is_child_turn` 文档）：先让父代理的
                // wait_task 在任务 running 时执行到 Waiting 暂停。
                if is_child_turn(&body) {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\n\
                     content-type: text/event-stream\r\n\
                     connection: close\r\n\
                     \r\n{}",
                    reply_for(&body)
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
    let connect = tokio::time::timeout(Duration::from_secs(60), TcpStream::connect(addr));
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
    tokio::time::timeout(Duration::from_secs(60), stream.read_to_end(&mut raw))
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

async fn get_json(addr: SocketAddr, path: &str) -> serde_json::Value {
    let (_, body) = http(addr, "GET", path, None).await;
    serde_json::from_str(&body).unwrap_or(serde_json::Value::Null)
}

// ---------------------------------------------------------------------------
// 全链 + 越界
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn web_chat_delegates_within_subtree_and_denies_cross_tree_end_to_end() {
    // ── 假 provider：真 Anthropic SSE 线格式，走生产 AnthropicApiClient 解析 ──
    let provider_requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let provider_addr = spawn_fake_anthropic(provider_requests.clone()).await;

    // ── 真 RuntimeHost（tempdir 派生库）+ 驱动循环 + 真 TCP 服务 ──
    // builder 构造让全部派生路径（agent.db 等）落 tempdir：`Default` 会
    // 预先从 $HOME/env 解析，事后只改 state_dir 字段不会重派生 agent_db。
    let dir = tempfile::tempdir().unwrap();
    let config = runtime::RuntimeConfig::builder()
        .data_dir(dir.path().join("data"))
        .state_dir(dir.path().join("state"))
        .build();
    let mut host = runtime::RuntimeHost::open(&config).await.unwrap();

    // 大上下文：web/homepage 的系统提示词带全套 host 工具 schema，
    // dummy 的 4096 可能触发 mid-turn 压缩改写历史标记。
    let mut model_info = agentik_core::testing::dummy_model_info("e2e-model");
    model_info.context_length = 200_000;
    let anthropic = Anthropic::new(
        "e2e-test-key",
        format!("http://127.0.0.1:{}", provider_addr.port()),
    )
    .unwrap();
    let model: Arc<ArcSwapOption<Model>> =
        Arc::new(ArcSwapOption::from_pointee(Model::with_client(
            model_info,
            AnthropicApiClient::new(anthropic),
        )));
    // host 侧 spawn（Spawn 命令）从 host 全局模型槽取模型——桌面壳同款接线
    host.set_model(model.clone());

    let shared = BibShared::open_in_memory().await.unwrap();
    let router = tui_http::ApiRouterBuilder::new(shared)
        .model(model)
        .host(host.infra())
        .build();
    let server = tui_http::start(router, "127.0.0.1:0").await.unwrap();
    let addr = server.addr();
    // HostCommand（spawn/delegate/interrupt…）只有驱动循环会处理
    //（spawn_driver 接管 host 所有权，control 句柄先取出来）
    let control = host.control();
    let driver = host.spawn_driver();

    // ── 越界靶子：host 侧（TUI 同款入口）预置 /root/researcher ──
    let researcher_profile = agentik_core::AgentProfile::new("researcher");
    control
        .spawn_with_profile(
            "researcher",
            &agentik_types::AgentPath::root(),
            researcher_profile,
            None,
        )
        .await
        .expect("spawn researcher host-side");

    // ── 建 thread：惰性拉起常驻 web/homepage，同时把 profile 行注册进
    //    host 的 spawn 查找缓存（RegisterProfile，P5b 补丁）──
    let (status, body) = http(
        addr,
        "POST",
        "/api/v1/agent/threads",
        Some(r#"{"agent_type": "homepage"}"#),
    )
    .await;
    assert_eq!(status, 200, "create thread: {body}");
    let thread_id = serde_json::from_str::<serde_json::Value>(&body).unwrap()["thread_id"]
        .as_str()
        .expect("thread_id")
        .to_owned();

    // ── 第一轮（全链）：spawn → delegate → wait → 结果回流 ──
    let (status, body) = http(
        addr,
        "POST",
        &format!("/api/v1/agent/threads/{thread_id}/chat"),
        Some(
            r#"{
                "agent_type": "homepage",
                "message": "E2E-P5B-MSG1 请派一个 helper 子代理，把调研任务委派给它并等结果。"
            }"#,
        ),
    )
    .await;
    assert_eq!(status, 200, "turn 1: {body}");
    // SSE 协议帧：三个工具的 start 帧成对出现
    for tool in ["spawn_agent", "delegate_to", "wait_task"] {
        assert!(
            body.contains(&format!("\"name\":\"{tool}\"")),
            "SSE 应含 {tool} 工具帧: {body}"
        );
    }
    assert!(body.contains("event: tool_call_result"), "工具结果帧: {body}");
    // 结果回流：子代理文本作为工具结果进入父上下文，父最终回答成流
    assert!(
        body.contains("event: text_delta") && body.contains(PARENT_FINAL_MARKER),
        "父最终回答: {body}"
    );
    assert!(body.contains("event: done"), "终端帧: {body}");

    // ── 观测面：/agents 有 live 子代理行（host 注册表来源；注册是异步
    //    channel → driver tick，轮询到视图就绪）──
    let rows = {
        let mut rows = Vec::new();
        for _ in 0..100 {
            let agents = get_json(addr, "/api/v1/agent/agents").await;
            if let Some(list) = agents["agents"].as_array() {
                rows = list.clone();
                if rows.iter().any(|row| {
                    row["path"] == "/root/web/homepage/helper" && row["live"] == true
                }) {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        rows
    };
    assert!(
        rows.iter().any(|row| row["path"] == "/root/researcher" && row["live"] == true),
        "host 侧预置的 researcher 应为 live: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row["path"] == "/root/web/homepage" && row["live"] == true),
        "常驻父代理行: {rows:?}"
    );

    // ── 观测面：/delegations 一条 completed 台账（结果=子代理全文）──
    let ledger = {
        let mut ledger = Vec::new();
        for _ in 0..100 {
            let delegations = get_json(addr, "/api/v1/agent/delegations").await;
            if let Some(list) = delegations["delegations"].as_array() {
                ledger = list.clone();
                if ledger.iter().any(|row| {
                    row["status"] == "completed"
                        && row["target_path"] == "/root/web/homepage/helper"
                        && row["response"]
                            .as_str()
                            .is_some_and(|text| text.contains(CHILD_REPLY_MARKER))
                }) {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        ledger
    };
    assert_eq!(ledger.len(), 1, "仅一条委派记录: {ledger:?}");
    assert_eq!(ledger[0]["caller_path"], "/root/web/homepage");

    // ── 第二轮（越界）：delegate_to 子树外 researcher 被拒，错误回流模型 ──
    let (status, body) = http(
        addr,
        "POST",
        &format!("/api/v1/agent/threads/{thread_id}/chat"),
        Some(
            r#"{
                "agent_type": "homepage",
                "message": "E2E-P5B-MSG2 请把这个任务委派给 researcher 处理。"
            }"#,
        ),
    )
    .await;
    assert_eq!(status, 200, "turn 2: {body}");
    assert!(
        body.contains("\"name\":\"delegate_to\""),
        "越界轮仍发起委派: {body}"
    );
    assert!(
        body.contains("event: text_delta") && body.contains(BOUNDARY_FINAL_MARKER),
        "越界轮最终回答: {body}"
    );
    assert!(body.contains("event: done"), "越界轮终端帧: {body}");

    // 拒绝发生在落台账之前（deny-before-record）：台账不增
    let delegations = get_json(addr, "/api/v1/agent/delegations").await;
    assert_eq!(
        delegations["delegations"].as_array().map(Vec::len),
        Some(1),
        "越界委派不产生台账: {delegations}"
    );

    // ── 转写：两轮 user + 两条最终回答落库（WAL 异步，轮询到边界轮
    //    终答出现；工具中间轮可能是空文本行，不做精确条数断言）──
    let messages = {
        let mut messages = Vec::new();
        for _ in 0..100 {
            let transcript = get_json(
                addr,
                &format!("/api/v1/agent/threads/{thread_id}/messages"),
            )
            .await;
            if let Some(list) = transcript["messages"].as_array() {
                messages = list.clone();
                let last = messages.last();
                if last.is_some_and(|row| {
                    row["role"] == "assistant"
                        && row["text"]
                            .as_str()
                            .is_some_and(|text| text.contains(BOUNDARY_FINAL_MARKER))
                }) {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        messages
    };
    assert_eq!(
        messages.first().map(|row| row["role"].clone()),
        Some(serde_json::json!("user"))
    );
    assert!(
        messages[0]["text"]
            .as_str()
            .is_some_and(|text| text.contains(MSG1_MARKER)),
        "首条为第一轮 user 消息: {messages:?}"
    );
    for (role, marker) in [
        ("user", MSG2_MARKER),
        ("assistant", PARENT_FINAL_MARKER),
        ("assistant", BOUNDARY_FINAL_MARKER),
    ] {
        assert!(
            messages.iter().any(|row| {
                row["role"] == role
                    && row["text"].as_str().is_some_and(|text| text.contains(marker))
            }),
            "转写应含 {role} × {marker}: {messages:?}"
        );
    }

    // ── 强证据（provider 净荷）──
    let requests = provider_requests.lock().unwrap().clone();
    // 子代理真的跑了一轮（它的请求只有任务文本，没有父会话历史）
    assert!(
        requests.iter().any(|body| {
            body.contains(CHILD_TASK_MARKER) && !body.contains(MSG1_MARKER)
        }),
        "子代理应有独立的一轮模型调用"
    );
    // 越界的可读错误回到了模型上下文（模型看得到拒绝原因）
    assert!(
        requests.iter().any(|body| body.contains("outside your sandbox")),
        "拒绝说明应出现在后续模型请求里"
    );
    // spawn 未因 profile 缓存缺失而失败（RegisterProfile 补丁的回归锚）
    assert!(
        !requests.iter().any(|body| body.contains("Spawn failed")),
        "spawn_agent 不应失败: {requests:#?}"
    );
    // 剧本没有跑飞（每个 canned 回复至多用一次）
    assert!(
        !requests.iter().any(|body| body.contains("unexpected-script-miss")),
        "脚本分发不应有漏配: {requests:#?}"
    );

    server.shutdown().await.unwrap();
    let mut host = driver.join().await;
    host.shutdown_all_agents_and_wait().await;
}
