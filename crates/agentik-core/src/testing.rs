use agentik_sdk::http::auth::AuthMethod;
use agentik_sdk::model::{Model, ModelInfo, ProviderConfig};
use agentik_sdk::provider::client::MockApiClient;

pub fn dummy_model_info(name: &str) -> ModelInfo {
    ModelInfo {
        model_name: name.into(),
        provider_id: uuid::Uuid::nil(),
        context_length: 4096,
        max_output_tokens: 1024,
        vision_ability: true,
        supports_function_calling: true,
        supports_streaming: true,
        supports_thinking: false,
        thinking_enabled: false,
        thinking_required: false,
        thinking_budget: None,
        input_token_price: 1.0,
        output_token_price: 2.0,
    }
}

/// A throwaway provider instance for tests. Uses the nil UUID so it pairs with
/// [`dummy_model_info`]'s `provider_id`.
pub fn dummy_provider_config() -> ProviderConfig {
    ProviderConfig {
        id: uuid::Uuid::nil(),
        name: "test-provider".into(),
        provider_type: "test".into(),
        base_url: "http://localhost".into(),
        api_key: "test-key".into(),
        auth_method: AuthMethod::Anthropic,
    }
}

pub fn get_mock_model(dummy_model_name: &str) -> Model {
    Model::with_client(dummy_model_info(dummy_model_name), MockApiClient::new())
}
