use crate::types::errors::{AnthropicError, Result};
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Authentication method for different API gateways
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMethod {
    /// Standard Anthropic API authentication with x-api-key header
    #[default]
    Anthropic,
    /// Bearer token authentication for custom gateways and third-party services
    Bearer,
    /// ChatGPT 订阅 OAuth：Bearer token + `chatgpt-account-id` 头。
    ///
    /// 持久化为 tag `"chatgpt"`（见 [`Self::storage_tag`]）；从存储读回时
    /// account_id 为空占位，实际值由 `Model::new` 从 token blob 填充。
    Chatgpt { account_id: String },
}

impl AuthMethod {
    /// 持久化标签（providers.auth_method 列）。与 TUI 既有写入值
    /// `"Anthropic"` / `"Bearer"` 大小写对齐。
    #[must_use]
    pub fn storage_tag(&self) -> &'static str {
        match self {
            AuthMethod::Anthropic => "Anthropic",
            AuthMethod::Bearer => "Bearer",
            AuthMethod::Chatgpt { .. } => "chatgpt",
        }
    }
}

impl TryFrom<String> for AuthMethod {
    type Error = AnthropicError;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        match value.to_lowercase().as_str() {
            "anthropic" => Ok(AuthMethod::Anthropic),
            "bearer" => Ok(AuthMethod::Bearer),
            "chatgpt" => Ok(AuthMethod::Chatgpt {
                account_id: String::new(),
            }),
            other => Err(AnthropicError::Configuration {
                message: format!("Unknown auth method: {other}"),
            }),
        }
    }
}

/// Authentication handler for Anthropic API and compatible gateways
#[derive(Debug, Clone)]
pub struct AuthHandler {
    api_key: String,
    auth_method: AuthMethod,
    /// OAuth access-token 热更新槽。仅 [`AuthMethod::Chatgpt`] 使用：
    /// 存在时其值优先于 `api_key` 快照（401 自愈刷新后无需重建客户端）。
    token_slot: Option<Arc<arc_swap::ArcSwap<String>>>,
}

impl AuthHandler {
    /// Create a new auth handler with standard Anthropic authentication
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            auth_method: AuthMethod::Anthropic,
            token_slot: None,
        }
    }

    /// Create a new auth handler with Bearer token authentication
    pub fn new_bearer(api_key: String) -> Self {
        Self {
            api_key,
            auth_method: AuthMethod::Bearer,
            token_slot: None,
        }
    }

    /// Create a new auth handler with specified method
    pub fn with_method(api_key: String, auth_method: AuthMethod) -> Self {
        Self {
            api_key,
            auth_method,
            token_slot: None,
        }
    }

    /// Attach the OAuth token hot-swap slot (ChatGPT subscription login).
    #[must_use]
    pub fn with_token_slot(mut self, slot: Arc<arc_swap::ArcSwap<String>>) -> Self {
        self.token_slot = Some(slot);
        self
    }

    /// Add authentication headers to the request
    pub fn add_auth_headers(&self, headers: &mut HeaderMap) -> Result<()> {
        match &self.auth_method {
            AuthMethod::Anthropic => {
                let api_key_header = HeaderValue::from_str(&self.api_key).map_err(|_| {
                    AnthropicError::Configuration {
                        message: "Invalid API key format".to_string(),
                    }
                })?;

                headers.insert("x-api-key", api_key_header);
                headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
            }
            AuthMethod::Bearer => {
                let bearer_token = format!("Bearer {}", self.api_key);
                let auth_header = HeaderValue::from_str(&bearer_token).map_err(|_| {
                    AnthropicError::Configuration {
                        message: "Invalid API key format for Bearer token".to_string(),
                    }
                })?;

                headers.insert("authorization", auth_header);
                headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
            }
            AuthMethod::Chatgpt { account_id } => {
                // 槽位值优先于构造时的快照：自愈刷新后请求头立即生效。
                let token = match &self.token_slot {
                    Some(slot) => slot.load_full().as_str().to_owned(),
                    None => self.api_key.clone(),
                };
                let bearer_token = format!("Bearer {token}");
                let auth_header = HeaderValue::from_str(&bearer_token).map_err(|_| {
                    AnthropicError::Configuration {
                        message: "Invalid API key format for Bearer token".to_string(),
                    }
                })?;
                headers.insert("authorization", auth_header);
                // account_id 为空（占位形态）时不发空值头。
                if !account_id.is_empty() {
                    let account_header = HeaderValue::from_str(account_id).map_err(|_| {
                        AnthropicError::Configuration {
                            message: "Invalid chatgpt-account-id format".to_string(),
                        }
                    })?;
                    headers.insert("chatgpt-account-id", account_header);
                }
            }
        }

        headers.insert("content-type", HeaderValue::from_static("application/json"));

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
        headers.get(name).and_then(|v| v.to_str().ok())
    }

    #[test]
    fn chatgpt_injects_bearer_and_account_header() {
        let auth = AuthHandler::with_method(
            "tok".to_string(),
            AuthMethod::Chatgpt {
                account_id: "org-123".to_string(),
            },
        );
        let mut headers = HeaderMap::new();
        auth.add_auth_headers(&mut headers).unwrap();
        assert_eq!(get(&headers, "authorization"), Some("Bearer tok"));
        assert_eq!(get(&headers, "chatgpt-account-id"), Some("org-123"));
        assert!(headers.get("anthropic-version").is_none());
        assert_eq!(get(&headers, "content-type"), Some("application/json"));
    }

    #[test]
    fn chatgpt_placeholder_account_omits_header() {
        let auth = AuthHandler::with_method(
            "tok".to_string(),
            AuthMethod::Chatgpt {
                account_id: String::new(),
            },
        );
        let mut headers = HeaderMap::new();
        auth.add_auth_headers(&mut headers).unwrap();
        assert!(headers.get("chatgpt-account-id").is_none());
    }

    #[test]
    fn chatgpt_token_slot_overrides_snapshot() {
        let slot = std::sync::Arc::new(arc_swap::ArcSwap::from_pointee("fresh-token".to_string()));
        let auth = AuthHandler::with_method(
            "stale-token".to_string(),
            AuthMethod::Chatgpt {
                account_id: "org-1".to_string(),
            },
        )
        .with_token_slot(slot);
        let mut headers = HeaderMap::new();
        auth.add_auth_headers(&mut headers).unwrap();
        assert_eq!(get(&headers, "authorization"), Some("Bearer fresh-token"));
    }

    #[test]
    fn storage_tag_roundtrip() {
        for method in [AuthMethod::Anthropic, AuthMethod::Bearer] {
            let tag = method.storage_tag();
            assert_eq!(AuthMethod::try_from(tag.to_string()).unwrap(), method);
        }
        assert_eq!(
            AuthMethod::Chatgpt {
                account_id: String::new()
            }
            .storage_tag(),
            "chatgpt"
        );
        assert!(matches!(
            AuthMethod::try_from("chatgpt".to_string()).unwrap(),
            AuthMethod::Chatgpt { .. }
        ));
    }
}
