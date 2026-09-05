# Web 端 Delegation / 多智能体化设计稿（P5 预研）

- 状态：P5a（§9）/ P5b（§10）/ P5c（§11）均已落地
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
- child agent 的 live SSE 订阅（delta 级实时流）——先靠台账 + 转写事后读，成本收益不成比例。（P5c-5 部分解除：轮粒度 live 输出已落地，delta 级仍非目标，见 §11.4。）

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
| P5c 收尾 + 补强 | §10.5 tool_result 乱序修复（agentik toolset）+ §8.2/8.3 收口（台账保留 / 孤儿 history 边界）+ spawn 上限 e2e + child live SSE（轮粒度，§11） | 混合工具批结果序单测；终态 14 天清扫单测；9 派第 9 拒全链 e2e；`/agents/events` 注册帧 HTTP 级测试 + 前端 SSE 流解析单测 |
| 非目标 | DAG 面板、手动 spawn UI、child live SSE（delta 级；轮粒度已由 P5c 交付） | — |

## 8. 风险与开放问题

- **`/root/web` 子树泄漏**：`GET /agents` 是宿主视角，会列出 TUI agent——桌面/TUI 与 web 同进程共享 host（D2 之后的既定事实）。web 前端要不要按子树过滤展示（建议：全列，路径可见即透明）。
- **delegation 台账无限增长**：`list_agent_delegations` 目前无分页参数，只 newest-first；P5a 端点先做 `limit` 钳制（默认 50），清理策略（保留 N 天）留开放问题。（P5c-3 已落地：终态 14 天保留，见 §11.3。）
- **`agent_history` 依赖 agent 在册**：`HostCommand::GetAgentHistory` 只查 live agent；上次运行遗留的孤儿 agent（`agent_graph` 有、注册表无）拿不到 history——P5a 先在 `/agents` 响应里标记 `live: false`，读取由 P5c-2 落地（持久化图回落 + 子树边界，见 §11.2）。
- **profile 演进权耦合**：P5b 若给 `homepage` 开 host_tools，其 persona 文案需同步声明 delegation 能力（`build_system_prompt` 的 tool_guidance 已自动生成，风险低但要过目）。

## 9. P5a 实现状态与偏差（已落地）

代码：`crates/runtime/src/host.rs`（`spawn_driver` / `HostDriver` / `try_process_registrations` / `read_agent_history` 提 pub）、`crates/tui-http/src/agent_runtime.rs`（三端点 + 合成 live 行 + 测试）、`apps/desktop/src-tauri/src/{server,state,lib}.rs`（驱动接入）、前端 `agentActivityApi.ts` / `AgentActivityDrawer.tsx` / `ChatPanel.tsx`（runtime 门控入口）。与 §4/§6 的偏差：

1. **补了设计稿没预见的驱动缺口（HostDriver）**：`RuntimeHost` 是调用方驱动的——`HostControl` 查询命令只在 `try_process_commands` / `recv_any` / `recv_and_process_command` 里被处理，TUI 事件循环在驱动，桌面壳与测试都没人驱动，命令直接饿死（oneshot 回复永不触发）。P1–P4 没踩到是因为线程端点全部走 `AgentHandle` 直连方法。新增 `RuntimeHost::spawn_driver() -> HostDriver`：后台轮询循环（非阻塞排空命令/后台注册/生命周期通知 + `timeout(100ms, recv_any)`——recv 后处理全程同步，超时不会截断半处理事件），`join()` 收回 host 保住桌面停机序（停 HTTP → 停驱动 → 优雅收 agent → drop 锁 → drop runtime）。桌面接入时顺带修复 `state.rs` 既有 bug：第二个 `runtime.take()` 永远拿到 None，agent 优雅停机从未执行过；无人消费的 `notify` unbounded 通道也由驱动排空（此前桌面进程长跑会无界增长）。
2. **常驻 web agent 不进 host 注册表**：`register_agent` 会把 handle move 进 relay 任务（事件流归 host），而 web agent 的事件流归线程 SSE driver 所有，二者互斥。因此 `/agents` 的 live 行由 tui-http 合成（registry 快照 + busy → running/idle 粗粒度状态），`/agents/history` 对常驻 agent 经 registry 解析（全路径/短名/agent_type 键）后 `read_agent_history` 直读存储（该函数因此提为 pub），其余走 `HostControl::agent_history`。§8 第 3 条"孤儿 agent 拿不到 history"对常驻 agent 不再成立。
3. **history 路径形态**：§4 写 `GET /agents/{name}/history`（路径段），实现为 `GET /agents/history?agent=…`——agent 引用是含斜杠的完整路径，路径段需要编码纠缠。
4. **前端入口位置**：§6 建议"AI 面板头部"；三个宿主页面（homepage / screening / paper reader）没有共享头部组件，实现为 ChatPanel 面板右上角悬浮按钮（锚在外层容器——消息列表是滚动容器，放里面会随内容滚走），同样仅 `mode === 'runtime'` 渲染。转写展开是抽屉内的轻量 {role,text} 渲染而非复用完整消息渲染组件（依赖过重，v1 取舍）。
5. **测试隔离事故（本阶段发现并修复）**：`RuntimeConfig::default()` 在构造时即从 `$HOME`/env 解析 `agent_db` 等派生路径，事后只改 `state_dir` 字段不会重派生——tui-http 的两处测试配置（P4 遗留写法）一直在读写用户真实 `~/.autonomics/agent.db`，留下单个测试 agent（`/root/web/homepage`）名下的会话/转写行与 `web/*` profile 种子行。已改用 builder 构造让全部派生路径落 tempdir；真实库的存量测试行待用户确认后清理。

