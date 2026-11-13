//! Opt-in live-model integration host; credentials are read-only and never logged.
use agentik_core::agent::{Agent, AgentConfig, InternalEvent};
use agentik_sdk::{
    ContentBlock,
    model::{Model, ModelInfo, ProviderConfig, ProviderType},
    provider::registry,
    types::AgentEvent,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    routing::post,
};
use rusqlite::{Connection, OpenFlags, params};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

// Metadata-only audit at the actual model boundary. Never write prompts, keys or image bytes.
struct RequestAuditClient {
    inner: Model,
    file: std::sync::Mutex<std::fs::File>,
}
impl RequestAuditClient {
    fn record(&self, messages: &[agentik_sdk::types::messages::Message]) {
        let mut images = 0usize;
        let mut base64_chars = 0usize;
        for message in messages {
            for block in &message.content {
                if let ContentBlock::Image { source } = block {
                    images += 1;
                    if let agentik_sdk::types::messages::ImageSource::Base64 { data, .. } = source {
                        base64_chars += data.len();
                    }
                }
            }
        }
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(
                file,
                "{}",
                json!({"messages":messages.len(),"images":images,"base64_chars":base64_chars})
            );
        }
    }
}
#[async_trait::async_trait]
impl agentik_sdk::provider::client::ApiClient for RequestAuditClient {
    async fn request(
        &self,
        messages: Vec<agentik_sdk::types::messages::Message>,
        tools: Vec<agentik_sdk::types::tools::ToolDefinition>,
        info: &ModelInfo,
    ) -> Result<agentik_sdk::types::messages::Message, agentik_sdk::types::errors::AnthropicError>
    {
        self.record(&messages);
        {
            let mut model = self.inner.clone();
            model.model_info = info.clone();
            model.request(messages, &tools).await
        }
    }
    async fn request_stream(
        &self,
        messages: Vec<agentik_sdk::types::messages::Message>,
        tools: Vec<agentik_sdk::types::tools::ToolDefinition>,
        info: &ModelInfo,
    ) -> Result<agentik_sdk::streaming::MessageStream, agentik_sdk::types::errors::AnthropicError>
    {
        self.record(&messages);
        {
            let mut model = self.inner.clone();
            model.model_info = info.clone();
            model.request_stream(messages, &tools).await
        }
    }
    async fn request_stream_with_system(
        &self,
        messages: Vec<agentik_sdk::types::messages::Message>,
        tools: Vec<agentik_sdk::types::tools::ToolDefinition>,
        info: &ModelInfo,
        system: Option<String>,
    ) -> Result<agentik_sdk::streaming::MessageStream, agentik_sdk::types::errors::AnthropicError>
    {
        self.record(&messages);
        let mut model = self.inner.clone();
        model.model_info = info.clone();
        model
            .request_stream_with_system(messages, &tools, system)
            .await
    }
    async fn test_connection(&self) -> Result<(), agentik_sdk::types::errors::AnthropicError> {
        Err(agentik_sdk::types::errors::AnthropicError::Configuration {
            message: "request audit client does not implement connection probes".into(),
        })
    }
}

