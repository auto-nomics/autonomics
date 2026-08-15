# 智能体运行时 (`agentik-core`)

[English](agent-runtime.md) | [中文](agent-runtime_zh.md)

统一的智能体循环、记忆管理、工具分发与多智能体编排。

## 功能特性

- **统一的智能体循环** —— 所有智能体共用一个行为循环。智能体个性和工具配置_仅_通过工具集和系统提示完成；循环本身不含任何智能体特定的代码路径（见 `crates/agentik-core/src/agent.rs`）。
- **响应式上下文** —— `AgentContext` trait：实现 `read()` / `write()`。循环在每个边界轮询版本号，并在其变化时向记忆注入 `[context-update]` 消息。内置 `InMemoryAgentContext` 用于测试。
- **带压缩的记忆** —— `Memory` 维护一个滚动列表的摘要 `MemoryItem`。当 token 压力逼近模型的 `context_length` 时，最旧的段由 LLM 摘要为 `summary`，并开启新的段。
- **跨会话持久记忆** —— 根级智能体从空闲的持久化会话提取高信号记忆，在 Turso 中合并生成结构化 memory entries 与紧凑摘要，将摘要注入系统提示，并提供受边界约束的 `memory_*` 读取 / 搜索工具。
- **工具集** —— `ToolRegistration` + `Toolset` 负责 schema 暴露、并行分发和每工具超时。每个 `T: ToolFunction` 自动擦除为 `DynToolFunction` 以支持异构存储。
- **内置工具** —— `attempt_complete`、`abort_task`（生命周期），`bash`（子进程，kill-on-drop 和截尾输出）。
- **生命周期** —— `AgentLifecycle`（IDLE / RUNNING / ABORTED）由内置生命周期工具驱动，智能体无需外部编排即可自行终止。
- **带反馈的重试** —— 可重试的 `AgentError` 触发指数退避，失败原因被注入记忆供下次尝试使用。
- **观测** —— 可选的 `mpsc` 事件通道将 `AgentUiEvent`（Thinking、LlmResponse、ToolCall、ToolResult、Requesting、Done、Error）流式传输到 TUI 或日志器。
- **快照** —— `AgentSnapshotStorage` trait，带 SQLite 后端，用于持久化智能体记忆和状态。
- **多智能体 `ProcessManager`** —— 以独立 tokio 任务的方式生成、启动、停止、重启和注入消息到多个智能体；将所有逐智能体事件聚合为一个 `broadcast::Receiver<ProcessEvent>` 流，含退出状态（`Completed` / `Error` / `Panicked` / `Cancelled` / `Stopped`）。

## 跨会话持久记忆

运行时在 Turso 中维护一个独立于单会话压缩的全局记忆状态。
根级智能体启动时执行 Codex 风格的两阶段管道：

1. Phase 1 在智能体数据库中认领空闲持久会话，提取结构化 `raw_memory`、
   rollout 摘要和 slug，脱敏明显密钥行，并记录 source hash，未变化的会话会被跳过。
2. Phase 2 获取全局单例合并锁，将有界的 stage-1 记录、现有 memory entries
   和用户显式更新说明合并，并原子写入 active entries、`v1` 摘要以及候选
   semantic observations。

所有智能体都可以通过专用工具搜索或读取记忆；自动注入提示的只有紧凑摘要。
可使用 `AUTONOMICS_USE_MEMORY` 和 `AUTONOMICS_GENERATE_MEMORY` 覆盖运行时默认值。
开启 `enable_kms` 后，候选 semantic observations 会被 grounding 到 KMS：
subject/object 会成为实体，关系会成为 Knowledge，并挂载到
`/Semantic Memory/<subject>` 下。只有当写入没有引入新的 KMS Error 诊断时才提交；
否则会回滚生成的实体、知识和索引挂载，并把 observation 标记为 rejected
且保留原因。KMS 表存放在同一个 `agent.db` Turso 数据库中，并与 AgentStorage
共享同一个连接锁。

KMS 工具权限遵循同一边界：任务智能体只会获得
`kms_readonly_registrations`；`kms_write_registrations` 保留给记忆 / KMS
维护进程使用（初期是确定性 grounding，后续可以是专门的潜意识维护智能体）。

## 过程宏 (`agentik-proc`)

- **`#[derive(ToolInput)]`** —— 从结构体生成 `impl ToolInput`，包括 `ToolBuilder` 链、必选与可选字段（通过 `Option<T>` 或 `#[default = ...]`），以及每字段的 `#[desc = "..."]` 描述。与结构体上的 `#[tool(name = "...", description = "...")]` 配合使用。

## 使用自定义工具运行智能体

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

// 通过过程宏声明式定义工具 schema。
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

## 多智能体编排

```rust
use agentik_core::process::ProcessManager;

let manager = ProcessManager::new();

// 生成（注册但不启动）——返回智能体 ID。
let id = manager.spawn(builder).await?;

manager.start(&id)?;
manager.inject_message(&id, vec![/* ContentBlock::Text { ... } */])?;

let mut events = manager.events();
while let Ok(ev) = events.recv().await {
    println!("{ev:?}");
}

// 优雅关闭所有智能体。
let exits = manager.shutdown().await;
```

## 架构说明

- **智能体循环设计。** 核心循环仅提供通用能力——请求/响应循环、生命周期管理、效果应用、记忆压缩。它从不编码智能体特定的行为、工具选择或提示工程。这些 exclusively 通过工具集和系统提示配置（`agent.rs:1` 文档化了这一契约）。
- **终止。** 不含工具调用的响应标志着完成（与模型的训练先验一致）。`attempt_complete` 保留用于兼容性但已弃用；循环在无工具调用分支上无论如何都会翻转为 `IDLE`。
- **类型擦除。** `ToolFunction::Input` 是关联类型，因此异构存储通过 blanket impl 擦除为 `DynToolFunction`——具体调用点保留完整的类型信息。
- **多智能体。** 每个智能体运行在自己的 tokio 任务中，拥有自己的命令通道、状态 watch 和取消令牌。一个转发器任务将逐智能体的 `AgentUiEvent`、生命周期变更和任务退出信号合并到管理器的 `ProcessEvent` 广播流中。
