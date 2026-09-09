//! OpenAI（ChatGPT 订阅 OAuth）provider preset。
//!
//! 与其他 provider 不同：不填 API key，而是通过 OAuth 浏览器登录
//! （Codex CLI 同款流程，见 [`oauth`] 模块）拿订阅 token。token blob
//! （JSON）存在 `ProviderConfig::api_key` 里，由 `Model::new` 解析。

pub mod oauth;

use crate::http::auth::AuthMethod;
use crate::model::ModelInfo;
use crate::model::ProviderType;
use crate::model::model_info::ModelInfoBuilder;
use crate::provider::ProviderPreset;
use crate::types::ReasoningEffort;
use crate::wire::WireProtocolKind;

// ─── Model IDs ──────────────────────────────────────────────────────────────
// 实测自 GET /backend-api/codex/models（2026-09，Plus 账号）。此处为离线
// 兜底目录；登录后由 [`OpenaiProvider::fetch_remote_catalog`] 自动拉全量。
pub const MODEL_GPT_6_ASTRA: &str = "gpt-6-astra"; // 旗舰
pub const MODEL_GPT_5_5: &str = "gpt-5.5";
const LARGE_CONTEXT_WINDOW: u64 = 1_050_000;
const MAX_OUTPUT_TOKENS: u64 = 128_000;

/// ChatGPT 后端根地址。`ChatgptResponsesWire` 追加
/// `/backend-api/codex/responses`，得到完整端点
/// `https://chatgpt.com/backend-api/codex/responses`。
pub const DEFAULT_BASE_URL: &str = "https://chatgpt.com";

pub struct OpenaiProvider;

impl ProviderPreset for OpenaiProvider {
    fn provider_type() -> ProviderType {
        ProviderType::Openai
    }
    fn preset_models() -> Vec<ModelInfo> {
        Self::model_definitions()
    }
    fn default_base_url() -> &'static str {
        DEFAULT_BASE_URL
    }
    /// ChatGPT 订阅 OAuth。占位形态 account_id 为空串；`Model::new` 从
    /// token blob 填充实际值。
    fn default_auth_method() -> AuthMethod {
        AuthMethod::Chatgpt {
            account_id: String::new(),
        }
    }
    fn wire_protocol() -> WireProtocolKind {
        WireProtocolKind::ChatgptResponses
    }
    fn supports_remote_catalog() -> bool {
        true
    }
}

impl OpenaiProvider {
    /// Preset model catalogue for the openai provider type — metadata only.
    ///
    /// 订阅制无按 token 计价，pricing 置 0。
    pub fn preset_models() -> Vec<ModelInfo> {
        <Self as ProviderPreset>::preset_models()
    }

    fn model_definitions() -> Vec<ModelInfo> {
        // Official OpenAI model metadata (checked 2026-09). The ChatGPT
        // catalogue reports the subscription routing cap (272k) rather than
        // each model's full context window, so presets use the model maximum.
        vec![
            // GPT-6-Astra — 最强旗舰（官方描述 "most capable"）。
            ModelInfoBuilder::new(MODEL_GPT_6_ASTRA)
                .context(LARGE_CONTEXT_WINDOW, MAX_OUTPUT_TOKENS)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .max_reasoning_effort(ReasoningEffort::Max)
                .pricing(0.0, 0.0)
                .build(),
            // GPT-5.6-Sol — 可靠的日常 agentic 主力。
            entry("gpt-5.6-sol", LARGE_CONTEXT_WINDOW, ReasoningEffort::Max),
            // GPT-5.6-Terra — 均衡的 agentic 编码模型。
            entry("gpt-5.6-terra", LARGE_CONTEXT_WINDOW, ReasoningEffort::Max),
            // GPT-5.6-Luna — 快且便宜的 agentic 编码模型。
            entry("gpt-5.6-luna", LARGE_CONTEXT_WINDOW, ReasoningEffort::Max),
            // GPT-5.5 — 上代通用模型。
            entry(MODEL_GPT_5_5, LARGE_CONTEXT_WINDOW, ReasoningEffort::Xhigh),
            // GPT-5.4-Mini — 小型快速模型。
            entry("gpt-5.4-mini", 400_000, ReasoningEffort::Xhigh),
            // GPT-Reserve — 备用容量档。
            entry("gpt-reserve", 272_000, ReasoningEffort::Max),
            // 注：codex-auto-review 为后端内部审查模型，不进用户目录。
        ]
    }

