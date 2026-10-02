# Manifest 规范

本文是 schema version 1 中 `manifest.toml` 的规范参考。实现层事实来源是
`crates/container-plugin/src/manifest.rs` 与
`crates/container-plugin/src/node_definition.rs`。清单会拒绝未知字段，因此
能被当前实现解析并不代表兼容未来 schema。

## 最小结构

```toml
schema_version = 1
plugin_name = "clusterprofiler"

[image]
reference = "ghcr.io/auto-nomics/autonomics/clusterprofiler@sha256:<64位十六进制摘要>"

[[nodes]]
kind = "clusterprofiler_ora"
desc = "一行节点描述。"
doc = "面向智能体的完整契约说明。"

[nodes.ports]
outputs = [{ path = "result.tsv" }]

[nodes.command]
interpreter = "Rscript"
script_file = "scripts/clusterprofiler_ora.R.sh"
```

`panels` 和 `nodes` 默认是空数组，但可发布插件至少应有一个节点。除极短示例外，
优先使用 `script_file`，不要内联 `script`。

## 家族字段

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `schema_version` | 是 | 必须为 `1`。 |
| `plugin_name` | 是 | 非空家族名。使用小写 kebab-case，并与仓库/安装目录名保持一致。 |
| `image` | 是 | 整个家族共享的可执行镜像元数据。 |
| `panels` | 否 | 家族内所有节点共同使用的目录面板绑定。 |
| `nodes` | 发布必填 | 共享镜像和面板绑定的 File-to-File 节点声明。 |

## `[image]`

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `reference` | 是 | 完整不可变引用 `host/path@sha256:<digest>`；可执行引用不能带 scheme 或 tag。 |
| `tag` | 否 | 仅用于展示的人类可读 tag，绝不参与拉取解析。 |
| `upstream` | 否 | 上游工具、版本和源码 revision。 |
| `license` | 否 | 镜像执行软件的 SPDX 许可证标识。 |

registry host 必须小写，可带数字端口。repository path 必须是非空小写段，且
不能是 `.` 或 `..`。digest 必须是 `sha256:` 加恰好 64 个十六进制字符，全零
占位符会被拒绝。

必须使用 push 后 registry 返回的 RepoDigest，不能使用本地 image ID。

## `[[panels]]`

每条记录把一个目录面板绑定到一个只读容器挂载点：

```toml
[[panels]]
binding = "gene_sets"
mount = "/panels/gene_sets"
bundle = "example/catalog-gene-sets"
```

- `binding` 是运行时 `DataBundle` 槽位名，在家族内必须唯一。
- `mount` 是容器内绝对路径，在家族内必须唯一。
- `bundle` 是 `owner/name` 形式的 Hugging Face dataset id。
- 家族内所有节点获得同一组面板绑定。若节点需要不同镜像或面板，应拆分插件。
- 面板内容由 data catalog 解析并校验摘要，绝不提交到插件仓库。

## `[[nodes]]`

| 字段 | 必填 | 默认值 | 含义 |
| --- | --- | --- | --- |
| `kind` | 是 | — | 工作区级注册标识：非空小写 `[a-z0-9_]`。 |
| `desc` | 是 | — | 简短列表描述。 |
| `doc` | 是 | — | 面向智能体的输入/输出/参数契约。 |
| `deprecated` | 否 | `false` | 标记旧节点但保留 kind。 |
| `timeout_secs` | 否 | `3600` | 必须大于零。 |
| `artifact_prefix` | 否 | `/artifacts/{kind}` | 发布输出使用的绝对 VFS 前缀。 |
| `ports` | 是 | — | 文件输入和必需输出。 |
| `params` | 否 | 空 | 参数 schema 声明。 |
| `command` | 是 | — | 容器调用方式。 |
| `resources` | 否 | 安全默认值 | 运行时资源与安全配置。 |

### `[nodes.ports]`

`inputs` 是文件端口数组：

```toml
inputs = [
  { type = "file", label = "gene_table", accepted_formats = ["tsv"] },
]
```

- `type` 当前只能是 `file`；插件节点按契约必须是 File-to-File。
- `label` 是 DAG 端口名。只有匿名单一输入才可省略。
- `accepted_formats` 的第一个值是主格式，其余是可接受替代格式。
  必须与 `label` 同时声明；匿名输入上的格式信息不会编译进端口契约。

`outputs` 必须至少一项：

```toml
outputs = [
  { path = "enrichment.tsv", format = "clusterprofiler_ora_tsv", label = "enrichment" },
]
```

- `path` 相对于 `/work`，不能是绝对路径、`.`、`..` 或包含 NUL。
- 成功运行后每个声明输出都必须存在，否则节点失败。
- `format` 是向下游传递的格式标签。
- `label` 默认取输出文件名主干。

