//! ChatGPT 订阅链路真实请求冒烟测试。
//!
//! 凭据来自 `chatgpt_login` example 产出的 blob JSON(重定向落盘):
//!
//! ```text
//! cargo run -p agentik-sdk --example chatgpt_login > /tmp/chatgpt_blob.json
//!
//! CHATGPT_AUTH_JSON=/tmp/chatgpt_blob.json \
//!   cargo test -p agentik-sdk --test cache_tests -- --nocapture
//! ```
//!
//! 门控与 `custom_gateway_integration.rs` 的 `CUSTOM_BEARER_TOKEN` 同款:
//! 未配置 `CHATGPT_AUTH_JSON` 时测试直接跳过,默认 `cargo test` / CI 不受
//! 影响;配置了但文件非法则 panic(错误配置应响亮失败)。可选
//! `CHATGPT_MODEL` 覆盖默认的 gpt-6-astra。
//!
//! 测试期间若因 401 自愈刷新了 token,新 blob 会写回同一个 JSON 文件,
//! 调试期间无需反复重新登录。

use std::path::PathBuf;

use agentik_sdk::model::{Model, ProviderConfig, ProviderType};
use agentik_sdk::provider::ProviderPreset;
use agentik_sdk::provider::openai::oauth::TokenBlob;
use agentik_sdk::provider::openai::{MODEL_GPT_6_ASTRA, MODEL_GPT_6_LUNA, OpenaiProvider};
use agentik_types::messages::{ContentBlock, Message, Role};

/// Load the workspace-root `.env` regardless of cargo's test CWD
/// (与 custom_gateway_integration 同款)。
fn load_dotenv() {
    let path = format!("{}/../../.env", env!("CARGO_MANIFEST_DIR"));
    let _ = dotenvy::from_path(&path);
}

fn load_auth_json(json_path: PathBuf) -> Result<TokenBlob, String> {
    let json =
        std::fs::read_to_string(&json_path).map_err(|e| format!("read auth json failed: {e}"))?;
    let token_blob = TokenBlob::from_json(&json)?;

    Ok(token_blob)
}

/// 组装最小请求链路:blob JSON → `ProviderConfig` → `Model`。
/// 返回 `None` 表示未配置凭据,调用方跳过。
fn chatgpt_model() -> Option<Model> {
    load_dotenv();
    let path = PathBuf::from(std::env::var("CHATGPT_AUTH_JSON").ok()?);
    let blob =
        load_auth_json(path.clone()).expect("CHATGPT_AUTH_JSON 必须指向合法的 TokenBlob JSON");

    let provider = ProviderConfig {
        id: uuid::Uuid::nil(),
        name: "openai".to_string(),
        provider_type: ProviderType::Openai,
        base_url: OpenaiProvider::default_base_url().to_string(),
        api_key: blob.to_json().unwrap(),
        auth_method: OpenaiProvider::default_auth_method(),
    };
    let model_name =
        std::env::var("CHATGPT_MODEL").unwrap_or_else(|_| MODEL_GPT_6_LUNA.to_string());
    let mut info = OpenaiProvider::preset_models()
        .into_iter()
        .find(|m| m.model_name == model_name)
        .unwrap_or_else(|| panic!("openai preset 不含模型 {model_name}"));
    info.provider_id = provider.id;

    let model = Model::new(info, &provider).expect("blob 构建 ChatGPT 模型失败");
    // 401 自愈刷新后把新 blob 写回原文件,凭据文件不会过期。
    Some(model.with_token_refreshed(move |json| {
        let _ = std::fs::write(&path, json);
    }))
}

fn user_msg(text: &str) -> Message {
    Message {
        id: String::new(),
        type_: "message".to_string(),
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        model: None,
        stop_reason: None,
        stop_sequence: None,
        usage: None,
        request_id: None,
    }
}

fn text_of(msg: &Message) -> String {
    msg.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn test_request_with_auth_json() {
    let Some(model) = chatgpt_model() else {
        println!(
            "⚠️  CHATGPT_AUTH_JSON 未配置(需指向 chatgpt_login 产出的 blob JSON)- skipping test"
        );
        return;
    };

    // OpenAI 前缀缓存要求 prompt ≥ 1024 token(不足则缓存机制不启用),
    // 几十 token 的短对话永远看不到 cache_read。构造 ~3000 token 的稳定
    // 长前缀,逐轮只追加短问题,验证第二轮起前缀是否命中缓存。
    let filler: String = (0..80)
        .map(|i| {
            format!(
                "Paragraph {i:03}: The quick brown fox jumps over the lazy dog while \
                 harbor lights flicker across the quiet bay, and the old lighthouse \
                 keeper notes the tide in his leather journal."
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut msgs: Vec<Message> = vec![user_msg(&format!(
        "Remember this reference material:\n\n{filler}\n\nReply with exactly: Hi"
    ))];

    for i in 0..3 {
        let resp = model
            .request_stream(msgs.clone(), &[])
            .await
            .expect("chatgpt stream request established")
            .final_message()
            .await
            .expect("stream completes");

        println!("turn {i} response: {}", text_of(&resp));
        // cache 调试的关键观测点:usage 由 response.completed 事件解码。
        // cache_write 恒 None 属预期——OpenAI 无 Anthropic 式显式写计费,
        // 缓存只体现在读侧 cached_tokens。
        if let Some(usage) = &resp.usage {
            println!(
                "model: {} usage: input={} output={} cache_read={:?}",
                model.model_info.model_name,
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_input_tokens,
            );
        }
        msgs.push(resp);
        if i + 1 < 3 {
            msgs.push(user_msg("Reply with exactly: Hi"));
            // 实测(curl 探针,SDK 原始请求体复刻):后端 prompt cache 写入
            // 传播延迟 ~10-25s——t+10s 未命中,t+25s 命中。背靠背请求必然
            // miss;间隔 30s 验证稳态命中。
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    }
}
