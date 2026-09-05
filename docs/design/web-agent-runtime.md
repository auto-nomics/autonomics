# Web 端智能体接入 RuntimeHost 设计稿（thread ≡ agent session）

- 状态：草案（待评审）
- 日期：2026-09-04
- 前置阅读：`docs/tui-http-api_zh.md`、`docs/agent-runtime_zh.md`

## 1. 背景与现状

Web 前端与 TUI 跑的是**同一个** agentik 引擎（`agentik_core::agent::Agent`），但接线方式不同：

| | TUI 智能体 | Web agent（现状） |
|---|---|---|
| 构建路径 | `RuntimeHost`/`SharedInfra::spawn_agent`（`crates/runtime/src/host.rs:289`） | `tui-http/src/agent.rs:84` 每请求 build 的 ephemeral Agent |
| 驱动方式 | `AgentProfile`（identity / prompt / feature flags） | 前端上行的 `agent_type` + `system_prompt` |
| 持久化 | `agent.db`（snapshot + WAL 重放），重启可恢复 | 无。转写由前端整包写 `bib_meta`（`web:chat:<scope>`，上限 5 MB） |
| 记忆 | KMS / MemoryBackend | 无 |
| 工具 | 全量（vfs bash、catalog、container-dev、data-engine、host_tools 无条件；opengwas/opentargets/gwascatalog/bibliography/writing 按 flag） | 仅 `bib_all_registrations` |
| 生命周期 | 长驻 `AgentHandle`，session 可 create/switch/close/rename | 随 SSE 流结束即死 |

成因是接线约束而非引擎差异：`tui-http` 刻意零依赖 `runtime` crate；desktop 壳（`apps/desktop/src-tauri/src/server.rs`）只装配 `BibShared` + 模型槽，进程内没有 RuntimeHost，且以"TUI 是 config.db 唯一写入方"为前提。

## 2. 目标与非目标

**目标**

1. Web 端聊天走 `SharedInfra::spawn_agent`，与 TUI 同一条智能体构建路径。
2. Web 的对话单元（thread）映射为 agent 的 **session**，获得快照 / WAL 恢复、服务端 compaction、记忆。
3. 工具面可按 profile 裁剪：web 默认只开文献类，shell / 容器类不出现在 HTTP 暴露面。
4. Desktop 与 web 前端功能一致：desktop 进程装配完整 RuntimeHost。

**非目标（本期不做）**

- Web 端多智能体网络 / delegation UI（`host_tools` 中的 spawn_agent 等对 web 关闭）。
- Web 端 DAG / data-engine 面板（工具不注册即可，UI 不动）。
- 替换 `/api/v1/bib` 其余端点。

## 3. 已定决策

- **D1：thread ≡ agent session**（否决 pane ≡ agent）。
  - pane 是 UI 容器（homepage / paperReader / screening / 追问视图），生命周期挂在浏览器页签上——刷新即死、卸载回调不可靠，是整条链路里最不适合当持久化锚点的东西。
  - thread 与 session 语义同构：都是"一段对话"。恢复机制（`restore_session_state`：最新 snapshot 作基线 + 重放其后 WAL，`agentik-core/src/storage.rs:768`）现成。
  - agent 本身对 web 用户不可见，是按 `agent_type` 常驻的基础设施；pane 退化为 session 的一个视图（挂载 → 订阅 SSE）。
- **D2：desktop 跟。** desktop 进程 `RuntimeHost::open`，放弃"TUI 是唯一写入方"假设，随之引入单写者锁（§9）。
- **D3：同型串行。** agent 的 session 语义是"单一活跃 session"（`switch_session` 会暂停当前 session），一个 agent 同时只服务一个流式回合；同 `agent_type` 的并发请求排队（§8）。

## 4. 总体架构

