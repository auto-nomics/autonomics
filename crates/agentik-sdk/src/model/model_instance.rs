use std::sync::Arc;

use crate::Anthropic;
use crate::config::ClientConfig;
use crate::model::ProviderConfig;
use crate::model::model_info::ModelInfo;
use crate::model::oauth_context::{OAuthContext, RefreshCallback};
use crate::model::sanitize::sanitize_messages;
use crate::provider::client::AnthropicApiClient;
use crate::provider::client::ApiClient;
use crate::provider::openai::oauth::TokenBlob;
use crate::streaming::MessageStream;
use agentik_types::errors::AnthropicError;
use agentik_types::messages::Message;
use agentik_types::tools::ToolDefinition;

#[derive(Clone)]
pub struct Model {
    pub model_info: ModelInfo,
    /// Owning provider's name (e.g. `"zai"`) — kept from the
    /// [`ProviderConfig`] so callers can report a provider-precise
    /// `provider:model` spec. Empty for test-constructed models
    /// ([`Self::with_client`] and friends).
    provider_name: String,
    client: Arc<dyn ApiClient>,
    /// ChatGPT 订阅 OAuth 运行时（仅 openai provider；其余 provider 为
    /// `None`，401 自愈逻辑整体跳过）。
    oauth: Option<OAuthContext>,
}

impl Model {
    /// Primary constructor: build `ApiClient` from the referenced provider's
    /// connection config. The model's own `provider_id` must already match
    /// `provider.id` — the caller is responsible for the join.
    ///
    /// openai provider：`api_key` 必须是 OAuth [`TokenBlob`] JSON（登录产
    /// 出），解析失败返回带指引的 Configuration 错误；实际 Bearer/account
    /// 头取自 blob，并装上热更新槽（401 自愈刷新后无需重建客户端）。
    pub fn new(model_info: ModelInfo, provider: &ProviderConfig) -> Result<Self, AnthropicError> {
        debug_assert_eq!(
            model_info.provider_id, provider.id,
            "model/provider id mismatch"
        );
        let mut client_config = ClientConfig::new(&provider.api_key, &provider.base_url)
            .with_auth_method(provider.auth_method.clone());
        let mut oauth = None;
        if provider.provider_type == crate::model::ProviderType::Openai {
            let blob = TokenBlob::from_json(&provider.api_key).map_err(|e| {
                AnthropicError::Configuration {
                    message: format!("ChatGPT 登录信息无效：{e}。请在模型配置中对 openai 重新登录"),
                }
            })?;
            let ctx = OAuthContext::new(blob.clone());
            client_config = ClientConfig::new(&blob.access_token, &provider.base_url)
                .with_auth_method(crate::http::auth::AuthMethod::Chatgpt {
                    account_id: blob.account_id.clone(),
                })
                .with_oauth_token_slot(Some(ctx.token_slot()));
            oauth = Some(ctx);
        }
        let wire = crate::provider::registry::wire_protocol(&provider.provider_type);
        let anthropic =
            Anthropic::with_config_and_wire(client_config, crate::wire::build_wire(wire)?)?;
        let api_client = AnthropicApiClient::new(anthropic);
        Ok(Self {
            model_info,
            provider_name: provider.name.clone(),
            client: Arc::new(api_client),
            oauth,
        })
    }

    /// Constructor for testing: inject a mock or custom `ApiClient`.
    pub fn with_client(model_info: ModelInfo, client: impl ApiClient + 'static) -> Self {
        Self {
            model_info,
            provider_name: String::new(),
            client: Arc::new(client),
            oauth: None,
        }
    }

    /// 测试/高级用法：注入自定义 client 与 OAuth 上下文（真实 HTTP 栈 +
    /// mock issuer 的 e2e 用）。
    pub fn with_client_and_oauth(
        model_info: ModelInfo,
        client: impl ApiClient + 'static,
        oauth: OAuthContext,
    ) -> Self {
        Self {
            model_info,
            provider_name: String::new(),
            client: Arc::new(client),
            oauth: Some(oauth),
        }
    }

