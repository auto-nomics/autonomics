# SDK (`agentik-sdk`)

[English](sdk.md) | [中文](sdk_zh.md)

LLM API client with multi-provider abstraction.

## Features

- **Messages API** — Create conversations with system prompts, multi-turn, temperature, top-p, stop sequences
- **Streaming (SSE)** — Token-by-token streaming with event-driven callbacks (`on_text`, `on_message`, `on_error`, `on_end`) and async iteration via the `Stream` trait
- **Stream reliability** — Automatic idle-timeout detection and reconnection before `MessageStart`, graceful HTTP body draining, configurable retry policies with exponential backoff and jitter
- **Tool / function calling** — JSON Schema tools, `tool_use` / `tool_result` blocks, server tools (web search)
- **Vision** — Send images via base64 or URL
- **Files API (Beta)** — Upload, list, download with SHA-256 integrity verification
- **Batch processing (Beta)** — Create and manage batch inference requests
- **Models API** — List and inspect models with capability and pricing metadata
- **Token & cost tracking** — `TokenCounter` with per-model pricing, accumulated usage, and cost estimation
- **Multi-provider abstraction** — `LlmProvider` trait with implementations:
  - Anthropic (direct)
  - DeepSeek (`deepseek-v4-pro`, `deepseek-flash`)
  - MiniMax
  - SenseNova
  - Mimo
  - ZAI
  - Bailian / Alibaba Cloud Model Studio (Anthropic-compatible; five
    regional gateways plus the Coding Plan / Token Plan domains —
    `coding.dashscope.aliyuncs.com` and `coding-intl.dashscope.aliyuncs.com`).
    Replaces `<WORKSPACE-ID>` in the preset URL with the workspace id from
    the Bailian console before use.
  - StepFun / 阶跃星辰 (OpenAI Chat Completions wire; Bearer auth. General
    API at `api.stepfun.com/v1` and Step Plan / Token Plan at
    `api.stepfun.com/step_plan/v1`. Catalogue covers `step-5-preview`,
    `step-3.7-flash` / `step-3.5-flash` / `step-3.5-flash-2603`, the
    audio `step-audio-2` family, and the Step-Plan-only `step-router-v1`.)
  - OpenRouter (OpenAI-compatible wire; live catalogue via
    `OpenrouterProvider::fetch_remote_catalog` against the public
    `GET /v1/models` endpoint)
  - OpenAI (ChatGPT subscription OAuth login — see
    [OpenAI via ChatGPT subscription](#openai-via-chatgpt-subscription))
- **Model pool** — Round-robin model selection across providers, with sticky selection by name
- **Flexible auth** — Anthropic `x-api-key`, Bearer token, or custom header for third-party gateways
- **Mock support** — `MockApiClient` via `mockall` for testing

## Quick Start

### Talking to a model directly via the SDK

```toml
[dependencies]
agentik-sdk = "0.3"
```

```rust
use agentik_sdk::Anthropic;
use agentik_types::MessageCreateBuilder;

#[tokio::main]
async fn main() -> agentik_sdk::Result<()> {
    let client = Anthropic::new("your-api-key", "https://api.anthropic.com")?;

    let message = client.messages().create(
        MessageCreateBuilder::new("claude-sonnet-4-20250514", 1024)
            .system("You are a helpful assistant.")
            .user("Hello, Claude!")
            .build(),
    ).await?;

    println!("Response: {:?}", message.content);
    Ok(())
}
```

## Configuration

```rust
use agentik_sdk::{Anthropic, ClientConfig, LogLevel, AuthMethod};
use std::time::Duration;

let config = ClientConfig::new("your-api-key", "https://api.anthropic.com")
    .with_timeout(Duration::from_secs(120))
    .with_max_retries(3)
    .with_log_level(LogLevel::Info)
    .with_auth_method(AuthMethod::Anthropic);

let client = Anthropic::with_config(config)?;
```

### OpenAI via ChatGPT subscription

The `openai` provider does **not** take an API key. Instead it logs in with a
ChatGPT account (Plus/Pro subscription) through the same OAuth device flow the
OpenAI Codex CLI uses, and sends requests to the ChatGPT backend
(`https://chatgpt.com/backend-api/codex/responses`, Responses protocol).

- **Login** (`provider::openai::oauth::login_flow`): PKCE + local callback
  server on `127.0.0.1:1455` (fallback `1457`) → authorize URL → exchange →
  `TokenBlob` (access/refresh token, `chatgpt_account_id`, email, plan).
  In the TUI: model config → `openai` → `Ctrl+E` → `L`.
- **Storage** (no schema migration): the token blob JSON lives in
  `providers.api_key`; `providers.auth_method` stores the tag `chatgpt`.
- **Refresh**: proactive refresh after 8 days / within 24h of expiry, plus
  self-healing on HTTP 401 (refresh → hot-swap the access token via a shared
  `ArcSwap` slot → retry once → persist the rotated blob).
- **Live model catalogue**: after login the client fetches
  `GET /backend-api/codex/models?client_version=…` (same endpoint family the
  Codex CLI backend serves; auth headers identical to chat requests). The TUI
  auto-refreshes right after login and on `Ctrl+F`; a small verified preset is
  compiled in as the offline fallback.
- ⚠️ **ToS risk**: this reuses the Codex CLI's public OAuth `client_id` in a
  third-party client — a grey area. Use your personal subscription at your
  own risk; there is prior art (Roo Code etc.), but OpenAI may throttle or
  restrict such clients. For API-key usage, configure `openrouter` or a
  custom provider instead.

### Environment variables

Copy `.env.example` to `.env` and fill in only what you need. Keys are grouped by concern:

```env
# LLM providers (set the ones you use)
MIMO_API_KEY=
SENSENOVA_API_KEY=
MINIMAX_API_KEY=        # MiniMax provider
DEEPSEEK_API_KEY=
ZAI_API_KEY=

# Scientific data clients
OPENGWAS_TOKEN=         # OpenGWAS / GWAS Catalog bearer token
EUTILS_API_KEY=         # optional; raises NCBI rate limit 3→10 req/s
```

## API Resources

| Resource            | Description                             |
| ------------------- | --------------------------------------- |
| `client.messages()` | Create messages and streaming responses |
| `client.batches()`  | Manage batch inference requests         |
| `client.files()`    | Upload and manage files                 |
| `client.models()`   | List and inspect models                 |