```
┌─ web 前端（浏览器 / desktop webview，同一 dist）──────────┐
│  pane（视图）── thread（对话，前端只存元数据 + session id）│
└──────────────┬───────────────────────────────────────────┘
               │ /api/v1/agent/*（SSE）
┌──────────────▼───────────────────────────────────────────┐
│ tui-http  ── feature "runtime-host" ──► 依赖 runtime      │
│   ApiRouterBuilder.host(&SharedInfra)                     │
│   AgentRegistry: { agent_type → AgentHandle }（常驻 ×3）  │
│   thread↔session 路由 + SSE 桥（复用现有 map_agent_event） │
└──────────────┬───────────────────────────────────────────┘
               │ spawn_agent（同一入口，TUI 也走这里）
┌──────────────▼───────────────────────────────────────────┐
│ runtime::SharedInfra                                      │
│   agent.db（snapshot+WAL）· memory/KMS · bib · vfs · …    │
└──────────────────────────────────────────────────────────┘
```

- `tui-http` 新增 cargo feature `runtime-host`（默认关，保持"最小可嵌入宿主"的旧用法可编译）。开启后 `ApiRouterBuilder` 新增 `.host(infra: runtime::SharedInfra)`；未提供 host 时维持现有 ephemeral 端点不变（降级路径）。
- TUI 侧接线改动：`apps/tui/src/app/terminal.rs::start_http_server` 把 `host.infra()` 整个传入，而不再只传 `infra().bib`。

## 5. API 设计

SSE 事件契约（`delta` / `tool` / `done` / `ping` / `error`，含 10 s ping 与 500 字符工具结果预览）**保持不变**，前端 `streamParser` 不动。

新端点（挂 `/api/v1/agent`，受现有 bearer auth 保护）：

| 方法 & 路径 | 语义 | 映射 |
|---|---|---|
| `POST /threads` | 建 thread。body: `{ agent_type, title? }` → `{ thread_id }` | `AgentHandle::create_session` |
| `GET /threads/:id/messages` | 拉全量转写（UI 重 hydration） | 读 session log（`SharedInfra.storage`） |
| `POST /threads/:id/chat` | 发言并流式返回。body: `{ message, context? }` | turn 锁 → `switch_session` → `MessageInject` → SSE |
| `PATCH /threads/:id` | 改名 | `rename_session` |
| `DELETE /threads/:id` | 关闭并删会话 | `close_session` |
| `GET /agent` | 模块能力描述（`{ mode: "runtime" \| "ephemeral" }`） | 前端据此选路径 |

关键语义：

- **前端不再上行 `messages[]` / `system_prompt`。** 历史由服务端 session 持有；静态 persona 归 profile（§6），动态上下文走 `context` 字段（§7）。
- 旧 `POST /chat` 保留为兼容端点（ephemeral 路径），P2 前端切换完成后标记废弃，P3 删除。
- `GET /threads/:id/messages` 需要在 `AgentStorage` 上补一个"按 session 列消息"的读接口（现 trait 有 snapshot / session-log 组件，缺面向 HTTP 的列表查询；实现于 turso_storage，走 WAL 表）。

## 6. Web profiles 与工具面裁剪

三个内置 profile，首次启动时由 runtime 写入 profile registry（已存在则跳过）：

| profile path | identity 来源 | flags |
|---|---|---|
| `web/homepage` | 现 `identity_for` 默认支路 + 前端 `CORE_PERSONA_RULES` 归并 | bibliography=on，其余 off |
| `web/paper_reader` | 现 `paperReader` 支路 | 同上 |
| `web/screening` | 现 `screening` 支路 | 同上 |

Persona 收归后端单一真源：前端 `DEFAULT_PERSONAS` / `systemPromptBuilder` 里的静态 persona 段移除（动态层保留，见 §7）。`build_system_prompt` 的 tool_guidance 机制顺带解决"persona 宣称的工具与实际注册不符"问题。

**工具面开关（本设计的必要前置）**：`tools_from_profile` 现状里 vfs bash、catalog、container-dev、data-engine、host_tools 是**无条件注册**的。给 `AgentProfile` 新增 flags：

```rust
#[serde(default = "default_true")]
pub enable_vfs_shell: bool,      // 现 vbash_registrations
#[serde(default = "default_true")]
pub enable_container_dev: bool,  // container_dev_registrations
#[serde(default = "default_true")]
pub enable_data_engine: bool,    // data_engine_tools::registrations
#[serde(default = "default_true")]
pub enable_host_tools: bool,     // host_tools（spawn_agent/delegate 等）
```

`default = true` 保证存量 TUI profile 反序列化后行为不变（默认全开）；web 三个 profile 显式置 false。这是把"工具面"从硬编码提升为 profile 可控，TUI 也因此获得细粒度配置能力。