fn configured_model(
    path: &Path,
    provider_name: &str,
    model_name: &str,
) -> Result<(Model, Vec<String>), Box<dyn std::error::Error>> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let (kind, mut base_url, key): (String, String, String) = connection.query_row(
        "SELECT provider_type,base_url,api_key FROM providers WHERE name=?1",
        [provider_name],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if key.is_empty() {
        return Err("selected provider has no stored credential".into());
    }
    let kind = ProviderType::from(kind.as_str());
    if base_url.trim().is_empty() {
        base_url = registry::default_base_url(&kind)
            .ok_or("provider needs an explicit endpoint")?
            .into();
    }
    let provider = ProviderConfig::new(
        provider_name,
        kind,
        base_url,
        key.clone(),
        registry::default_auth_method(&ProviderType::from(provider_name)),
    );
    let mut info=connection.query_row("SELECT context_length,max_output_tokens,vision_ability,supports_function_calling,supports_streaming,supports_thinking,thinking_enabled,input_token_price,output_token_price FROM models m JOIN providers p ON p.id=m.provider_id WHERE p.name=?1 AND m.model_name=?2",params![provider_name,model_name],|r|Ok(ModelInfo {model_name:model_name.into(),provider_id:provider.id,context_length:r.get::<_,i64>(0)?.max(4096) as u64,max_output_tokens:r.get::<_,i64>(1)?.max(0) as u64,vision_ability:r.get::<_,i64>(2)?!=0,supports_function_calling:r.get::<_,i64>(3)?!=0,supports_streaming:r.get::<_,i64>(4)?!=0,supports_thinking:r.get::<_,i64>(5)?!=0,thinking_enabled:r.get::<_,i64>(6)?!=0,input_token_price:r.get(7)?,output_token_price:r.get(8)?,..Default::default()}))?;
    if !info.vision_ability || !info.supports_function_calling || !info.supports_streaming {
        return Err(
            "live figure workflow requires configured vision, function calling and streaming"
                .into(),
        );
    }
    if let Some(preset) = registry::preset_models(&provider.provider_type)
        .unwrap_or_default()
        .into_iter()
        .find(|m| m.model_name == model_name)
    {
        info.thinking_required = preset.thinking_required;
        info.max_reasoning_effort = preset.max_reasoning_effort;
    }
    let mut secrets = vec![key.clone()];
    if let Ok(Value::Object(blob)) = serde_json::from_str::<Value>(&key) {
        for name in ["access_token", "refresh_token"] {
            if let Some(token) = blob.get(name).and_then(Value::as_str) {
                secrets.push(token.into());
            }
        }
    }
    let model = Model::new(info, &provider).map_err(|e| scrub(&e.to_string(), &secrets))?;
    Ok((model, secrets))
}
fn scrub(text: &str, secrets: &[String]) -> String {
    let mut safe = text.to_string();
    for secret in secrets {
        if !secret.is_empty() {
            safe = safe.replace(secret, "[REDACTED]");
        }
    }
    safe
}
fn bounded(value: Value, secrets: &[String]) -> Value {
    match value {
        Value::String(s) => Value::String(scrub(&s, secrets).chars().take(12000).collect()),
        Value::Array(a) => Value::Array(a.into_iter().map(|v| bounded(v, secrets)).collect()),
        Value::Object(o) => Value::Object(
            o.into_iter()
                .map(|(k, v)| {
                    let sensitive = matches!(
                        k.as_str(),
                        "api_key"
                            | "access_token"
                            | "refresh_token"
                            | "authorization"
                            | "dataURL"
                            | "data"
                    );
                    (
                        k,
                        if sensitive {
                            json!("[OMITTED]")
                        } else {
                            bounded(v, secrets)
                        },
                    )
                })
                .collect(),
        ),
        other => other,
    }
}
struct Live {
    sender: mpsc::UnboundedSender<InternalEvent>,
    events: Mutex<mpsc::UnboundedReceiver<AgentEvent>>,
    trace: Mutex<std::fs::File>,
    secrets: Vec<String>,
    cancelled: CancellationToken,
    failed: AtomicBool,
}
impl Live {
    async fn record(&self, value: Value) {
        let safe = bounded(value, &self.secrets);
        let mut file = self.trace.lock().await;
        let _ = writeln!(file, "{safe}");
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Turn {
    text: String,
}
async fn turn(
    State(live): State<Arc<Live>>,
    Json(input): Json<Turn>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let fail = |status, message: &str| (status, Json(json!({"error":message})));
    if live.failed.load(Ordering::SeqCst) {
        return Err(fail(
            StatusCode::CONFLICT,
            "live session terminated after an earlier failure",
        ));
    }
    if input.text.trim().is_empty() || input.text.len() > 32000 {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "text must contain 1..32000 bytes",
        ));
    }
    let mut events = live.events.lock().await;
    live.record(json!({"type":"user","text":input.text})).await;
    live.sender
        .send(InternalEvent::MessageInject {
            content: vec![ContentBlock::Text { text: input.text }],
            origin: agentik_sdk::types::MessageOrigin::User,
            delegation_id: None,
        })
        .map_err(|_| fail(StatusCode::SERVICE_UNAVAILABLE, "agent stopped"))?;
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut results = Vec::new();
    let response = tokio::time::timeout(std::time::Duration::from_secs(240), async {
        while let Some(event) = events.recv().await {
            match event {
                AgentEvent::TextDelta(delta) => text.push_str(&delta),
                AgentEvent::ToolCall { name, input } => {
                    println!(
                        "LIVE_TOOL_CALL={} action={}",
                        name,
                        input["action"].as_str().unwrap_or("")
                    );
                    live.record(json!({"type":"tool_call","name":name,"input":input}))
                        .await;
                    calls.push(json!({"name":name,"input":input}));
                }
                AgentEvent::ToolResult { ok, content } => {
                    let content = serde_json::from_str::<Value>(&content).unwrap_or(json!(content));
                    let result = json!({"ok":ok,"content":content});
                    live.record(json!({"type":"tool_result","result":result}))
                        .await;
                    results.push(bounded(result, &live.secrets));
                }
                AgentEvent::RetryableError {
                    attempt,
                    max_retries,
                    ..
                } => {
                    live.record(json!({"type":"retry","attempt":attempt,"max_retries":max_retries}))
                        .await
                }
                AgentEvent::Error(error) => {
                    live.record(json!({"type":"error","message":error})).await;
                    return Err(());
                }
                AgentEvent::TurnCompleted {
                    status, telemetry, ..
                } => {
                    live.record(
                        json!({"type":"turn_completed","status":status,"telemetry":telemetry}),
                    )
                    .await
                }
                AgentEvent::Done => return Ok(()),
                _ => {}
            }
        }
        Err(())
    })
    .await;
    if !matches!(response, Ok(Ok(()))) {
        live.failed.store(true, Ordering::SeqCst);
        live.cancelled.cancel();
        live.record(json!({"type":"failed","timeout":response.is_err()}))
            .await;
        return Err(fail(
            StatusCode::BAD_GATEWAY,
            "live model turn failed; inspect the local trace",
        ));
    }
    live.record(json!({"type":"assistant","text":text})).await;
    println!("LIVE_TURN_DONE calls={}", calls.len());
    Ok(Json(json!({"text":text,"calls":calls,"results":results})))
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 5 {
        return Err(
            "usage: visualization_live CONFIG_DB PROVIDER MODEL FIGURE_DB TRACE_JSONL".into(),
        );
    }
    if Path::new(&args[3]).exists() || Path::new(&args[4]).exists() {
        return Err("live test requires fresh figure database and trace paths".into());
    }
    let (model, secrets) = configured_model(Path::new(&args[0]), &args[1], &args[2])?;
    let audit = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(Path::new(&args[4]).with_extension("requests.jsonl"))?;
    let model = Model::with_client(
        model.model_info.clone(),
        RequestAuditClient {
            inner: model,
            file: std::sync::Mutex::new(audit),
        },
    );
    let store = visualization::Store::open(&args[3])?;
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let mut tools = visualization_tools::registrations(args[3].clone().into());
    tools.extend(visualization_tools::bioart_registrations(
        args[3].clone().into(),
    ));
    tools.extend(visualization_tools::bioicons_registrations(
        args[3].clone().into(),
    ));
    let mut agent=Agent::builder().with_tools(tools).with_config(AgentConfig {max_iterations:12,max_retries:1}).with_system_prompt_identity("科研绘图助手").with_system_prompt_section("依据用户提供的科学内容，使用已注册工具协助规划、构图和局部编辑。用户会通过画布上下文提供目标项目与画布。").with_cancel_token(cancel.clone()).build().await?;
    agent.set_model(Arc::new(model));
    agent.set_agent_event_tx(tx);
    let sender = agent.internal_event_tx();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let live = Arc::new(Live {
        sender,
        events: Mutex::new(rx),
        trace: Mutex::new(options.open(&args[4])?),
        secrets,
        cancelled: cancel,
        failed: AtomicBool::new(false),
    });
    let task = tokio::spawn(async move {
        agent.run().await;
    });
    let app = Router::new()
        .nest(
            "/api/v1/visualization",
            api_server::visualization::router(store),
        )
        .merge(
            Router::new()
                .route("/test/turn", post(turn))
                .layer(DefaultBodyLimit::max(64 * 1024))
                .with_state(live.clone()),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    println!(
        "VISUALIZATION_LIVE_ADDR={} provider={} model={}",
        listener.local_addr()?,
        args[1],
        args[2]
    );
    axum::serve(listener, app).await?;
    let _ = live.sender.send(InternalEvent::Shutdown);
    task.await?;
    Ok(())
}
