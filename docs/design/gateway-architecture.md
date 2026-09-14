# Backend Gateway 架构设计

状态:**P0/P1/P2 已实现**(refactor/migrate-to-gateway-backend 分支):`tui serve` 常驻
daemon + gateway crate(REST+SSE)+ TUI 瘦客户端迁移完成。后续:P3 headless 走
gateway、P4 web 前端、P5 desktop(见 §11,各自独立 PR)。

前置阅读:`docs/headless-run-design.md`(§1.2 记录了「未来若出现独立 daemon 需求再
升级」——本文档就是那次升级)。

## 0. 背景与决策

headless 设计定稿时,TUI 与 headless 同进程直连 `RuntimeHost`,明确推迟了 daemon /
RPC 层。gateway 分支兑现该升级,核心动机:

- **agent 生命周期与前端解耦**:TUI 退出即杀 agent 是旧架构最大的行为缺陷——长任务
  跑一半关掉终端就丢;
- **多前端**:TUI 之外,web / desktop / headless CLI 需要同一运行时的多个并发视图;
- **单写者**:`agent.db` / `app.db` / podman 连接等进程级资源只应有一个属主。

已确认的决策:

| 决策 | 内容 |
|---|---|
| D1 进程形态 | `tui` 二进制新增 `serve` 子命令(一个二进制多前端,codex 模式),不建独立 daemon 二进制 |
| D2 TUI 定位 | 彻底瘦客户端,单一代码路径;daemon 未运行时自动拉起 `tui serve --daemon` |
| D3 传输 | v1 单 TCP loopback(`127.0.0.1:8765`,沿用 `AUTONOMICS_HTTP_API_ADDR`)+ REST(命令)+ SSE(事件)。UDS 留作后续加固 |
| D4 鉴权 | gateway API 恒 bearer 保护:`AUTONOMICS_HTTP_API_TOKEN` 或每次启动生成、写入 `<state_dir>/gateway.token`(0600)——token 文件即本地进程的文件系统权限门。bib API 维持原契约(无 env token 则 loopback 开放) |
| D5 写权归属 | app DB 的 rusqlite 连接只存在于 daemon(`gateway::model_store`);`Model` 构造(含 ChatGPT token 刷新回调)全部 daemon 侧 |
| D6 jay 分支 | 旧多前端实现(`github/jay`,基于 9/3 旧 main)只借鉴设计(flock 单写者、SSE 帧形状、thread≡session),代码在 main 上重写 |

**Headline 行为变化**:TUI 退出不再杀 agent。前端生灭只是连接断开;agent/session
在 daemon 中持久存活,重连即恢复(转写从 storage 重放,运行中 turn 的未落盘 delta
除外——与旧 TUI 崩溃等价,不劣化)。

## 1. 架构

```
┌─────────┐   REST(命令,fire-and-forget→202)    ┌───────────────────────────────┐
│ tui     │ ───────────────────────────────────▶ │ tui serve(常驻 daemon)        │
│ (瘦客户) │ ◀─────────────────────────────────── │  axum 127.0.0.1:8765          │
└─────────┘   SSE /api/v1/events                 │  ├─ EventHub(seq/重放/lag)     │
              (id=seq, Last-Event-ID 重放)        │  ├─ driver(独占 recv_next)    │
                                                │  ├─ RuntimeHost 唯一实例        │
                                                │  ├─ model_store(app DB 独占)   │
                                                │  ├─ /api/v1/bib(原样迁移)     │
                                                │  └─ flock state_dir 单写者     │
                                                └───────────────────────────────┘
```

## 2. crate 布局

```
crates/gateway   proto(wire 类型)/ hub(EventHub)/ driver(宿主泵)/ server(axum)/
                 daemon(run_daemon 启动序)/ client(GatewayClient+EventPump)/
                 manager(probe/ensure_running/stop)/ model_store(app DB)
依赖方向:apps/tui → gateway → runtime → agentik-*;gateway → tui-http(仅 bib router)
apps/tui 不再直接依赖 runtime/tui-http(gateway 重导出所需的少数类型)
```

`tui-http` 退化为 bib 模块 crate(`bib::router` 与 `frontend_router` 公开供 gateway
组合),保持零依赖 runtime 的原设计原则。

## 3. wire 协议

全部挂 `/api/v1`,bearer 保护;`/api/v1/bib` 原样 nest(契约冻结)。

### 3.1 Hydration(GET,同步 JSON)

