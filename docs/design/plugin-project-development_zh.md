# 插件项目与容器开发

插件项目以 daemon 管理的单个插件工作区为单位。一个项目对应一个 Git 仓库和一个 `manifest.toml`，其中声明的多个节点共享插件镜像。项目开发内容和已激活的运行快照是不同的：修改工作区不会自动改变已注册节点。

## 开发界面（已实现）

**统一插件工作区**（桌面活动栏或移动侧栏 → 插件管理，editor kind `plugin-projects`）：

- 插件总览按名称合并开发项目和已安装快照，每个插件只显示一行：项目状态、安装状态、镜像、节点 kind、「已注册」徽标；支持回滚与卸载（工作区与快照保留）。仅安装但没有开发工作区的插件不显示「打开项目」。
- 新建插件：绑定已批准环境（`POST /api/v1/plugin-projects`，走 `create_plugin_scaffolded`），生成可运行的脚手架，创建后直接在当前工作区连接开发容器。agent 工具创建的插件保持裸 manifest。
- 左侧「项目 / 文件」导航与总览共用同一份项目数据、新建流程和项目选择。总览与当前项目之间切换保留代码草稿与 VS Code 工作台。
- 项目编辑：Monaco 多文件缓冲区、Ctrl+S、`original` 乐观并发保存、文件及文件夹的新建/重命名/删除、容器文件监控、语言服务、Python 调试和可选 VS Code 工作台。
- 开发容器：通过 `GET/POST/DELETE .../container` 统一管理，编辑、语言服务、调试、终端共用同一容器；支持重启（`POST .../container?rebuild=true`）、停止与重新连接。停止保留本地编辑缓冲区，未保存的 VS Code 修改会先提示。
- manifest 无效时可进入修复模式，通过 `dev-files` / `dev-file` 编辑工作区，修复后重新连接容器。
- 终端使用工作台底部终端面板（xterm.js），会话独立于编辑器标签，支持输入、输出、调整尺寸与重新连接。
- 校验 / 激活 / 试跑 / 报告位于同一项目编辑区，使用已保存的工作区；有未保存修改时提示先保存。激活后刷新总览中的安装与注册状态。
- 校验 / 激活：`POST .../validate`、`POST .../activate`（六门校验 → 本地快照 → 热注册，节点以 `plugin/node` 进入 DAG）；报告显示各门 pass/fail。
- 试跑：`POST .../nodes/{kind}/test-run`，按声明的 file 入口端口填样例文本（参数为 JSON），复用生产 container_command 契约，出口按 OutputSpec 逐个回读（64 KiB 截断）。
- 旧布局中的 `plugin-manager` 和 `plugin-dev` 标签自动合并为统一工作区；原先正在开发的项目会恢复打开。

## 容器与工作区

开发容器由 gateway 的 `DevContainers` 管理，项目工作区挂载到 `/work`。文件保存和容器终端修改都落到同一工作区，校验及激活读取这份已保存内容；修改工作区不会自动改变已注册的快照。项目的 devcontainer 配置用于容器、远程用户和生命周期命令。GUI 不再使用另一套独立的开发会话或文本编辑器。

## 接口一览

| 方法 | 路径 | 用途 |
| --- | --- | --- |
| GET/POST/DELETE | `/plugin-projects/{name}/container` | 状态 / 连接（`?rebuild=true` 重建）/ 停止 |
| GET/POST/DELETE | `/plugin-projects/{name}/terminal` | 终端输出 / 打开 / 关闭 |
| POST/PUT | `/plugin-projects/{name}/terminal/input`、`/terminal/size` | 终端输入 / 调整尺寸 |
| GET/PUT | `/plugin-projects/{name}/dev-files`、`/dev-file` | 无效 manifest 的文件修复 |
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
# 1. GUI → 插件管理 → 新建插件（选环境）→ 在同一工作区打开项目
# 2. 容器自动连接；底部终端里 `ls /work` 应看到 manifest.toml/scripts/README.md
# 3. 编辑 scripts/demo.sh → 校验（六门 pass）→ 激活
# 4. GET /api/v1/plugins 应出现该插件且 registered，kind 为 <plugin>/<kind>
# 5. 试跑：入口 input_text 填文本 → 运行 → 出口 demo_out.txt 内容回显
# 6. 停止/重启容器 → /work 文件不变；切换插件总览再返回项目，编辑缓冲区仍保留
```
