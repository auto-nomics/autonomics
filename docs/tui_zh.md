# Autonomics TUI（`autonomics-tui`）

[English](tui.md) | [中文](tui_zh.md)

Autonomics 智能体平台的交互式终端界面——通过一个基于 `ratatui` 的二进制程序，
与 LLM 智能体对话、配置模型供应商、管理 OpenGWAS 缓存，以及维护本地文献库。

## 功能特性

- **智能体对话** — 基于 `agentik-core` 的流式对话，支持工具调用。实时 Markdown
  渲染（代码块经 `syntect` 语法高亮）、思维增量展示、逐轮 token 用量统计、后台
  工具任务，以及支持增量历史搜索（`Ctrl+R`）的可滚动对话记录。
- **模型配置** — 树形供应商目录。输入 API Key 即可激活内置模型；变更持久化到
  `phloem.db`，并热替换到运行中的智能体（无需重启）。
- **文献管理** — `bib` 子命令：上传全文 PDF（自动提取纯文本）、列出待处理全文
  请求、查看文章状态、搜索本地文献库，以及导出为 BibTeX / RIS / Markdown / CSL JSON。
- **OpenGWAS 缓存** — `cache` 子命令：刷新或清除 OpenGWAS `gwasinfo` 目录的本地
  SQLite 快照。

## 构建与运行

```bash
# 构建（在工作区根目录）
cargo build -p tui

# 启动交互式 TUI（默认子命令）
cargo run -p tui
# …或安装二进制后
autonomics-tui

# 指定 TUI 配置文件
autonomics-tui tui --config /path/to/config.toml
```

工作区中的二进制名为 `tui`；如需使用下文所示的 CLI 名称，可安装/拷贝为
`autonomics-tui`。两者指向同一个构件。

## CLI 总览

```
autonomics-tui [OPTIONS] [COMMAND]

Commands:
  tui     启动交互式 TUI（无子命令时的默认行为）
  cache   本地缓存管理（刷新、查看、清除）
  bib     文献管理——上传全文 PDF、列出待处理请求
  help    打印帮助信息

Options:
  -h, --help     打印帮助
  -V, --version  打印版本号
```

---

## 交互式 TUI

### 布局

```
┌─────────────────────── 标签栏 ───────────────────────┐
│  [Agent]   Config                                    │
├──────────────────────────────────────────────────────┤
│  状态栏（模型 · token · 智能体状态）                    │
│  后台工具任务（运行时显示）                              │
│                                                       │
│  对话记录（可滚动，Markdown 渲染）                       │
│                                                       │
│  ❯ 输入编辑器（无边框，自动增高）                        │
│  底部快捷键提示                                         │
└──────────────────────────────────────────────────────┘
```

两个标签页：**Agent** 和 **Config**。默认标签页为 Agent。

### 全局快捷键

| 按键 | 动作 |
|------|------|
| `]` / `[` | 切换到下一个 / 上一个标签页 |
| `Ctrl+C` | 协作式取消（第一次按下）→ 强制退出（3 秒内再次按下） |

### Agent 标签页

Agent 标签页有两种输入模式——**浏览模式**（默认）和**输入模式**（编辑器）。

**浏览模式——滚动对话记录**

| 按键 | 动作 |
|------|------|
| `↑` / `↓` | 上下滚动一行 |
| `PageUp` / `PageDown` | 滚动半页（12 行） |
| `Home` | 跳转到顶部 |
| `End` | 跳转到底部（重新启用自动滚动） |
| `Enter` | 进入编辑器（输入模式） |
| 鼠标滚轮 | 滚动；解除自动滚动锁定 |
| `Ctrl+G` | 切换自动滚动到底部的锁定状态 |

**输入模式——编写消息**

| 按键 | 动作 |
|------|------|
| 输入字符 | 在编辑器中插入文本 |
| `Enter` | 发送消息（返回浏览模式） |
| `Shift+Enter` / `Alt+Enter` | 插入换行（多行编写） |
| `Esc` | 退出编辑器（不发送） |
| `↑` / `↓` | 回溯上一条 / 下一条历史消息 |
| `Ctrl+R` | 启动增量历史搜索（输入即过滤，`↑`/`↓` 导航，`Enter` 确认，`Esc` 取消） |

编辑器是基于 `xai_textarea` 的 Emacs 风格多行文本区域：完整的 Emacs 键位、
撤销/重做、鼠标选择和内置滚动条。自动换行；输入框最高增长到 10 行后内部滚动。

智能体运行时，编辑器被禁用并显示占位提示。

**对话记录渲染**

- 用户消息、助手回复（流式）、思维块、工具调用与结果、后台任务完成通知、
  错误消息分别以不同的区块类型渲染。
- 助手消息渲染为 Markdown，代码块带语法高亮。
- 每轮 token 用量（`input`、`output`、`cache_read`）附在助手消息下方，并在
  状态栏中累加。
- 运行中的后台工具任务显示在对话区上方的专用条带中，完成后折叠。

### Config 标签页

内置供应商目录（来自 `agentik_sdk::provider::registry`）的**树形视图**：

```
▼ deepseek ✓                ← 已配置，已展开
  ● deepseek-v4-pro         ← 当前激活的模型（绿色 ●）
    deepseek-v4-flash
▶ mimo ✓                    ← 已配置，已折叠
── minimax ✗                ← 未配置（灰色）
```

| 按键 | 动作 |
|------|------|
| `↑` / `↓`（或 `k` / `j`） | 移动光标 |
| `→` / `←`（或 `l` / `h`、`Tab` / `BackTab`） | 展开 / 折叠供应商节点 |
| `Enter` | 激活光标所在模型 |
| `e` | 编辑该供应商的 API Key（打开凭据面板） |
| `r` | 从数据库 + SDK 注册表重新加载目录 |

