use crate::http::auth::AuthMethod;
use crate::types::errors::{AnthropicError, Result};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub api_key: String,
    pub base_url: String,
    pub timeout: Duration,
    pub max_retries: u32,
    pub log_level: LogLevel,
    pub auth_method: AuthMethod,
    /// OAuth access-token 热更新槽（ChatGPT 订阅登录）。与 `Model` 的
    /// OAuth 上下文共享同一 Arc；[`crate::http::auth::AuthHandler`] 取值
    /// 优先于 `api_key` 快照，401 自愈刷新后无需重建客户端。
    pub oauth_token_slot: Option<Arc<arc_swap::ArcSwap<String>>>,
}

#[derive(Debug, Clone)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Off,
}

impl ClientConfig {
    /// Create a new client configuration with the provided API key
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            timeout: Duration::from_secs(600), // 10 minutes default
            max_retries: 2,
            log_level: LogLevel::Warn,
            auth_method: AuthMethod::Anthropic,
            oauth_token_slot: None,
        }
    }

    /// Set the request timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the maximum number of retries
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set the log level
    pub fn with_log_level(mut self, log_level: LogLevel) -> Self {
        self.log_level = log_level;
        self
    }

    /// Set the base URL
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Set the authentication method
    pub fn with_auth_method(mut self, auth_method: AuthMethod) -> Self {
        self.auth_method = auth_method;
        self
    }

    /// Attach the OAuth token hot-swap slot (ChatGPT subscription login).
    pub fn with_oauth_token_slot(mut self, slot: Option<Arc<arc_swap::ArcSwap<String>>>) -> Self {
        self.oauth_token_slot = slot;
        self
    }

    /// Configure for custom gateway (Bearer token + base URL)
    pub fn for_custom_gateway(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self.auth_method = AuthMethod::Bearer;
        self
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<()> {
        if self.api_key.is_empty() {
            return Err(AnthropicError::Configuration {
                message: "API key cannot be empty".to_string(),
            });
        }

        if self.base_url.is_empty() {
            return Err(AnthropicError::Configuration {
                message: "Base URL cannot be empty".to_string(),
            });
        }

        if !self.base_url.starts_with("http://") && !self.base_url.starts_with("https://") {
            return Err(AnthropicError::Configuration {
                message: "Base URL must start with http:// or https://".to_string(),
            });
        }

        Ok(())
    }
}
