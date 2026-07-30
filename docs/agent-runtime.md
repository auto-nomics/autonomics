# Agent Runtime (`agentik-core`)

[English](agent-runtime.md) | [中文](agent-runtime_zh.md)

Uniform agent loop, memory management, tool dispatch, and multi-agent orchestration.

## Features

- **Uniform agent loop** — One behavioral loop for all agents. Agent personality and tooling are configured _only_ through the toolset and system prompt; no agent-specific code paths in the loop itself (see `crates/agentik-core/src/agent.rs`).
- **Reactive context** — `AgentContext` trait: implement `read()` / `write()`. The loop polls the version at each boundary and injects a `[context-update]` message into memory when it changes. Built-in `InMemoryAgentContext` for tests.
- **Memory with compaction** — `Memory` keeps a rolling list of summarized `MemoryItem`s. When token pressure rises against the model's `context_length`, the oldest segment is summarized by the LLM into a `summary` and a fresh segment is opened.
- **Toolset** — `ToolRegistration` + `Toolset` handle schema exposure, parallel dispatch, and per-tool timeouts. Every `T: ToolFunction` is auto-erased to `DynToolFunction` for heterogeneous storage.
- **Built-in tools** — `attempt_complete`, `abort_task` (lifecycle), `bash` (subprocess with kill-on-drop and tail-truncated output).
- **Lifecycle** — `AgentLifecycle` (IDLE / RUNNING / ABORTED) driven by built-in lifecycle tools, so agents self-terminate without external orchestration.
- **Retry with feedback** — Retryable `AgentError`s trigger exponential backoff and the failure reason is injected back into memory for the next attempt.
- **Observation** — Optional `mpsc` event channel streams `AgentUiEvent`s (Thinking, LlmResponse, ToolCall, ToolResult, Requesting, Done, Error) to a TUI or logger.
- **Snapshots** — `AgentSnapshotStorage` trait with a SQLite backend for persisting agent memory and status.
- **Multi-agent `ProcessManager`** — Spawn, start, stop, restart, and inject messages into multiple agents as independent tokio tasks; aggregates all per-agent events into one `broadcast::Receiver<ProcessEvent>` stream with exit status (`Completed` / `Error` / `Panicked` / `Cancelled` / `Stopped`).

## Proc macros (`agentik-proc`)

- **`#[derive(ToolInput)]`** — Generates `impl ToolInput` from a struct, including the `ToolBuilder` chain, required vs. optional fields (via `Option<T>` or `#[default = ...]`), and `#[desc = "..."]` per-field descriptions. Pair with `#[tool(name = "...", description = "...")]` on the struct.

## Running an agent with a custom tool

```toml
[dependencies]
agentik-core = { path = "..." }
agentik-sdk  = { path = "..." }
agentik-proc = { path = "..." }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust
use std::sync::Arc;

use agentik_core::agent::{Agent, AgentConfig};
use agentik_core::context::InMemoryAgentContext;
use agentik_core::tools::{ToolFunction, ToolResult, ToolRegistration, error::ToolError};
use agentik_core::toolset::Toolset;
use agentik_sdk::model::model_pool::ModelPool;
use agentik_sdk::provider::mimo::{MimoProvider, MODEL_MIMO_V2_5};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// Declarative tool schema via proc macro.
#[derive(Debug, Deserialize, Serialize, agentik_proc::ToolInput)]
#[tool(name = "echo", description = "Echo back a message")]
pub struct EchoInput {
    #[desc = "The text to echo back"]
    pub text: String,
}

pub struct EchoTool;

#[async_trait]
impl ToolFunction for EchoTool {
    type Input = EchoInput;

    async fn run(&self, input: EchoInput) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::success("echo", format!("echo: {}", input.text)))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = MimoProvider::new(None, std::env::var("MIMO_API_KEY")?);
    let model = provider.get_model(MODEL_MIMO_V2_5)?;

    let mut pool = ModelPool::new();
    pool.add_model(model);

    let ctx = Arc::new(InMemoryAgentContext::new());

    let mut agent = Agent::builder()
        .with_model_pool(Arc::new(pool))
        .with_context(ctx)
        .with_system_prompt_identity("You are a minimal demo agent.")
        .with_config(AgentConfig::default())
        .build()
        .await?;

    agent.register_tool(ToolRegistration::from(EchoTool))?;
    agent.start().await?;
    Ok(())
}
```

## Multi-agent orchestration

```rust
use agentik_core::process::ProcessManager;

let manager = ProcessManager::new();

// Spawn (registers but does not start) — returns the agent ID.
let id = manager.spawn(builder).await?;

manager.start(&id)?;
manager.inject_message(&id, vec![/* ContentBlock::Text { ... } */])?;

let mut events = manager.events();
while let Ok(ev) = events.recv().await {
    println!("{ev:?}");
}

// Graceful shutdown of every agent.
let exits = manager.shutdown().await;
```

## Architecture notes

- **Agent loop design.** The core loop only provides generic capabilities — request/response cycling, lifecycle management, effect application, memory compaction. It never encodes agent-specific behavior, tool selection, or prompt engineering. Configure those exclusively via the toolset and system prompt (`agent.rs:1` documents this contract).
- **Termination.** A response with no tool calls signals completion (matches the model's trained prior). `attempt_complete` is retained for compatibility but deprecated; the loop flips to `IDLE` on the no-tool-call branch regardless.
- **Type erasure.** `ToolFunction::Input` is an associated type, so heterogeneous storage erases to `DynToolFunction` via a blanket impl — concrete call sites keep full type information.
- **Multi-agent.** Each agent runs in its own tokio task with its own command channel, status watch, and cancellation token. A forwarder task merges per-agent `AgentUiEvent`s, lifecycle changes, and task-exit signals into the manager's `ProcessEvent` broadcast stream.