在**凭据面板**（右侧）中，将 API Key 输入文本框：

| 按键 | 动作 |
|------|------|
| 输入 / 粘贴 | 输入 API Key |
| `Enter` | 保存到数据库（`providers` 表） |
| `Esc` | 取消，不保存 |

激活模型时，`provider:model` 会被持久化到 `settings` 表，并在智能体运行时中
原子热替换 `Model`——下一轮对话即使用新模型，无需重启。

---

## `cache` 子命令

管理 OpenGWAS `gwasinfo` 的本地 SQLite 缓存。

```
autonomics-tui cache <ACTION>

Actions:
  refresh-opengwas   从远程 API 重新拉取 OpenGWAS gwasinfo 目录
  clear-opengwas     删除本地 OpenGWAS gwasinfo 缓存文件
```

**刷新**

```bash
autonomics-tui cache refresh-opengwas [--show-cache-path]
```

拉取完整目录并覆盖 SQLite 快照。需要设置 `OPENGWAS_TOKEN`。

**清除**

```bash
autonomics-tui cache clear-opengwas [-y]
```

删除缓存文件。下一次 TUI 会话中的查询将触发全新拉取。除非传入 `-y`，否则会
交互式确认。

**缓存目录解析顺序**

1. `$OPENGWAS_CACHE_DIR`（非空时）
2. `$HOME/.cache/opengwas`
3. `$TMPDIR/opengwas`（兜底）

---

## `bib` 子命令

本地文献库管理。所有子命令均接受 `--db <PATH>`（默认 `bib.db`）。

```
autonomics-tui bib [OPTIONS] <ACTION>

Actions:
  upload    上传 PDF（或其他文档）作为文章全文
  requests  列出标记为需要上传全文的文章
  info      查看指定文章的当前全文状态
  list      列出文献库中的所有文章
  export    以引用格式导出文章（BibTeX、RIS、Markdown）
```

### `upload` — 上传全文

```bash
autonomics-tui bib upload \
  --pdf <PATH> \
  --article-id <ID> \
  [--collection-id <CID>]
```

读取文件，根据扩展名（`pdf` / `html` / `txt`）检测格式，通过 `SimpleExtractor`
提取纯文本，并存储 `FullText` 记录。若提供 `--collection-id`，该集合内文章的
`fetch_status` 将更新为 `fulltext_available`。

### `requests` — 待处理全文请求

```bash
autonomics-tui bib requests [--collection-id <CID>]
```

列出所有 `fetch_status = fulltext_requested` 的文章。输出包含文章 ID、标题、
DOI、年份、所属集合及备注。

### `info` — 文章详情

```bash
autonomics-tui bib info --article-id <ID>
```

打印文章的书目元数据（标题、期刊、年份、DOI），以及——若已上传全文——其格式、
来源、文件路径、字节大小和 200 字文本预览。

### `list` — 搜索 / 列出文献库

```bash
autonomics-tui bib list [--query <关键词>] [--limit <N>]
```

`--query` 匹配标题和摘要。默认上限 50 条（最大 500）。

### `export` — 引用导出

```bash
autonomics-tui bib export \
  --format <bibtex|ris|markdown|csl_json> \
  [--collection-id <CID>] \
  [--output <PATH>] \
  [--limit <N>]
```

导出某个集合或整个文献库。不指定 `--output` 时，渲染后的引用文本输出到
stdout。

---

## 环境变量

| 变量 | 用途 |
|------|------|
| `OPENGWAS_TOKEN` | `cache refresh-opengwas` 及任何 OpenGWAS 智能体查询所必需 |
| `OPENGWAS_CACHE_DIR` | 覆盖 OpenGWAS 缓存目录 |
| `RUST_LOG` | 覆盖默认 tracing 过滤器（默认为 `autonomics_tui=debug,agentik_core=debug,agentik_sdk=debug`） |

## 磁盘产物

| 路径 | 内容 |
|------|------|
| `phloem.db` | TUI 配置数据库（`providers`、`models`、`settings` 表） |
| `bib.db` | 文献库（文章、集合、全文记录） |
| `logs/phloem-tui.log.<日期>` | 按日轮转的 tracing 日志 |

## 架构说明

- **主循环** — 由 `tokio::select!` 驱动，固定 ~60 fps 渲染节拍（漏拍策略为
  `Skip`）。事件分支只修改状态并设置 dirty 标志；渲染仅在节拍分支中执行。
  空闲时零 CPU 占用。
- **状态 / 渲染分离** — `AppState` 持有所有可变状态；组件无状态，通过
  `StatefulWidgetRef` 读取状态。逐消息内容版本缓存（`msg_versions` +
  `cached_msg_lines`）跳过未变消息的重渲染。
- **模型热替换** — `active_model: Arc<ArcSwapOption<Model>>` 让 Config 标签页
  能原子地替换模型；智能体在每轮读取最新模型。
- **Panic 安全** — panic hook 在传播前恢复终端状态，确保崩溃不会让 shell 停
  留在 raw / 备用屏模式。

## 源码导览

| 文件 | 职责 |
|------|------|
| `src/main.rs` | CLI 定义（clap）、子命令分发、`bib` 实现 |
| `src/app.rs` | `App` 结构体、异步主循环、事件处理、模型热替换 |
| `src/state.rs` | 所有 TUI 状态类型 + `AgentEvent → state` 归约器 |
| `src/config_db.rs` | `providers` / `models` / `settings` 表的 SQLite CRUD |
| `src/widgets/` | Ratatui 组件（agent 标签页、对话、状态栏、输入、模型配置、工具执行） |
| `src/xai_textarea/` | 内嵌多行文本区域（Emacs 键位、换行、撤销/重做） |