### `[nodes.params]`

每个键是参数名，对应表支持：

| 字段 | 含义 |
| --- | --- |
| `type` | 必填：`bool`、`int`、`number`、`string`、`string_array`。 |
| `default` | 与类型匹配的 JSON 值。无默认且 `optional = false` 的参数是必填。 |
| `optional` | 默认 `false`。缺失的可选参数解析为 null，并在 `argv`/`env` 渲染为空字符串。 |
| `doc` | 面向智能体的参数语义和单位。 |
| `min`、`max` | 闭区间数值边界。 |
| `exclusive_min`、`exclusive_max` | 开区间数值边界。 |
| `min_len`、`max_len` | 数组元素数量边界；只对 `string_array` 有效。 |
| `requires` | 布尔门控：当本参数解析为 `true` 时，所有目标布尔参数也必须为 `true`。 |

编译出的 JSON Schema 是 `additionalProperties: false` 的对象。缺失必填值、
未知键、类型错误和边界违规都会在容器启动前失败。

### `[nodes.command]`

| 字段 | 含义 |
| --- | --- |
| `interpreter` | 必填的 `command[0]`，如 `sh`、`bash`、`python`、`Rscript`。 |
| `argv` | interpreter 后的固定参数。 |
| `script` | 内联脚本；与 `script_file` 互斥。 |
| `script_file` | 相对脚本路径，适合可审查插件；loader 会在校验前内联。 |
| `env` | 额外环境变量。 |
| `files` | 写入 `/work/.autonomics/files` 的静态文本文件。 |

`script_file` 只能包含普通相对路径组件。绝对路径、`.`、`..` 和 NUL 都会被
拒绝。建议将脚本放在插件仓库的 `scripts/` 目录。

运行时自动提供：

- `AUTONOMICS_INPUT0`、`AUTONOMICS_INPUT1`……
- `AUTONOMICS_OUTPUT0`、`AUTONOMICS_OUTPUT1`……
- `AUTONOMICS_INPUT_COUNT` 与 `AUTONOMICS_OUTPUT_COUNT`
- `AUTONOMICS_WORKDIR`
- 存在脚本时的 `AUTONOMICS_SCRIPT` 与 `AUTONOMICS_FILES_DIR`

### 模板规则

占位符写作 `{{ param }}`，token 只能包含 ASCII 字母、数字或 `_`，可出现在
`argv`、`env` 和脚本源码中，且必须引用已声明参数。

- `argv` 与 `env` 渲染已校验值，不经过 shell 重新解析。
- null 渲染为空字符串。
- 字符串数组在 `argv`/`env` 表面用单个空格连接。
- 脚本渲染在引号字面量和注释之外应用 interpreter 相关引用规则；不要依赖它构造命令。
- schema version 1 的 `files` 值是原样透传，不要写入占位符。

安全写法是把参数渲染到环境变量，再由脚本解析和校验。

### `[nodes.resources]`

省略该表即保留安全默认值：

```toml
[nodes.resources]
read_only_rootfs = true
```

| 字段 | 取值/默认值 | 含义 |
| --- | --- | --- |
| `network` | `isolated`（默认）、`egress` | `isolated` 无网络设备；每个 `egress` 插件必须说明理由。 |
| `read_only_rootfs` | 默认 `true` | `/work`、`/tmp`、`/dev/shm` 仍是运行时可写挂载。 |
| `pull_policy` | 默认 `missing`；也可 `always`、`newer`、`never` | Podman 拉取策略；生产清单通常省略。 |
| `cpus` | 正数 | CPU 限制。 |
| `memory` | 运行时大小字符串，如 `8Gi` | 内存限制。 |
| `pids_limit` | 正整数 | 进程数限制。 |
| `shm_size` | 运行时大小字符串 | 共享内存大小。 |
| `gpus` | `all`、数量或 `device=0,2` | GPU 直通；省略表示不可见。 |
| `user` | 镜像/运行时相关覆盖 | 仅在必须使用非 root 内置用户时设置。 |

资源覆盖属于安全审查范围。插件 README 必须解释每一个显式覆盖。

## 校验行为

启动加载是 fail-closed。出现以下情况时阻断插件发布：

- TOML 解析失败或存在未知字段。
- 镜像引用或面板 bundle id 无效。
- 面板 mount 或 binding 重复。
- 同时设置 `script` 与 `script_file`，或脚本路径不安全/缺失。
- 节点 kind 非法、超时为零或没有输出。
- 输出路径不安全。
- 非数组参数使用数组长度边界。
- 布尔门控指向缺失或非布尔参数。
- 命令占位符引用未声明参数。
- 节点 kind 与同一插件根下其他插件冲突。

参数默认值、类型、边界和门控在节点 spec 编译执行时还会再次校验。
