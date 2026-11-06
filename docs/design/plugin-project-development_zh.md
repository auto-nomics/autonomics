# 插件项目与容器开发

插件项目以 daemon 管理的单个插件工作区为单位。一个项目对应一个 Git 仓库和一个 `manifest.toml`，其中声明的多个节点共享插件镜像。项目开发内容和已激活的运行快照是不同的：修改工作区不会自动改变已注册节点。

## 开发界面（已实现）

**插件管理**（工作台活动栏 → 插件管理）：

- 已安装插件区（`GET /api/v1/plugins`）：状态、镜像、节点 kind、「已注册」徽标；支持回滚与卸载（工作区与快照保留）。
- 开发项目区（`GET /api/v1/plugin-projects`）：打开开发窗口。
- 新建插件：绑定已批准环境（`POST /api/v1/plugin-projects`，走 `create_plugin_scaffolded`）——生成可运行的脚手架（demo 节点 + `scripts/demo.sh` + README），创建后直接进入开发窗口。agent 工具创建的插件保持裸 manifest。

**开发窗口**（editor kind `plugin-dev`，VSCode devcontainer 式）：

- 三区布局：文件树 | 代码编辑（Ctrl+S 保存、`original` 乐观并发 409）| 底部 dock（终端 / 试跑 / 报告）。
- 容器会话：打开窗口时探测 `GET .../dev-session`，absent/stopped 则自动 `POST {"action":"start"}`（attach 语义）。容器是 daemon 侧长驻的 `autonomics-dev-<plugin>`（label `io.autonomics.dev-session`），窗口关闭不影响；操作条提供 重启容器 / 停止。
- 终端：xterm.js over `GET .../terminal`（WebSocket，同源会话 cookie 认证）；二进制帧即 pty 流；shell 退出显示「重开 shell」。podman CLI 不支持 exec resize，resize 仅提示重开。
- 校验 / 激活：`POST .../validate`、`POST .../activate`（六门校验 → 本地快照 → 热注册，节点以 `plugin/node` 进入 DAG）；报告在 dock 渲染各门 pass/fail。
- 试跑：`POST .../nodes/{kind}/test-run`，按声明的 file 入口端口填样例文本（参数为 JSON），完全复用生产 container_command 契约（`.autonomics/inputs`、AUTONOMICS_INPUT/OUTPUT 家族、`$input/$output/$workdir`），出口按 OutputSpec 逐个回读（64 KiB 截断）。

## 容器会话的显式建模

按本文档前版要求，镜像 / 挂载 / 身份 / 网络 / 资源全部显式：

- 镜像：manifest 的 digest 钉死环境引用，不接受其它来源。
- 挂载：项目工作区 rw bind-mount 到 `/work`——同步即挂载本身，重启/删除容器不可能丢项目文件。
- 身份：`--userns keep-id` + `no-new-privileges` 保留；容器内 uid 映射到宿主用户，工作区文件属主正确。
- 网络：默认隔离（`--network none`），按次显式开启（`{"action":"start","network":true}`）。
- 资源：默认 4 CPU / 2Gi / 512 pids；可写 rootfs（装包需要）仅存在于 operator 的 dev 会话路径，agent 节点 `Resources` 路径的 hardening 不变。

## 接口一览

| 方法 | 路径 | 用途 |
| --- | --- | --- |
| GET/POST | `/plugin-projects/{name}/dev-session` | 会话探测 / start·stop·restart |
| GET (WS) | `/plugin-projects/{name}/terminal` | 容器终端流 |
| GET | `/plugin-projects/{name}/files`、`/file`、`/nodes` | 文件列表/读写、节点声明 |
| PUT | `/plugin-projects/{name}/file` | 保存（`original` 409） |
| POST | `/plugin-projects/{name}/validate`、`/activate` | 校验 / 校验+本地激活+热注册 |
| GET | `/plugin-projects/{name}/report` | 最新（或 `?attempt=n`）校验报告 |
| POST | `/plugin-projects/{name}/nodes/{kind}/test-run` | 节点试跑 |
| POST | `/plugins/{name}/fork`、`/uninstall`、`/rollback` | 管理动作 |
| GET/POST | `/plugin-projects`、`/plugins`、`/plugins/environments` | 列表 / 新建 / 环境批准 |

## 手动端到端配方

```bash
cd apps/gui && npm run build:gateway        # 前端嵌入 crates/gateway/gui-dist
cargo build -p <daemon binary>               # 重编 daemon（嵌入更新）
# 批准一个环境（digest 钉死），然后：
# 1. GUI → 插件管理 → 新建插件（选环境）→ 自动进入开发窗口
# 2. 会话自动启动；终端里 `ls /work` 应看到 manifest.toml/scripts/README.md
# 3. 编辑 scripts/demo.sh → 校验（六门 pass）→ 激活
# 4. GET /api/v1/plugins 应出现该插件且 registered，kind 为 <plugin>/<kind>
# 5. 试跑：入口 input_text 填文本 → 运行 → 出口 demo_out.txt 内容回显
# 6. 停止/重启容器 → /work 文件不变（bind mount 契约）
```