    /// 登录后实时拉取模型目录（`GET /backend-api/codex/models?
    /// client_version=…`，Codex CLI 同源端点；认证头与对话接口一致）。
    /// 过滤内部审查模型。错误为可直接展示的人类可读信息。
    ///
    /// # Errors
    ///
    /// 连接失败、非 2xx 或解码失败时返回错误信息。
    pub async fn fetch_remote_catalog(
        base_url: &str,
        access_token: &str,
        account_id: &str,
    ) -> std::result::Result<Vec<ModelInfo>, String> {
        let url = format!(
            "{}/backend-api/codex/models?client_version=1.0.0",
            base_url.trim_end_matches('/')
        );
        let response = reqwest::Client::new()
            .get(&url)
            .timeout(std::time::Duration::from_secs(30))
            .header("Authorization", format!("Bearer {access_token}"))
            .header("chatgpt-account-id", account_id)
            .header("originator", crate::wire::openai::chatgpt::ORIGINATOR)
            .send()
            .await
            .map_err(|e| format!("catalogue request failed: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("catalogue endpoint returned {}", response.status()));
        }
        let catalogue: RemoteCatalogue = response
            .json()
            .await
            .map_err(|e| format!("catalogue decode failed: {e}"))?;
        Ok(map_remote(catalogue.models))
    }
}

/// 目录端点的单模型条目（仅取所需字段，其余忽略）。
#[derive(serde::Deserialize)]
struct RemoteModel {
    slug: String,
    #[serde(default)]
    context_window: u64,
    #[serde(default)]
    input_modalities: Vec<String>,
    #[serde(default)]
    supported_reasoning_levels: Vec<serde_json::Value>,
    #[serde(default)]
    supports_parallel_tool_calls: bool,
}

#[derive(serde::Deserialize)]
struct RemoteCatalogue {
    models: Vec<RemoteModel>,
}

/// 后端内部审查模型，过滤出用户可选目录。
const INTERNAL_MODEL_SLUGS: [&str; 1] = ["codex-auto-review"];

/// 目录条目 → `ModelInfo`（订阅制 pricing 置 0；输出上限目录不暴露，
/// 统一 128k，由服务器按实际收敛）。
fn map_remote(models: Vec<RemoteModel>) -> Vec<ModelInfo> {
    models
        .into_iter()
        .filter(|m| !INTERNAL_MODEL_SLUGS.contains(&m.slug.as_str()))
        .map(|m| {
            let vision = m.input_modalities.iter().any(|x| x == "image");
            let max_effort = strongest_reasoning_effort(&m.supported_reasoning_levels);
            let thinking = max_effort.is_some();
            let mut builder = ModelInfoBuilder::new(m.slug)
                .context(m.context_window, 128_000)
                .capabilities(vision, m.supports_parallel_tool_calls, true, thinking)
                .pricing(0.0, 0.0);
            if thinking {
                builder = builder.thinking_enabled(None);
                if let Some(effort) = max_effort {
                    builder = builder.max_reasoning_effort(effort);
                }
            }
            builder.build()
        })
        .collect()
}

/// Parse the strongest effort advertised by the ChatGPT catalogue.
fn strongest_reasoning_effort(levels: &[serde_json::Value]) -> Option<ReasoningEffort> {
    for (effort, wire_value) in [
        (ReasoningEffort::Max, "max"),
        (ReasoningEffort::Xhigh, "xhigh"),
        (ReasoningEffort::High, "high"),
        (ReasoningEffort::Medium, "medium"),
        (ReasoningEffort::Low, "low"),
        (ReasoningEffort::Minimal, "minimal"),
        (ReasoningEffort::None, "none"),
    ] {
        let matches = levels.iter().any(|level| {
            level
                .get("effort")
                .and_then(serde_json::Value::as_str)
                .or_else(|| level.as_str())
                == Some(wire_value)
        });
        if matches {
            return Some(effort);
        }
    }
    None
}

/// Shared preset entry; only context and the maximum effort differ by model.
fn entry(name: &str, context_length: u64, max_effort: ReasoningEffort) -> ModelInfo {
    ModelInfoBuilder::new(name)
        .context(context_length, MAX_OUTPUT_TOKENS)
        .capabilities(true, true, true, true)
        .thinking_enabled(None)
        .max_reasoning_effort(max_effort)
        .pricing(0.0, 0.0)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn preset_catalogue_has_real_entries() {
        let models = OpenaiProvider::preset_models();
        assert!(models.len() >= 7);
        assert!(models.iter().any(|m| m.model_name == MODEL_GPT_6_ASTRA));
        assert!(models.iter().any(|m| m.model_name == MODEL_GPT_5_5));
        for (name, context_length) in [
            (MODEL_GPT_6_ASTRA, 1_050_000),
            ("gpt-5.6-sol", 1_050_000),
            ("gpt-5.6-terra", 1_050_000),
            ("gpt-5.6-luna", 1_050_000),
            (MODEL_GPT_5_5, 1_050_000),
            ("gpt-5.4-mini", 400_000),
            ("gpt-reserve", 272_000),
        ] {
            let model = models
                .iter()
                .find(|m| m.model_name == name)
                .unwrap_or_else(|| panic!("preset contains {name}"));
            assert_eq!(model.context_length, context_length, "{name}");
            assert_eq!(model.max_output_tokens, 128_000, "{name}");
        }
        // 内部审查模型不进 preset。
        assert!(!models.iter().any(|m| m.model_name == "codex-auto-review"));
        assert!(models.iter().all(|m| m.provider_id == Uuid::nil()));
        // 订阅制：不按 token 计价。
        assert!(models.iter().all(|m| m.input_token_price == 0.0));
    }

    #[test]
    fn remote_catalogue_json_maps_to_model_info() {
        let json = r#"{"models":[{
            "slug": "gpt-6-astra",
            "context_window": 272000,
            "input_modalities": ["text", "image"],
            "supported_reasoning_levels": [{"effort": "low"}, {"effort": "high"}],
            "supports_parallel_tool_calls": true
        }, {
            "slug": "codex-auto-review",
            "context_window": 272000,
            "supported_reasoning_levels": [{"effort": "low"}]
        }, {
            "slug": "gpt-plain",
            "context_window": 128000
        }]}"#;
        let cat: RemoteCatalogue = serde_json::from_str(json).unwrap();
        let models = map_remote(cat.models);

        assert_eq!(models.len(), 2, "internal auto-review model filtered");

        let astra = &models[0];
        assert_eq!(astra.model_name, "gpt-6-astra");
        assert_eq!(astra.context_length, 272_000);
        assert_eq!(astra.max_output_tokens, 128_000);
        assert!(astra.vision_ability);
        assert!(astra.supports_function_calling);
        assert!(astra.supports_thinking);
        assert!(astra.thinking_enabled);
        assert_eq!(astra.max_reasoning_effort, Some(ReasoningEffort::High));

        // 退化条目：缺 modalities / reasoning 字段走默认值。
        let plain = &models[1];
        assert_eq!(plain.model_name, "gpt-plain");
        assert!(!plain.vision_ability);
        assert!(!plain.supports_thinking);
        assert!(!plain.thinking_enabled);
    }

    #[test]
    fn openai_defaults_to_chatgpt_auth_and_wire() {
        assert!(matches!(
            <OpenaiProvider as ProviderPreset>::default_auth_method(),
            AuthMethod::Chatgpt { .. }
        ));
        assert_eq!(
            <OpenaiProvider as ProviderPreset>::wire_protocol(),
            WireProtocolKind::ChatgptResponses
        );
    }
}