| 路径 | 响应 |
|---|---|
| `GET /gateway/status` | `{pid, version, uptime_secs, last_seq, agent_count}`(探活端点) |
| `GET /state` | 冷启动全量包:`{last_seq, active_model_spec, profiles, agents, sessions{agent→[SessionInfo]}, display_settings, model_catalog}` |
| `GET /agents`、`GET /agents/{name}/model`、`GET /agents/{name}/dag` | AgentInfo 列表 / `{model, context_length}` / DagTuiSnapshot |
| `GET /storage/agents`、`DELETE/PATCH /storage/agents/{uuid}` | 恢复选择器(resume picker)的 list/delete/rename |
| `GET /agents/{agent_id}/sessions/{session_id}/history` | 合并转写(transcript + 压缩摘要 + 未归档 live 行)——旧 TUI 进程内拼接逻辑的服务化 |
| `GET /agents/{agent_id}/plan` | 持久化计划 |
| `GET /model-config` | providers + models + active_model + openai token 状态 |
| `GET /settings` | display 开关 |

带 `/` 的 agent 路径(如 `/root/researcher/worker`)按 `%2F` 百分号编码过路由,单个
path segment 携带完整路径。

### 3.2 命令(oneshot=同步 200;fire-and-forget=202,结果经事件流回流)

`POST /agents`(spawn,同步返回 path)、`POST /agents/{name}/messages`、
`POST .../cancel|compact|shutdown`、`PUT /agents/{name}/model`(热切换+持久化)、
`POST .../sessions`、`.../sessions/{id}/activate|close`、`PATCH .../title`、
`GET /agents/{name}/sessions`(请求 SessionList)、
`PUT /model-config/provider|active-model`、`POST /model-config/chatgpt/login|refresh`、
`POST /model-config/providers/{name}/catalog`、`PUT /settings`、
`POST /gateway/shutdown`(优雅停机)。

线上的模型一律用 spec 字符串 `provider:model`——`Model` 持有 `Arc<dyn ApiClient>`
不可序列化也不该可序列化;daemon 侧经 `model_bootstrap::resolve_model_spec` 重建并挂
token 刷新回调。

### 3.3 SSE 事件流(`GET /events`)

```
id: 10417                          ← 全局单调 seq(driver 循环内分配,全序)
event: agent | host | notice | lag  ← 15s keepalive ping
data: {"agent":"/root/x","event":{"TextDelta":"…"}}   ← AgentEvent 原生 serde
```

- **AgentEvent 直接上网线**(serde 完备),前端复用既有 typed 处理器
  (TUI 的 `state::apply_event` 纯函数零改动);`HostEvent` 无 serde → wire 镜像
  `HostEventView`;daemon 侧生命周期(notify)用 `GatewayNotice`
  (`model_changed` / `chatgpt_login` / `catalog_fetched`)。
- **重放**:重连带 `Last-Event-ID: N` → 环形缓冲(4096)回放 `(N, tail]` 续流;
  N 已被挤出 → 先发 `event: lag {"missed":n,"resume_seq":m}`,客户端全量 re-hydrate。
- **Hydration 一致性协议**:先取 snapshot(`last_seq` 在快照序列化之后从 hub 读取)
  → pump 从 `last_seq` 起(重放覆盖间隙)→ 客户端按 seq 去重。不丢不重。

### 3.4 EventHub

`seq + ring(VecDeque 4096) + broadcast(1024)` 同锁发布——seq 分配、入环、广播同
序,所有订阅者看到同一全序。**背压红线:publish 永不阻塞 driver**——慢消费者
lag 即弃帧重同步。环形缓冲只服务短窗重连,不是持久订阅日志(session 恢复走
storage)。

## 4. Driver:daemon 对宿主的独占

`RuntimeHost::recv_any()` 非纯接收:内联 delegation 记账、拓扑路由、状态派生。
daemon driver 是唯一消费者,经新增的 `RuntimeHost::recv_next()`(多路复用 agent
事件 / host 事件 / 命令 / 后台 spawn 注册,替代 TUI 旧事件循环的裸指针 select hack)。

driver 维护 session 缓存(`SessionList/SessionClosed` 事件折叠,纯函数可测)供
`GET /state` 同步应答;agent 注册时与 TurnCompleted 后按需补拉列表(见 §10 quirk)。

## 5. daemon 生命周期

- **启动序**:model store(app DB)→ `RuntimeHost::open`(持 flock,冲突→
  `InstanceLockHeld` 友好报错退出)→ profile seed + 默认模型槽 → ChatGPT
  ensure-fresh → axum bind(成功后才写 token/pid 文件,探活即用)→ driver 循环。
- **`tui serve`**:前台(stderr+文件日志,Ctrl+C=优雅停);`--daemon`:stdio 全
  null + 独立进程组(自动拉起方 `ensure_running` spawn);`status` / `stop` 子命令。