## 7. 会话与持久化

**真源转移**：转写唯一真源从前端 `bib_meta` 移到 `agent.db`（snapshot + WAL）。

- 前端 thread 元数据（id、标题、agent_type、创建时间）仍存前端现有位置，另附 `session_id`；不再整包写转写（`POST /api/v1/bib/chat` 的 chat 用途退役，端点保留给非 chat 的 meta 用途）。
- 前端 `messageCompaction`（token 预算裁剪）在 session 模式下删除——服务端 `AgentHandle::compact` 才是权威；后续可加 `POST /threads/:id/compact` 端点（P4）。
- **动态上下文**（paperReader 当前打开哪篇论文）：只有前端知道，改为每回合 `context` 字段，注入为该 turn 的上下文块（渲染进 user turn 前导，例如"【当前文献】…"），不再冒充 agent 级 system prompt——agent 级 prompt 现在是常驻的、跨 session 共享的。
- **存量迁移（可选，P4）**：一次性导入——遍历 `bib_meta` 的 `web:chat:<scope>` 键，按 scope 归型建 session，把旧 payload 里的消息重放进 session log。不做也不阻塞上线：旧 thread 继续以只读旧格式展示，新对话走新链路。

## 8. 生命周期与并发

- **常驻 agent 惰性启动**：首次某 `agent_type` 的请求到达时 `spawn_agent`（profile 取自 registry），存入 tui-http 的 `AgentRegistry`。进程内最多 3 个，idle 时阻塞在 channel recv 上，零 CPU 成本。
- **无 GC 问题**：这是 D1 相对 pane ≡ agent 的核心收益——agent 不随 UI 生灭，刷新 / 关标签 / 崩溃都只是 SSE 断开，session 状态无损；session 是 db 行，没有运行时成本。
- **同型并发（D3）**：每个 agent 一把 turn 锁。同 `agent_type` 的第二个流式请求排队等待（上限 1 个排队位，超出返回 409 `agent_busy`，前端提示"该助手正在回复另一对话"）。不同 `agent_type` 天然并行（homepage 与 paperReader 同时流式没有问题）。
- 进程退出：随现有 tui-http graceful shutdown 路径，先断 SSE 再收 agent（与 TUI 现有退出序一致）。

## 9. Desktop 接入（D2）

`apps/desktop/src-tauri/src/server.rs` 从"BibShared + config.db 读一次模型槽"改为 `RuntimeHost::open` + `.host(host.infra())`：

1. **模型槽来源**：改用 `SharedInfra` / `AgentHandle::model_handle`（`Arc<ArcSwapOption<Model>>`），TUI 内换模型或 config 变更后的热更新顺带成立；删除"启动时读一次 config.db（TUI 是唯一写入方）"的假设与代码。
2. **单写者锁**：TUI 与 desktop 可能同时运行，两个进程开同一个 `agent.db` / `bib.db`。在 `state_dir` 放 advisory 文件锁（`flock`，`runtime.lock`），`RuntimeHost::open` 时获取；拿不到锁的第二个实例直接报错退出，desktop 弹窗提示"Autonomics 已在 TUI 中运行"。不尝试多进程写并发——SQLite/Turso 层面的双写一致性问题面远大于收益。
3. **启动重量**：`SharedInfra::open` 是全量打开（engine manager、container infra 等）。container/k3s 客户端本身惰性连接，可接受；若实测启动耗时明显，再评估按需拆分，不预做。
4. desktop 与 TUI 的差异只剩 UI 壳（terminal vs webview），后端能力完全一致。

## 10. 阶段划分

| 阶段 | 内容 | 验收 |
|---|---|---|
| P1 后端 | `AgentProfile` 新 flags；web 三 profile 注册；`tui-http` feature `runtime-host` + 新端点 + AgentRegistry + turn 锁；TUI `start_http_server` 传 infra | curl 走通 thread 全生命周期；TUI 原功能回归（默认 flags 不改变其工具面） |
| P2 前端 | `useChatSender` 切 `/threads` API；删 messages 上行与静态 persona；thread 元数据附 session_id | 三种 pane 流式正常、刷新后 `GET /messages` 恢复 |
| P3 Desktop + 收尾 | desktop `RuntimeHost::open` + 单写者锁 + 模型槽改造；旧 `POST /chat` 删除 | desktop 与 TUI 互斥提示正确；模型热更生效 |
| P4 可选 | 存量 thread 迁移导入；`compact` 端点；delegation / 多智能体 web 化预研 | — |

