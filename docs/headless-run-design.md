# Headless 运行模式设计(初步)

状态:**P0/P1 已实现**(feat/headless-mode 分支);**P3-gateway 已实现**(feat/headless-via-gateway 分支)——`autonomics run` 默认经 gateway daemon 执行(`crates/headless::gateway_runner::run_via_gateway`,RunEvent 契约与退出码不变),`--ephemeral` 保留进程内 `run_task`(benchmark 隔离)。CLI:`autonomics run`(--json / -o / --profile / --model / --timeout / --session / --ephemeral / --manifest / --list-sessions);退出码 0/1/2/3。RunEvent 与 manifest 通过 `run_id` 关联;Ctrl+C 与输出管道断连会协作取消远端 turn。剩余:P2 的 --output-schema、多 turn stdin 脚本,P3 的多 agent 网络运行。参考实现:codex-rs `exec` 子命令(`/mnt/disk3/codex/codex-rs/exec`)。

## 0. 背景与目标

当前 autonomics 唯一的 LLM agent 运行入口是交互式 TUI。RuntimeHost、agentik-core
会话循环、工具体系本身是 UI 无关的,但全部初始化逻辑绑在 ratatui `App` 的生命周期里
(`apps/autonomics/src/app/mod.rs:79`),导致:

- benchmark(ReproBioBench)只能用纯 DAG 适配器绕过 agent(`token_count: 0`);
- 脚本 / CI / 定时任务无法复用 agent 能力;
- 与 BioMNI 调研中 "typed, auditable harness" 的核心主张不匹配——评测需要无人值守运行。

**目标**:提供 `run` 子命令,单命令驱动 agent 完成一个 prompt,不依赖终端 UI;
同时提供库 API,使 benchmark adapter 与 CLI 走同一条执行路径。

**非目标(初步阶段)**:

- 多轮交互式 REPL(codex 同样把交互留给 TUI,exec 只做单 turn + resume);
- 多 agent 网络编排(现有 `AgentNetwork` + `TerminationSpec` 的暴露留到后期);
- 审批/沙箱策略层(autonomics 当前无此层;工具经容器执行已有隔离);
- app-server 式 RPC 协议层(理由见 §1.2)。

## 1. 从 codex exec 借鉴的设计

### 1.1 采纳的要点

1. **stdout 纪律**(最重要的一条)。codex exec 顶部注释写明:
   默认模式下 stdout 只允许出现最终 agent 消息;`--json` 模式下 stdout 必须是严格
   的 JSONL(一行一个事件);其余一切输出(进度、日志、警告)一律走 stderr。
   并用 `#![deny(clippy::print_stdout)]` 在 lint 层强制。这使
   `codex exec "..." | jq` 和 `codex exec "..." > result.md` 都可靠。
2. **可插拔输出处理器**。`EventProcessor` trait:
   `process_server_notification → CodexStatus {Running, InitiateShutdown}`,
   加 `print_config_summary` / `process_warning` / `print_final_output`。
   人读渲染与 JSONL 序列化是同一事件流的两个消费者,而非两条代码路径。
3. **稳定的外部事件 schema**(`exec_events.rs`)。对外契约独立于内部协议枚举:
   `thread.started / turn.started / item.started|updated|completed /
   turn.completed(usage) / turn.failed(error) / error`,item 按类型化 payload
   (agent_message / reasoning / command_execution / mcp_tool_call / …)。
   内部枚举演进不破坏外部消费者。
4. **prompt 来源语义**。位置参数 | `-` 哨兵强制读 stdin | 管道 stdin 与参数并存时,
   stdin 内容作为 `<stdin>…</stdin>` 块附加到 prompt 后。
5. **脚本化辅助**。`-o/--output-last-message FILE`(即使 turn 失败也写出,空内容兜底,
   并 stderr 警告)、`--output-schema FILE`(结构化最终输出)、`--json`。
6. **会话恢复**。`exec resume [SESSION_ID|--last] [PROMPT]`;`--ephemeral` 跳过持久化。
7. **与 TUI 共享 CLI 选项与配置**(`SharedCliOptions`:model、cwd、`-c key=value`
   config 覆盖、profile)。TUI 与 exec 的配置同源,只差交互面。
8. **退出码由 turn 终态决定**,脚本可以只看 `$?` 判断成败。

### 1.2 明确不采纳的部分

- **app-server JSON-RPC 协议层**:codex exec 现在经 `InProcessAppServerClient`
  走 app-server 协议,是因为它要同时服务 IDE / TUI / cloud 多前端并支持跨进程。
  autonomics 的 TUI 与未来的 headless 在同进程直连 `RuntimeHost`,引入 RPC 层
  属于为时过早的抽象。库 API + JSONL 输出契约已覆盖需求;未来若出现独立 daemon
  需求再升级。
  > **2026-09 更新**:独立 daemon 需求已落地——见
  > `docs/design/gateway-architecture.md`(常驻 gateway + REST/SSE 多前端)。
  > headless 走 gateway 的迁移(P3)规划在该文档 §11;`--ephemeral` 保留进程内
  > 路径作为 benchmark 隔离的永久选项。
