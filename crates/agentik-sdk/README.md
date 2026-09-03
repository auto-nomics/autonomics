# agentik-sdk

Comprehensive, type-safe Rust SDK for the Anthropic API, extracted from
the agentik ecosystem as a standalone crate.

This project is a hard fork of
[dimichgh/anthropic-sdk-rust](https://github.com/dimichgh/anthropic-sdk-rust).

## Features

- Streaming responses (SSE)
- Tool use / function calling
- Vision / image input
- File uploads
- Batch processing
- Async/await based
- **Agent module** (behind `agent` feature): provider abstraction, model pool, Anthropic-compatible provider implementations

## Usage

```toml
[dependencies]
agentik-sdk = "0.3"
```

```rust
use agentik_sdk::Anthropic;

let client = Anthropic::new("your-api-key")?;
let response = client.messages().create(&request).await?;
```

## Agent Feature

Enable the `agent` feature for provider abstraction and model management:

```toml
[dependencies]
agentik-sdk = { version = "0.3", features = ["agent"] }
```

This adds:
- `Role::System` and `Role::Tool` for internal agent bookkeeping
- `Message` constructors: `system()`, `user()`, `assistant_text()`, `tool_result()`, etc.
- `agent::model::{Model, ModelInfo}`
- `agent::model::model_pool::{ModelPool, ModelPoolError}`
- `agent::client::{ApiClient, AnthropicApiClient, MockApiClient}`
- `agent::provider::{LlmProvider, ProviderInfo, ProviderError}`
- `agent::minimax::{MinimaxProvider, MODEL_MINIMAX_M2_7}`
- `agent::sensenova::{SensenovaProvider, MODEL_SENSENOVA_6_7_FLASH_LITE, MODEL_DEEPSEEK_V4_FLASH}`
- `agent::mimo::{MimoProvider, MimoEndpoint, TokenPlanRegion, MODEL_MIMO_V2_5_PRO, ...}`
- `agent::zai::{ZaiProvider, ZaiEndpoint, MODEL_GLM_5_1, MODEL_GLM_4_6, ...}`
- `agent::openrouter::{OpenrouterProvider, MODEL_CLAUDE_SONNET_5, MODEL_GPT_5_5, ...}` (plus
  `OpenrouterProvider::fetch_remote_catalog` for the live `/v1/models` catalogue)
- `agent::deepseek::{DeepseekProvider, MODEL_DEEPSEEK_V4_PRO, MODEL_DEEPSEEK_V4_FLASH, MODEL_DEEPSEEK_CHAT, MODEL_DEEPSEEK_REASONER}`
