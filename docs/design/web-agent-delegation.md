# Web 端 Delegation / 多智能体化设计稿（P5 预研）

- 状态：草案（待评审；本文档是预研产出，不含已落地代码）
- 日期：2026-09-05
- 前置阅读：`docs/design/web-agent-runtime.md`（P1–P4：thread ≡ agent session、web profiles、runtime-host）
- 范围：让 web 端**看到**并（受限地）**使用** agentik 的多智能体 delegation 机制

## 1. 背景：现有机制盘点（事实清单）

多智能体引擎在 runtime/agentik 层已完整，web 只是没接线：

| 机制 | 位置 | 现状 |
|---|---|---|
| host 工具面 | `crates/runtime/src/host_tools.rs` | 13 个工具：`spawn_agent`（子路径派生 `/root/web/homepage/worker`）、`delegate_to`（Async、24h 超时）、`send_message`（fire-and-forget）、`route_task`/`list_agents`/`get_agent_info`（能力发现）、`list_delegations`/`get_agent_history`（事后审计）、`shutdown_agent`/`interrupt_agent`、`derive_profile`。拓扑类（connect/reset）半禁用——协作是 delegate 驱动 |
| 控制通道 | `crates/runtime/src/control.rs` | `HostControl` 可克隆 handle；查询类命令（GetStatus / ListDelegations / GetAgentHistory / ListPersistedAgents）带 oneshot 回复。**`SharedInfra.host_control` 是公开字段（`host.rs:110`）——tui-http 今天就能直接查** |
| 事件广播 | `host.rs:1033` `HostEvent` | `AgentRegistered` / `AgentUnregistered` / `AgentStatusChanged`（含 live 状态五态：idle/running/awaiting_tool/completed/failed）broadcast，可订阅 |
| delegation 台账 | `agentik-core/src/storage.rs:655` | `upsert_agent_delegation` / `list_agent_delegations`：delegation_id、caller/target、task、五态（pending/running/completed/interrupted/failed）、turn_id、**session_id**、response、时间戳，持久化在 agent.db |
| agent 图 | `HostCommand::ListPersistedAgents` | `agent_graph` 表跨进程存活（上次运行遗留的 agent 也在） |
| web 侧门控 | `tui-http/src/agent_runtime.rs:635` | web 三 profile `enable_host_tools = false`（`host.rs:471` 按 flag 注册工具） |
| web agent 隔离 | `agent_runtime.rs:102` | 常驻 web agent 驻 `/root/web/{name}` 子树，TUI picker 不可见 |
| SSE 工具帧 | `tui-http/src/agent.rs:68/80/96` | `ToolCall`/`ToolResult`/`ToolCallBackground` 已映射成 `tool_call_start`/`tool_call_result`/后台帧，前端 `streamParser.ts` 已消费（含 running 态清扫） |

推论：**只要给某个 web profile 开 `enable_host_tools`，delegation 在 web 聊天里立即以工具卡片形式可见**（parent 的 tool 帧）；但引擎的三个无边界点（§5）使得直接开闸不安全。

## 2. 目标与非目标

**目标**

1. **观测优先**：web 能看到 host 里有哪些 agent（含 web 常驻三剑客 + 任何派生 child）、delegation 台账与结果、child 的转写。
2. **受限使用**（二期）：单一 web profile 可 `delegate_to` / `spawn_agent`，带硬边界（子树、上限、profile 白名单）。
3. 复用既有持久化与 SSE 契约，不新增存储表、不改 `streamParser`。

**非目标（维持 web-agent-runtime §2，本稿重申）**

- Web 端 DAG / 拓扑面板（`agentik_network` 的 connect/termination 那套）——delegate 驱动已覆盖场景。
- Web 端手动 spawn UI（用户点按钮选 profile 生 agent）——child 由模型经工具自行派生，web 只观测。
- child agent 的 live SSE 订阅（delta 级实时流）——先靠台账 + 转写事后读，成本收益不成比例。

## 3. 已定决策（待评审）

- **D1：P5a 只读观测先行，P5b 受限 delegation 其后。** 只读端点零安全面扩张、零行为变更；把「web 上看不到 agent 在干嘛」这个最痛的点先解决。
- **D2：读路径直接经 `SharedInfra.host_control` 查询命令**，不在 tui-http 侧复制注册表状态。真源单一（RuntimeHost 内存态 + agent.db 台账）。
- **D3：live 更新用 `HostEvent` broadcast → SSE**，不做前端轮询。（一期可先无 live，刷新拉取即可——端点已够。）
- **D4：P5b 的边界做在 host 层（Spawn/resolve 入口校验），不做在工具描述里**——提示词不是安全边界。

## 4. API 设计（P5a 只读，挂 `/api/v1/agent`，runtime-host feature 门控 + bearer 保护同 threads）