    /// The owning provider's name (empty for test-constructed models).
    pub fn provider_name(&self) -> &str {
        &self.provider_name
    }

    /// Provider-precise identity: `"provider:model"` when the provider
    /// name is known, else the bare model name (test/mock models).
    pub fn model_spec(&self) -> String {
        if self.provider_name.is_empty() {
            self.model_info.model_name.clone()
        } else {
            format!("{}:{}", self.provider_name, self.model_info.model_name)
        }
    }

    /// 设置 token 刷新回调（参数 = 新 blob JSON，写库用）。仅 OAuth 模型
    /// 生效；其他 provider 原样返回。
    #[must_use]
    pub fn with_token_refreshed(mut self, cb: impl Fn(String) + Send + Sync + 'static) -> Self {
        self.oauth = self
            .oauth
            .take()
            .map(|ctx| ctx.with_on_refreshed(RefreshCallback::new(cb)));
        self
    }

    /// 条件式主动刷新（距上次 >8 天或 exp <24h）。返回 `Some(新 blob
    /// JSON)` 供调用方写库；无需刷新或非 OAuth 模型 → `None`。
    pub async fn ensure_fresh_token(&self) -> Option<Result<String, String>> {
        self.oauth.as_ref()?.ensure_fresh().await
    }

    /// 401 自愈核心：刷新 token → 写入热更新槽 → 回调上报。失败时返回
    /// 带原因（可直接展示）的 Authentication 错误。
    async fn heal_oauth(&self) -> Result<(), AnthropicError> {
        let ctx = self
            .oauth
            .as_ref()
            .expect("heal_oauth must only be called on OAuth models");
        ctx.refresh()
            .await
            .map_err(|e| AnthropicError::Authentication {
                message: format!("ChatGPT token 自动刷新失败：{e}"),
                status: 401,
            })
            .map(|_| ())
    }

    pub fn vision(mut self, enabled: bool) -> Self {
        self.model_info.vision_ability = enabled;
        self
    }
    pub fn set_context_window(mut self, window: u64) -> Self {
        self.model_info.context_length = window;
        self
    }

    pub fn context_length(&self) -> u64 {
        self.model_info.context_length
    }

    pub async fn request(
        &self,
        messages: Vec<Message>,
        tools: &[ToolDefinition],
    ) -> Result<Message, AnthropicError> {
        let messages = sanitize_messages(messages);
        let response = match self
            .client
            .request(messages.clone(), tools.to_vec(), &self.model_info)
            .await
        {
            Err(AnthropicError::Authentication { .. }) if self.oauth.is_some() => {
                self.heal_oauth().await?;
                self.client
                    .request(messages, tools.to_vec(), &self.model_info)
                    .await?
            }
            other => other?,
        };
        Ok(response)
    }

    pub async fn request_stream(
        &self,
        messages: Vec<Message>,
        tools: &[ToolDefinition],
    ) -> Result<MessageStream, AnthropicError> {
        let messages = sanitize_messages(messages);
        match self
            .client
            .request_stream(messages.clone(), tools.to_vec(), &self.model_info)
            .await
        {
            Err(AnthropicError::Authentication { .. }) if self.oauth.is_some() => {
                self.heal_oauth().await?;
                self.client
                    .request_stream(messages, tools.to_vec(), &self.model_info)
                    .await
            }
            other => other,
        }
    }

    /// Like [`request_stream`](Self::request_stream) but also sets the
    /// top-level `system` field on the request. Use this when the
    /// caller has a system prompt that should NOT be mixed into the
    /// messages array (avoids same-role coalescing that breaks
    /// tool_use/tool_result adjacency).
    pub async fn request_stream_with_system(
        &self,
        messages: Vec<Message>,
        tools: &[ToolDefinition],
        system: Option<String>,
    ) -> Result<MessageStream, AnthropicError> {
        let messages = sanitize_messages(messages);
        match self
            .client
            .request_stream_with_system(
                messages.clone(),
                tools.to_vec(),
                &self.model_info,
                system.clone(),
            )
            .await
        {
            Err(AnthropicError::Authentication { .. }) if self.oauth.is_some() => {
                self.heal_oauth().await?;
                self.client
                    .request_stream_with_system(messages, tools.to_vec(), &self.model_info, system)
                    .await
            }
            other => other,
        }
    }