- **item 回填机制**(turn.completed 后调 thread/read 补齐 items):它源于
  app-server 通道背压丢事件。autonomics 事件通道是 unbounded mpsc,无此问题。

## 2. 架构

一个新库 crate + 一个薄子命令;不建独立二进制(codex 模式:一个二进制,多个前端)。

```
crates/headless/              # 新库 crate:autonomics-headless
  src/
    lib.rs                    # run_task(): 库入口(供 adapter、测试、CLI 复用)
    cli.rs                    # RunArgs(clap 定义;供 TUI 二进制复用)
    event.rs                  # RunEvent 外部 JSONL schema(独立于 AgentEvent)
    processor.rs              # OutputProcessor trait + human / jsonl 两个实现

apps/autonomics/
  cli.rs                      # + Command::Run(RunArgs)
  commands/run.rs             # 薄包装:init_logging → 调库 → 映射退出码
```

数据流:

```
RunArgs → RuntimeConfig(默认 + 覆盖)
        → build_model(自 App 下沉,见下)
        → RuntimeHost::open
        → host.spawn_agent(agent_path, profile, model)
        → handle.send_message(prompt)
        → 事件循环:handle.recv_event() → AgentEvent → 映射 → OutputProcessor
        ← 终止:TurnCompleted{status} (权威) / is_final_status 交叉校验
        → 收尾:shutdown_all_agents_and_wait → flush 输出 → 退出码
```

**关键前置解耦**:把 `App::build_model` / `build_model_from_spec`
(`apps/autonomics/src/app/model_config.rs:13,31`)与 profile 引导
(`seed_defaults_if_empty` + `list_profiles`)从 TUI 下沉到 runtime crate。
这是 headless 不复制 TUI 初始化逻辑的前提,也是保证两条入口配置同源的唯一办法。

## 3. CLI 契约(初步)

```text
autonomics run [OPTIONS] [PROMPT]

  PROMPT                             任务提示词;'-' 或缺省时读 stdin;
                                     管道 stdin 与参数并存时,stdin 附加为 <stdin> 块
  --json                             stdout 输出 JSONL 事件流(§4)
  -o, --output-last-message <FILE>   最终消息写入文件(turn 失败也写出)
  --profile <PATH>                   agent profile;缺省用默认 profile
  --agent <PATH>                     AgentPath;缺省 /root/headless
  --session <UUID>                   在既有会话上继续(§7)
  --model <NAME>                     覆盖模型(仅本次运行;默认仍读 settings 表)
  --timeout <SECS>                   整体超时;超时取消,退出码 2
  --manifest <FILE>                  输出 run manifest(§8)
  --list-sessions                    列出稳定 headless identity 的持久 session
  --ephemeral                        本次运行不落会话持久化(评测 / CI)
  -C, --cwd <DIR>                    工作目录(语义对齐 codex)
```

退出码:

| 码 | 含义 | benchmark 重试策略 |
|---|---|---|
| 0 | turn `Completed` | 不重试 |
| 1 | turn `Failed`(agent 层错误) | 不重试(属被测系统能力范畴) |
| 2 | 超时 / 取消 | 可重试(基础设施性失败) |
| 3 | 启动错误(host 打不开、profile 不存在、模型不可用) | 可重试(配置修复后) |

区分 1 与 2/3 是有意为之:评测场景下,agent 自己失败是测量结果,环境失败才是噪声。

## 4. 事件模型:AgentEvent → RunEvent(JSONL)

外部 schema **独立定义**(`event.rs`),不把内部 `AgentEvent` 直接 serde 出去——
内部枚举的演进不应破坏外部消费者(codex 的 exec_events 与 protocol 分层同理)。

| RunEvent | 来源(AgentEvent) | payload 要点 |
|---|---|---|
| `run.started` | — | 首事件:{run_id, agent_id, session_id, profile, model} |
| `turn.started` | `TurnStarted` | {turn_id} |
| `item.started` / `item.completed` | `ToolCall` → `ToolResult` | 一次工具调用合并为单 item 生命周期:{id, tool, input, result?, ok?} |
| `item.delta` | `TextDelta` / `ThinkingDelta` | 仅 `--json` 透传;人读模式在 stderr 流式渲染 |
| `item.completed` (agent_message) | `LlmResponse` | 完整文本;**人读模式下这就是唯一上 stdout 的内容** |
| `item.completed` (reasoning) | `Thinking` | 思考块 |
| `usage` | `UsageUpdate` | 累计 token;`turn.completed` 携带汇总 |
| `notice` | `Compact` / `PlanUpdate` / `RetryableError` / `ToolCallBackground` 等 | 非致命运行时事件,统一 tag 区分 |
| `turn.completed` | `TurnCompleted{status: Completed}` | {turn_id, usage} |
| `turn.failed` | `TurnCompleted{status: Failed}` / `Error` | {turn_id, message} |
| `run.ended` | — | 尾事件:{run_id, status, wall_time_secs, usage, turns, tool_calls} |