| 方法 & 路径 | 语义 | 映射 |
|---|---|---|
| `GET /agents` | 活跃 agent 列表（含 status/last_event/tools）+ 持久化 agent 图合并 | `HostControl::get_status` + `list_persisted_agents`（按 path 去重，live 态优先） |
| `GET /delegations?status=&target=&limit=` | delegation 台账，newest first | `HostControl::list_delegations(None, ..)`（**不按 caller 过滤**——web 是宿主视角，不是 agent 视角） |
| `GET /agents/{name}/history?limit=` | agent 转写（默认 20，上限 100） | `HostControl::agent_history` |

- `GET /agent` 模式探测不变（`mode:"runtime"` 已蕴含这些端点存在；`ephemeral` 宿主不挂载，前端 404 即隐藏 UI）。
- 响应形状直接序列化 `AgentInfo` / `DelegationSnapshot` / `AgentExecutionHistory`（serde 已就绪，`snake_case`）。

## 5. 安全边界：直接开闸的三个洞 + 对策（P5b 前置）

1. **短名解析跨子树**（`host.rs:1840` `resolve_agent`）：short-name 对全注册表匹配——web agent 可 `delegate_to`/`shutdown_agent`/`get_agent_history` 到 TUI 侧 agent（如 `/root/researcher`）。
   对策：`resolve_agent` 加可选 caller 子树约束（`HostCommand` 已携带 caller 信息的命令按 caller path 前缀过滤候选）；或 host_tools 构建时给 web agent 注入 `subtree: "/root/web"`，工具层强制前缀校验（D4 要求做在 host 层）。
2. **Spawn profile 解析是全局的**（`host.rs:1255` 附近）：`profile_segment` 含 `/` 按绝对路径解析——web agent 可实例化 TUI profile（全工具面 + host_tools 递归 = 提权）。
   对策：Spawn 命令增加「caller 子树」参数，profile 候选限制为 caller 自身 profile 及其 `web/*` 派生 profile；绝对路径仅允许白名单前缀。
3. **spawn 无上限**：模型可以无限派生 child（每个 child 是完整 agent，消耗模型槽调用）。
   对策：host 层 per-caller 子树 spawn 上限（建议 8）+ host 总量上限（建议 16，含 TUI 侧）；超限返回 Err，工具把错误文本交还模型自纠。

另有一个**行为性**（非安全）问题：`delegate_to` 是 Async 工具、24h 超时，parent turn 期间 web 的同型 turn 锁一直持有——用户侧表现为助手长时间忙碌（409）。对策（P5b 一并）：web 聊天超时提示 + 已有 `interrupt_agent` 暴露为前端「取消」按钮（`POST /agents/{name}/interrupt`，fire-and-forget）。

## 6. 前端形态（建议）

- **聊天内（零改动）**：parent 的 `delegate_to` 经既有 tool 帧渲染成工具卡片（streamParser 已支持）。
- **代理活动抽屉（P5a 新增，最小面）**：AI 面板头部「活动」入口 → 抽屉列 `/agents`（name/path/status/last_event）+ `/delegations`（task、状态、耗时）；行点击展开 `history` 转写（复用消息渲染组件）。
- **模式降级**：`probeAgentMode() === 'ephemeral'` 时不渲染入口，与 threads 同一套降级逻辑。

## 7. 阶段划分

| 阶段 | 内容 | 验收 |
|---|---|---|
| P5a 只读观测 | 上述 3 端点 + 前端活动抽屉 | TUI 侧起一个 delegation，web 抽屉可见状态流转与转写；`ephemeral` 宿主不渲染 |
| P5b 受限 delegation | §5 三对策落地（host 层）+ 选定 profile（建议 `homepage`）开 `enable_host_tools` + interrupt 按钮 + e2e（复用 `tests/migration.rs` 假 provider 手法：fake provider 脚本化触发 delegate_to） | web 聊天内 spawn→delegate→结果回流全链；越界（跨子树/超限）被拒且模型收到可读错误 |
| 非目标 | DAG 面板、手动 spawn UI、child live SSE | — |

## 8. 风险与开放问题

- **`/root/web` 子树泄漏**：`GET /agents` 是宿主视角，会列出 TUI agent——桌面/TUI 与 web 同进程共享 host（D2 之后的既定事实）。web 前端要不要按子树过滤展示（建议：全列，路径可见即透明）。
- **delegation 台账无限增长**：`list_agent_delegations` 目前无分页参数，只 newest-first；P5a 端点先做 `limit` 钳制（默认 50），清理策略（保留 N 天）留开放问题。
- **`agent_history` 依赖 agent 在册**：`HostCommand::GetAgentHistory` 只查 live agent；上次运行遗留的孤儿 agent（`agent_graph` 有、注册表无）拿不到 history——P5a 先在 `/agents` 响应里标记 `live: false`，读取留 P5b（需按 agent_id 直读存储）。
- **profile 演进权耦合**：P5b 若给 `homepage` 开 host_tools，其 persona 文案需同步声明 delegation 能力（`build_system_prompt` 的 tool_guidance 已自动生成，风险低但要过目）。