- **优雅停机**(POST /gateway/shutdown):断 SSE → `shutdown_all_agents_and_wait`
  (30s 宽限)→ 退出。
- **自动拉起**(`gateway::manager::ensure_running`):探活(1s 超时)→ 失败则
  spawn 当前 exe `serve --daemon` → 轮询就绪(15s 上限,冷启动 SharedInfra 需数秒)。
- kms/bib/cache 子命令不受影响(今天已多进程共读;flock 只围 `RuntimeHost::open`)。

## 6. TUI 瘦客户端迁移要点

- `App` 字段:`GatewayClient` + SSE pump 通道 + render 模型缓存(渲染路径禁 HTTP)
  替代 host/handles/http_server/conn;事件循环六源→四源;
- spawn/resume/session CRUD/cancel/compact/DAG 全部走 client;回复照旧以事件回流,
  语义与进程内时代一致;
- model_config 重写为薄客户端:目录/凭据写经 `/model-config`;ChatGPT OAuth
  (回调 listener、token 落库、启动刷新)整体迁 daemon;
- Ctrl+C 两段语义保留;退出仅断连并提示 daemon 仍在运行;
- lag/重连 → 从 `/state` reconcile(补新 agent、折叠 session 列表、空 tab 拉转写)。

## 7. 多前端并发语义

- 每 agent 单活跃 session、单流式 turn,消息投递即入队——引擎原语义,无需跨进程
  turn 锁;所有 viewer 平等收同一全序流;一个前端 cancel,其余前端可见;
- web 面(P4)将加 per-agent turn 锁:并发 chat 请求 409 `agent_busy`(借 jay
  P3 结论:直接 409 不排队,前端流式期间本就禁发);
- 事件全量 fan-out;web 的 per-thread SSE 是同一流的过滤+映射视图,无第二真源。

## 8. 鉴权与网络面

- gateway API 恒 bearer(env token 或 token 文件);
- bib API + 内嵌前端维持 `tui-http` 契约(env token 才启用鉴权);
- 无 CORS 头——浏览器跨源读被 SOP 拦截;SPA(P4)由 gateway 同源托管,token 在
  serve index 时注入;
- 传输层 UDS、token 会话化留 P5 加固评估。

## 9. 测试策略

- gateway 单测:serde round-trip(wire 契约锁定)、hub 全序/重放/lag、session 缓存
  折叠;
- gateway 集成测试(可靠性基石,mock model 驱动):真实 axum + 真实 client 全链
  spawn→deliver→TurnCompleted→重放→优雅停机,bearer 门控;隔离配置与 headless
  测试同法(全部 DB 路径重定向到 temp dir);
- 真实二进制冒烟:冷启动自动拉起、TUI 死亡 daemon 存活(同 pid)、重连、优雅停止。

## 10. 已知 quirk 与约束

- **mid-turn ListSessions 被吞**:session 循环的 `apply_internal_event` 把会话管理
  internal 事件当防御性 no-op(旧 TUI 时代即存在,靠「空闲时再请求」掩盖)。
  driver 因此在注册时与 TurnCompleted 后补拉列表,而非 turn 中。修复该 quirk 需要
  动 `run_session` 的事件重排语义,留独立 PR。
- **`tui run` 过渡期冲突**:默认 in-process 模式与运行中 daemon 抢锁,报错指向
  `tui serve stop` / `--ephemeral`;P3(headless 走 gateway)彻底消除。
- podman 子进程 stdin 约束随 `SharedInfra` 转移到 daemon(daemon 全程 null stdio);
  `tui run --ephemeral` 永久保留进程内路径(benchmark 隔离硬需求)。

## 11. 后续 PR 规划

| 期 | 内容 | 要点 |
|---|---|---|
| P3 headless | `tui run` 默认走 gateway | spawn(`headless-<uuid8>`)→ SSE 过滤 agent 帧 → `TranslationState::translate`(提 pub)→ RunEvent 契约逐字节不变;`--session` 等 SessionActivated;超时→cancel+合成 turn.failed;退出码 0/1/2/3 不变;`--ephemeral` 保留进程内 |
| P4 web | `apps/web`(Vite+React19+TS+pnpm) | thread≡agent session;threads/chat SSE(delta/tool/done/ping/error 帧,fetch 流解析——EventSource 带不了 Authorization);409 agent_busy;dist 由 gateway 内嵌托管,同源 token 注入;活动抽屉(agents/delegations 只读) |
| P5 desktop | Tauri 2 纯壳 | ensure_running + 读 token + webview 指 `127.0.0.1:8765/#token=…`(hash 传 token);不自开 RuntimeHost,无第二写者;可选 UDS listener 加固 |