## 10. P5b 实现状态与偏差（已落地）

代码：`crates/runtime/src/host.rs` + `control.rs`（`resolve_agent_for_caller` 子树边界 / `check_spawn_caps` / `RegisterProfile` / spawn-注册顺序契约）、`crates/runtime/src/host_tools.rs`（工具带 caller_path）、`crates/tui-http/src/agent_runtime.rs`（homepage v3 persona + 迁移 + `POST /agents/interrupt`）、前端 `ChatPanel.tsx` 停止按钮 + `agentThreadsApi.ts` `interruptAgent`。e2e：`crates/tui-http/tests/delegation.rs`（真 TCP + 真 `AnthropicApiClient` + 标记脚本化假 provider，覆盖 §7 验收两条主线）。与设计的偏差与发现：

1. **`host.profiles` 缓存缺口（RegisterProfile 补丁）**：Spawn 命令按 `profile_segment` 解析子代理 profile 时查的是 host 内存缓存（`set_profiles` 只在 TUI 启动时灌入），web 壳惰性播种 `web/*` profile 行后缓存里没有——工具驱动的 spawn 一律 "Spawn failed"，且新起 host 进程对着已有 config.db（行在、缓存空）同样失败。补 `HostCommand::RegisterProfile`（fire-and-forget upsert），`ensure_web_profile` 每条返回路径都镜像进缓存；行本身仍归 profile 存储，缓存只是 Spawn 时刻的查找面。
2. **spawn→delegate 注册时序竞态（e2e 抓出的真 bug）**：后台 spawn 任务先回 Ok（工具结果随之回流，父代理下一轮全在进程内、亚毫秒）再把 handle 排进注册通道，而驱动循环每轮先排空命令再排空注册——快模型 spawn 后立刻 delegate_to 会撞上"not registered"。修复为顺序契约：spawn 任务先 `reg_tx` 后 `reply_tx`，驱动循环（含 `recv_and_process_command` 的命令分支）先排空注册再处理命令。因果链保证：依赖命令入队时，注册必已在通道里。
3. **interrupt 的接线形态**：§5 只说"中断按钮"。常驻 web agent 不进 host 注册表（见 §9.2），`HostControl::interrupt_agent` 看不见它们——线程 SSE driver 每轮发布 per-turn `CancellationToken`（`current_cancel` 槽，轮终清除），`POST /agents/interrupt` 翻 token；端点不能排队等轮锁（会与被中断的轮自死锁），所以匹配走 registry 派生路径而非 handle 锁。host 在册 agent 仍走 `HostControl`（caller None，web 用户即宿主操作员）。前端仅 runtime 模式且流式中渲染"停止"；中断后流以终端帧自然收尾。
4. **homepage v2→v3 迁移策略**：老库的 `web/homepage` 行要拿到 delegation 能力，但不能覆盖用户改过的 persona。以"系统提示词仍等于 v2 种子全文"为未编辑判据：命中则一次性刷新为 v3 文案 + `enable_host_tools`（幂等，落库）；用户改过的行保持内容且不开工具（日志提示可在 profile 编辑器手动 opt-in）。
5. **并行工具的 tool_result 乱序（agentik 既有缺口，仅记录）**：同一助手消息里多个工具调用时，tool_result 按完成序回填，可能与 tool_use 顺序错位（Anthropic 协议要求两者对应）——e2e 曾以 `delegate_to + wait_task` 同轮捆绑触发（wait 抢先执行拿到 "no background task"）。真实模型看到 "Task #N" 后下一轮才 wait，脚本同构即可绕开；修复留待 agentik 侧统一（结果应按 tool_use 顺序配对）。
6. **spawn 上限（§5.3）**：`check_spawn_caps` 在 Spawn / SpawnWithProfile 两处命令入口前置检查（总量 16 + 每 caller 子树 8，超限返回可读错误），因为 spawn 本身在后台任务里，事后拒绝只能走 reply 错误路径。