P1/P2 可并行开发（新端点与旧端点并存），P3 依赖 P1。

## 11. 风险与开放问题

- **`GET /threads/:id/messages` 的读接口**需要动 `AgentStorage` trait + turso 实现，注意 WAL 表的索引与量级（长会话消息数上万时列表查询性能）。
- **session 语义边界**：`switch_session` 的"暂停当前 session"在异常路径（turn 中途断流后切走再切回）下的快照一致性需要测试覆盖；这是复用 agentik 机制的主要回归风险点。
- **锁的 UX**：单写者锁对"忘了 TUI 开着"的用户是新的失败模式，报错文案要指路（"关闭 TUI 后重开 desktop"）。未来若确有双开需求，再评估 desktop 只读模式。
- **web profile 的演进权**：`web/*` profile 与前端 strategy 语义耦合（persona 文案、上下文格式），改其一要同步另一个。建议在这份文档落地后，把三个 persona 文案的唯一真源定为 profile registry，前端彻底不再内置文案。

## 12. 实现状态与偏差记录（P1/P2 已落地）

**P1（后端）**：`crates/agentik-core/src/storage.rs`（4 flags，`serde(default)` 兼容存量行，零 SQL 迁移）、`crates/runtime/src/host.rs::tools_from_profile`（按 flag 门控；kms_readonly 仍无条件注册——存量残留，待清理）、`crates/tui-http/src/agent_runtime.rs`（AgentRegistry + 线程端点 + turn 串行）、`crates/tui-http/src/server.rs`（`GET /api/v1/agent` 模式探测）。`GET /threads/:id/messages` 复用了已有的 `get_transcript_messages`，未新增存储读接口。

**P2（前端）**：`apps/web/src/features/ai-chat/api/agentThreadsApi.ts`（探测/映射/CRUD）、`useChatSender`（runtime 分支）、`useChatInit`（水合兜底）、`ChatPanel`（模式状态接线）、`systemPromptBuilder`（`excludePersona`）。

与 §7/§8 的偏差，均已按更简单的方案实现：

1. **同型并发**：§8 写"排队 1 位"；实现为**直接 409 `agent_busy`**（前端提示"该助手正在回复上一条消息"）。turn 锁由 handle mutex 担任，driver 任务在 SSE 客户端断开后仍排空到终态事件才释放——既防旧事件泄漏到下一 turn，也免掉排队状态机。前端输入框在 streaming 期间本就禁发，409 只兜并发窗口。
2. **映射键**：§7 暗示 scope 单键；实现为 **`(scope, agent_type)` 二元组**（localStorage `agentThreadSession:{scope}|{agentType}`）——paperReader 与 screening 可能对同一篇论文各持会话，它们是不同 resident agent、transcript 分库存储，scope 单键会串会话。
3. **双写保留**：§7 写"不再整包写转写"；P2 保留 `persistMessages`（bib_meta 照存 UI 全量 payload，含追问线程）。理由：水合兜底、旧端点降级、P4 迁移都以 bib_meta 为锚；双写让 runtime 模式随时可回退。退役留给 P3/P4。
4. **采用规则**：仅**新对话（UI 无历史）或已有映射**的主对话走 runtime；bib_meta 存量对话（有历史、无映射）继续 ephemeral，历史不凭空丢。inline 追问线程 P2 一律 legacy（其转写依赖 UI 结构，session 化是 P4 课题）。
5. **自定义 persona**：用户在 /prompt 弹窗设置的 persona 是会话级覆盖，走每轮 `context` 上行（`excludePersona` 只排除 agentType 默认文案），服务端 profile 恒为默认。
6. **模式探测降级**：`probeAgentMode` 任何失败（网络/非 JSON/无 mode）→ `'ephemeral'` → 旧端点。desktop（P3 前）与旧 TUI 进程因此永远走旧链路，无需版本协商。
