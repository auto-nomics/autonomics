# SDK (`agentik-sdk`)

[English](sdk.md) | [中文](sdk_zh.md)

LLM API 客户端与多服务商抽象层。

## 功能特性

- **Messages API** —— 创建带系统提示、多轮、temperature、top-p、stop 序列的对话
- **流式输出（SSE）** —— 逐 token 流式输出，支持事件驱动回调（`on_text`、`on_message`、`on_error`、`on_end`）和通过 `Stream` trait 的异步迭代
- **流可靠性** —— 自动空闲超时检测和 `MessageStart` 之前的重连、HTTP body 的优雅排空、可配置的重试策略（指数退避 + 抖动）
- **工具/函数调用** —— JSON Schema 工具、`tool_use` / `tool_result` 块、服务端工具（网页搜索）
- **视觉** —— 通过 base64 或 URL 发送图像
- **Files API（Beta）** —— 上传、列出、下载，带 SHA-256 完整性校验
- **批处理（Beta）** —— 创建和管理批量推理请求
- **Models API** —— 列出和检视模型，含能力与定价元数据
- **Token 与成本追踪** —— `TokenCounter` 支持按模型定价、累计用量和成本估算
- **多服务商抽象** —— `LlmProvider` trait，实现包括：
  - Anthropic（直连）
  - DeepSeek（`deepseek-v4-pro`、`deepseek-v4-flash`）
  - MiniMax
  - SenseNova
  - Mimo
  - ZAI
  - OpenRouter（OpenAI 兼容协议；通过 `OpenrouterProvider::fetch_remote_catalog`
    拉取公开 `GET /v1/models` 端点的实时模型目录）
  - OpenAI（ChatGPT 订阅 OAuth 登录——见
    [通过 ChatGPT 订阅使用 OpenAI](#通过-chatgpt-订阅使用-openai)）
- **模型池** —— 跨服务商轮询选择模型，支持按名称粘性选择
- **灵活认证** —— Anthropic `x-api-key`、Bearer token，或面向第三方网关的自定义 header
- **Mock 支持** —— 通过 `mockall` 的 `MockApiClient`，用于测试

## 快速开始

### 通过 SDK 直接与模型对话

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

## 配置

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

### 通过 ChatGPT 订阅使用 OpenAI

`openai` provider **不填 API key**：走与 OpenAI Codex CLI 相同的 OAuth
浏览器登录，用 ChatGPT 账号（Plus/Pro 订阅）授权，请求发往 ChatGPT 后端
（`https://chatgpt.com/backend-api/codex/responses`，Responses 协议）。

- **登录**（`provider::openai::oauth::login_flow`）：PKCE + 本机回调服务器
  （`127.0.0.1:1455`，兜底 `1457`）→ 授权 URL → 换 token → `TokenBlob`
  （access/refresh token、`chatgpt_account_id`、邮箱、订阅计划）。
  TUI 操作：模型配置页 → `openai` → `Ctrl+E` → `L`。
- **存储**（零 schema 迁移）：token blob JSON 存 `providers.api_key` 列；
  `providers.auth_method` 列存标签 `chatgpt`。
- **刷新**：距上次刷新超 8 天 / 距过期不足 24h 主动刷新；HTTP 401 自愈
  （刷新 → 通过共享 `ArcSwap` 槽热替换 access token → 重试一次 → 轮转后
  的 blob 回写落库）。
- **模型目录实时拉取**：登录后自动请求
  `GET /backend-api/codex/models?client_version=…`（Codex CLI 同源端点，
  认证头与对话接口一致）；TUI 登录成功后自动刷新、`Ctrl+F` 手动刷新。
  另内置一份实测 preset 作离线兜底。
- ⚠️ **ToS 风险**：第三方客户端复用 Codex CLI 的公开 OAuth `client_id`
  属灰色地带。请用个人订阅自担风险；有先例（Roo Code 等），但 OpenAI 可能
  限流或封禁此类客户端。需要 API key 方式请配置 `openrouter` 或自定义
  provider。

### 环境变量

将 `.env.example` 复制为 `.env`，仅填写你需要的项。密钥按用途分组：

```env
# LLM 服务商（设置你使用的即可）
MIMO_API_KEY=
SENSENOVA_API_KEY=
MINIMAX_API_KEY=        # MiniMax 服务商
DEEPSEEK_API_KEY=
ZAI_API_KEY=

# 科学数据客户端
OPENGWAS_TOKEN=         # OpenGWAS / GWAS Catalog bearer token
EUTILS_API_KEY=         # 可选；将 NCBI 速率限制从 3 提升至 10 req/s
```

## API 资源

| 资源                 | 描述                             |
| -------------------- | --------------------------------- |
| `client.messages()`  | 创建消息和流式响应                 |
| `client.batches()`   | 管理批量推理请求                   |
| `client.files()`     | 上传和管理文件                     |
| `client.models()`    | 列出和检视模型                     |