终止信号以 `TurnCompleted`(带 turn_id 与三态 status)为权威;`Done` 与
`LifecycleChanged` 终态仅作交叉校验。`TurnAborted` 映射为 `run.ended{status:
cancelled}` + 退出码 2。

## 5. 输出纪律

完全采纳 codex 规则,并在 crate 上加 `#![deny(clippy::print_stdout)]`(仅人读
路径的一个 `println!` 豁免,用 `#[allow]` 标注):

- 人读模式:进度(工具调用、thinking、流式 delta)→ stderr;**只有最终 agent
  消息写 stdout**;
- `--json`:stdout 全部为 JSONL 事件;人读摘要与警告 → stderr;
- tracing:复用现有 `init_logging` 的文件 appender + `--nocapture` 时并流 stderr
  的行为,headless 子命令无需新日志管线。

## 6. 终止与超时

- 权威终止:`TurnCompleted`;`is_final_status`(`host.rs:3166`,Completed|Failed)
作交叉校验;
- CLI Ctrl+C 触发协作取消并等待 `run.ended{status: cancelled}`;`--json`
  输出管道断连同样取消 gateway/进程内 agent,避免消费端退出后继续消耗模型与工具;
- 超时:`tokio::time::timeout` 包住事件循环;触发 → `handle.cancel()`(协作式
  取消,agent 会发 `TurnAborted`)→ 宽限期(默认 30s)排空事件 →
  `shutdown_all_agents_and_wait`;
- 宽限期内若仍未退出,直接 drop host 强制回收(容器工具由 container-runtime 层
  自行清理,与 TUI 的 Ctrl+C 路径一致)。

## 7. 会话恢复

agentik-core storage 已按 agent name 自动恢复并做 WAL replay
(`spawn_agent` 内 `get_agent_by_name` → `with_id`;session 快照 + WAL 在
`Agent::run()` 引导时回放),所以恢复语义几乎免费:

- `--session <UUID>`:spawn 后 `switch_session(id)` → `send_message`;
- `--ephemeral`:阶段一用临时 `state_dir` 覆盖 `RuntimeConfig` 实现(顺带解决 CI
  并发隔离);storage 层原生 no-op 写模式留作后续优化;
- `sessions list` 子命令(枚举历史会话供脚本选取)放 P1。
- `--list-sessions` 已实现:gateway 直接读取稳定 `/root/headless` agent 的持久
  session records,不要求模型配置,也不需要先 spawn agent;`--json` 输出数组。

## 8. Run manifest

对齐 BioMNI "auditable harness" 主张,提供通用 run manifest(任何 run 都产出审计元数据,
未来 benchmark adapter 在其上映射自己的契约):

```json
{
  "run_id": "...",
  "prompt_hash": "sha256:...",
  "profile": "...", "model": "...",
  "session_id": "...",
  "status": "completed | failed | cancelled",
  "usage": {"input_tokens": 0, "output_tokens": 0, "...": 0},
  "wall_time_secs": 0.0,
  "turns": 1
}
```

## 9. 分阶段实施

(原 Step 7"reprobio-adapter 换芯"已完成后随 adapter 一并移除;`crates/headless` 现为唯一评测入口,未来的 benchmark adapter 作为它的库消费者另行开发。)

- **P0(骨架)**:`build_model` / profile 引导下沉 runtime;`crates/headless`
  库 + `run` 子命令;单 agent 单 turn;人读 + `--json`;退出码;`--timeout`。
- **P1(可用性)**:`--output-last-message`、`--session` 恢复、
  `--ephemeral`(临时 state_dir)、`--manifest`、`sessions list`。
- **P2(评测)**:`--output-schema`(依赖 agentik-sdk 结构化输出能力,见 §10.3);多 turn stdin 脚本(每行一条 user 消息的 JSONL)。
- **P3(编排)**:多 agent 网络运行——`NetworkSpec`(nodes/edges/termination)
  以 JSON 文件输入,run 至 `TerminationSpec` 满足;复用
  `host.add_node`/`connect`/`set_termination`/`inject_initial_prompts` 现有 API。

## 10. 开放问题

1. **二进制归属**:`run` 挂在现有 tui 包(包名 `tui`,二进制名有误导性)还是新建
   `apps/exec`?倾向先挂子命令验证设计,稳定后再统一 CLI 命名。
2. `--ephemeral` 的持久语义:临时 state_dir 是否与 VFS / OpenGWAS 缓存等
   共享目录冲突,需要在 P1 实测。
3. 结构化输出:agentik-sdk 的模型能力位未见 `supports_structured_output`;
   `--output-schema` 是否降级为 prompt 约定 + 本地 JSON 校验,待确认 SDK 能力。
4. headless 下容器工具的资源约束(并发容器数、单容器超时)是否需要 CLI 覆盖,
   还是完全沿用 profile 配置。
5. TUI 的 HTTP API(`tui-http`)在 headless 模式默认不启动,是否需要
   `--http` 开关供外部观测。