    /// 是否为 ChatGPT 订阅 OAuth 模型（openai + 合法 token blob 构造）。
    #[must_use]
    pub fn is_chatgpt(&self) -> bool {
        self.oauth.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::auth::AuthMethod;
    use crate::model::provider_config::ProviderType;
    use crate::provider::ProviderPreset;
    use crate::provider::openai::MODEL_GPT_6_ASTRA;
    use crate::provider::openai::OpenaiProvider;

    fn openai_provider(api_key: &str) -> ProviderConfig {
        ProviderConfig {
            id: uuid::Uuid::nil(),
            name: "openai".to_string(),
            provider_type: ProviderType::Openai,
            base_url: OpenaiProvider::default_base_url().to_string(),
            api_key: api_key.to_string(),
            auth_method: OpenaiProvider::default_auth_method(),
        }
    }

    fn model_info() -> ModelInfo {
        let mut info = OpenaiProvider::preset_models()
            .into_iter()
            .find(|m| m.model_name == MODEL_GPT_6_ASTRA)
            .expect("preset contains gpt-6-astra");
        info.provider_id = uuid::Uuid::nil();
        info
    }

    fn blob_json(access_token: &str) -> String {
        TokenBlob {
            access_token: access_token.to_string(),
            refresh_token: "refresh-token".to_string(),
            account_id: "org-9".to_string(),
            email: Some("user@example.com".to_string()),
            plan_type: Some("plus".to_string()),
            last_refresh: chrono::Utc::now(),
        }
        .to_json()
        .unwrap()
    }

    #[test]
    fn openai_with_valid_blob_builds_chatgpt_model() {
        let provider = openai_provider(&blob_json("access-1"));
        let model = Model::new(model_info(), &provider).expect("valid blob builds");
        assert!(model.is_chatgpt());
    }

    #[test]
    fn openai_with_garbage_api_key_reports_configuration_error() {
        let provider = openai_provider("not-a-blob");
        let err = Model::new(model_info(), &provider)
            .err()
            .expect("garbage api_key must fail");
        assert!(matches!(err, AnthropicError::Configuration { .. }));
        assert!(err.to_string().contains("重新登录"));
    }

    #[test]
    fn non_oauth_provider_builds_without_oauth() {
        let provider = ProviderConfig {
            id: uuid::Uuid::nil(),
            name: "deepseek".to_string(),
            provider_type: ProviderType::from("deepseek"),
            base_url: "https://api.deepseek.com".to_string(),
            api_key: "sk-x".to_string(),
            auth_method: AuthMethod::Bearer,
        };
        let mut info = model_info();
        info.provider_id = uuid::Uuid::nil();
        let model = Model::new(info, &provider).expect("non-oauth builds");
        assert!(!model.is_chatgpt());
    }

    #[test]
    fn model_spec_carries_provider_when_known() {
        let provider = ProviderConfig {
            id: uuid::Uuid::nil(),
            name: "bailian".to_string(),
            provider_type: ProviderType::from("bailian"),
            base_url: "https://dashscope.aliyuncs.com".to_string(),
            api_key: "sk-x".to_string(),
            auth_method: AuthMethod::Bearer,
        };
        let mut info = model_info();
        info.model_name = "glm-5.3".into();
        info.provider_id = uuid::Uuid::nil();
        let model = Model::new(info, &provider).expect("builds");
        assert_eq!(model.provider_name(), "bailian");
        assert_eq!(model.model_spec(), "bailian:glm-5.3");
    }

    #[test]
    fn model_spec_falls_back_to_bare_name_for_test_models() {
        let mut info = model_info();
        info.model_name = "mock-model".into();
        let model = Model::with_client(info, crate::provider::client::MockApiClient::new());
        assert_eq!(model.provider_name(), "");
        assert_eq!(model.model_spec(), "mock-model");
    }
}