## 11. P5c 实现状态与偏差（已落地）

P5b 开放问题收尾 + 三项补强。代码：`crates/agentik-core/src/tools/toolset.rs`（结果按 tool_use 顺序回填）、`crates/agentik-core/src/storage.rs` + `storage/turso_storage.rs`（`purge_agent_delegations`）、`crates/runtime/src/host.rs`（孤儿 history 子树边界 / 台账清扫 / `HostEvent::AgentOutput`）、`crates/tui-http/src/agent_runtime.rs`（`GET /agents/events` SSE 端点）、前端 `agentActivityApi.ts` / `AgentActivityDrawer.tsx`（订阅 + 行内实时更新）；e2e `tests/delegation.rs` 增 spawn 上限用例。与设计的偏差与决策：

1. **tool_result 乱序修复落在 agentik toolset（关闭 §10.5）**：`Toolset::execute()` 改为按 tool_use 槽位回填——结果向量以调用序建槽，三类出口（同步等待完成、后台任务转 pending、未知工具/参数校验失败）都写回自己的原槽位，最后按槽序组装。混合批（async + unknown + sync）单测锚定顺序不变式；真实模型的并行工具批从此不再与协议错位。
2. **孤儿 agent history 补齐（关闭 §8.3）**：`GetAgentHistory` 注册表未命中时回落持久化 agent 图，按 agent_id 直读存储。回落路径同样过子树边界：沙箱内 caller（`/root/web` 子树）读子树外孤儿返回可读错误（"outside your sandbox"），宿主级调用（HTTP 面 / TUI）不受限——与 §5.1 的 resolve 边界同构。
3. **台账保留（关闭 §8.2）**：`purge_agent_delegations` 只清终态（completed/interrupted/failed）且 updated_at 早于 14 天的行；driver 启动即扫一次，之后每 6 小时。非终态行永不清——跨进程重启的 inflight 台账依赖它续状态。清扫 best-effort：失败只告警，不打断驱动循环。
4. **child live SSE（部分解除 §2 非目标第 3 条）**：
   - **粒度**：`AgentEvent::LlmResponse` 一帧 = 每模型轮完整文本，非 TextDelta 增量。host 中继本来就在轮粒度消费事件流，delta 级需要 per-agent 流分流（与 §9.2 的线程 SSE driver 所有权冲突），成本收益判断对 delta 级依然成立。
   - **通道**：`HostEvent::AgentOutput` broadcast → `GET /agents/events` SSE（`registered`/`unregistered`/`status`/`output` 四帧 + keepalive）。慢消费者落后 broadcast 环只告警不断流，客户端靠重拉 `/agents` 重同步。
   - **范围**：只有 host 在册 agent（TUI 侧 + 工具派生 child）。常驻 web agent 的事件流归线程 SSE driver（§9.2 的互斥不变），它们的输出本来就流经聊天流本身。
   - **前端**：`subscribeAgentEvents` 用 fetch + ReadableStream 手解 SSE 线格式——EventSource 带不了 Authorization 头。断流 3s 退避重连；重连成功先发本地合成 `sync` 帧，抽屉收到后重拉快照补基线（增量流有缺口）。抽屉对 registered/unregistered/status 就地更新行，output 追加进展开行的实时输出缓冲（封顶 8000 字符防长会话膨胀）。台账无实时事件（HostEvent 不含台账变更），仍靠手动刷新。
5. **spawn 上限 e2e（§5.3 验收）**：假 provider 脚本连派 9 个子代理，第 9 个被 per-caller 子树上限（8）拒绝，错误文本经 tool_result 回流模型后收敛终答；`/agents` 恰 8 个 live child、provider 净荷证明拒绝发生在 8 次成功之后。剧本编写陷阱（后来者免踩）：canned 回复的 tool_use id 必须每轮唯一——会话按 tool_use_id 全局去重 tool_result，复用 id 会让真实结果被当重复丢弃、留下未应答 tool_use 触发 sanitize 桩（"Tool execution has been interrupted"），模型陷入无限重派。

