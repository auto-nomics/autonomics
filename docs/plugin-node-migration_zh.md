# 节点插件化迁移工作流

把硬编码的容器节点迁移为清单插件的标准工作流。本文是
[Container Node Migration Workflow](container-node-migration.md) 在插件时代的续篇：
那份文档描述「工具 → 镜像 + 目录面板 + 薄包装」的三层分离；这份文档描述
「薄包装 → 纯数据清单 + git 仓库」的最后一跃。

参考实现是 `ldsc` 家族（`ldsc_h2`、`ldsc_munge`、
`ldsc_rg`），已端到端迁移完成，并由
`crates/container-plugin/tests/ldsc_migration.rs`（黄金对账）和
`ldsc_e2e.rs`（分发链路）持续验证。

## 总览

一个插件 = 一个目录 = 一个 git 仓库。插件目录是自包含单元，承载一个工具
节点存在所需的全部东西：

```text
ldsc/
├── manifest.toml          # 节点契约：params、ports、panels、image
├── scripts/               # 执行脚本，用相对路径引用
│   ├── h2.sh
│   ├── munge.sh
│   └── rg.sh
├── Dockerfile             # 镜像溯源（构建+推送仍走 GHCR）
├── ldsc-python3/          # vendored 上游源码（不含内嵌 .git）
├── test_*.sh              # 镜像基线
└── README.md
```

启动时运行时宿主执行 `container_plugin::sync::sync`（读
`state_dir/plugins.toml`），把每个插件物化到 `state_dir/plugins` 下，随后
loader 扫描该根目录、以 fail-closed 方式校验每个 `manifest.toml`，并为每个
`[[nodes]]` 条目注册一个 `ManifestNodeFactory`。

## 适用范围

当节点是 `container_command` 支撑的包装时迁移：一次或多次共享同一镜像与
面板集的工具调用。**不要**迁移纯 Rust 的进程内变换——它们没有镜像、没有
面板，也没有理由离开编译期注册表。

一个插件家族可以容纳多个 `[[nodes]]`，当且仅当它们共享镜像与面板绑定——
`ldsc` 的 h²/munge/rg 共享一个镜像和一对面板，所以是一个插件。如果两个
分析需要不同的镜像或不同的面板，那就是两个插件。

## 第 0 步：DSL 能力预检

写任何 TOML 之前，先读旧包装的 `XxxSpec`、`validate`、`container_spec`，
确认插件 DSL 能表达每一种行为。最常见对不上号的三种情况：

1. **可选 flag** —— 一个没有默认值、缺省即省略 flag 的参数（如
   `intercept_h2`、`chisq_max`）。用 `optional = true` 表达，经 `env` 传入，
   让脚本自己 `[ -n "$VAR" ]` 判断。v0 模板语言没有条件语句；`if` 逻辑全部
   归脚本。
2. **gzip 输入处理** —— legacy 的 `decompress_gzip_inputs` 辅助函数没有插件
   等价物。在脚本里内联一段 `case *.gz) gzip -dc …` 分支。
3. **列名 / 列表覆盖**（munge 的九个列 flag）——每个变成一个
   `optional = true` 的字符串参数，经专用 env 变量传入，脚本自己拼
   `--flag $val` 列表。

如果 DSL 确实表达不了某个东西（新参数类型、数组下标、条件模板），先扩展
`crates/container-plugin` **再**迁移——不要在清单或脚本里打补丁。

## 第 1 步：创建插件目录

在插件检出根目录（如在开发路径 `/mnt/projects/node-plugins/`）下创建
`<tool>/`，包含：

- `manifest.toml` —— 见下文编写规范。
- `scripts/*.sh` —— 执行脚本，经 `script_file = "scripts/<name>.sh"` 引用。
- `Dockerfile` + vendored 源码 + `test_*.sh` —— 从 `containers/<tool>/`
  原样搬来（把测试脚本的 `root=` 改指插件目录）。
- `README.md` —— 溯源、目录结构、迁移对账说明。

**镜像构建树必须在同一次改动里搬走。** 插件是工具的唯一事实来源；一个
Dockerfile 还留在 `containers/` 下的包装只迁移了一半。

如果某个仍未迁移的 Rust 测试引用了 `containers/<tool>/` 下的 fixture（如
`ldsc_munge.rs` 引用了 vendored munge fixture），把该测试改指插件
目录，插件检出缺失时回退为跳过。

## 第 2 步：黄金对账测试

在 `crates/container-plugin/tests/<tool>_migration.rs` 加一个测试，编译清单
并逐字段断言与旧包装 `container_spec` 的对账（见 `ldsc_migration.rs`）：

```rust
let manifest = load_manifest(&root);            // TOML + 内联 script_file
let node = node_by_kind(&manifest, "…");
let compiled = compile_container_spec(node, &manifest.image,
                                      &manifest.panels, &json!({}))?;

assert_eq!(compiled.image, "<完整 digest 钉死的引用>");
assert_eq!(compiled.outputs[0].path, "….log");
assert_eq!(compiled.outputs[0].format.as_deref(), Some("…_log"));
assert_eq!(compiled.panel_bundles[0].panel_id, "wjixiang/catalog-…");
assert_eq!(compiled.timeout_secs, …);
assert_eq!(compiled.artifact_prefix, "/artifacts/…");
// 语义级脚本标记，不做字节级相等
assert!(script.contains("--ref-ld-chr /panels/ref_ld/LDscore."));
```

两条规则让黄金测试诚实：

- **镜像/输出/面板/资源字节级相等。** 迁移的全部意义就在于编译出的 spec 是
  同一个契约。
- **脚本语义级相等，而非字节级。** 插件经 env 和自带的 `if` 分支驱动 flag，
  而不是 Rust 拼字符串，所以变量名和排版不同。断言承载行为的 token
  （`--ref-ld-chr /panels/…`、`--out "$AUTONOMICS_WORKDIR/…"`、gzip 的
  `case` 分支），并在注释里记录任何刻意的变量名差异（`OUT_PREFIX` vs
  legacy 的 `out_prefix`）。

## 第 3 步：发布到 git

```sh
git init --initial-branch=main
# vendored 源码必须是普通文件，不能是 gitlink：
rm -rf <vendored>/.git
git add -A && git commit
gh repo create auto-nomics/<tool>-plugin --private --source=.
git push -u origin main
REV=$(git rev-parse HEAD)
```

然后在 `~/.autonomics/plugins.toml`（或部署侧等价文件）声明来源：

```toml
[[plugin]]
name = "ldsc"
git = "git@github.com:auto-nomics/ldsc-plugin.git"
rev = "6f7118d61dd60ca7ce95d7d524ccec3880d96026"   # 钉死的 commit SHA
```

**`rev` 必须钉 commit SHA。** tag 和分支会移动；镜像钉 digest、数据包钉
digest、插件代码钉 rev——同一条不可变纪律向上延伸一层。`sync` 会拒绝非
SHA 的 ref。

发布 rev 通过波次端到端测试（`plugin_wave_e2e.rs` 模式：sync 所有已声明的
家族、加载、注册、断言 kind）之前，迁移不算完成。同一波迁移的家族一起在
`plugins.toml` 里声明，由一个共享的 e2e 测试统一验证，让安装链路在波次
交付前作为整体被演练。

## 第 4 步：删除旧包装

删掉 `crates/node-bundles/nodes-io/src/<tool>_container.rs`、它的 `pub mod`
行、以及 `registry.register(…)` 块。然后扫残留引用：

```sh
grep -rn "<tool>_container::" crates/ apps/ --include='*.rs'
```

凡是引用过包装的地方都要改到插件或删掉：

- `data-engine` 测试里的 kind 期望列表（`list_nodes`、`get_node_spec`）
- `data-engine/tests/bundle_bound_nodes.rs` 里的 `catalog()` 面板 fixture 和
  build case
- 集成测试里任何 `use nodes_io::<tool>_container::…`

**要么一次迁完整个家族，要么接受一个显式标注的临时共享常量模块。**
`ldsc` 试点先只迁了 `ldsc_h2`，让 `ldsc_munge`/`ldsc_rg` 指向一个一次性的
`ldsc_image` 常量模块——家族其余部分落地的那一刻，这个模块就成了死代码，
还得再删一遍。当家族共享同一镜像和面板集时，优先在同一次改动里迁完每个
`[[nodes]]` 条目。

## 清单编写规范

```toml
schema_version = 1
plugin_name = "ldsc"

[image]
reference = "ghcr.io/auto-nomics/autonomics/ldsc@sha256:2dad70…"
tag = "3.0.1-allele-filter"          # 仅展示；不参与解析
upstream = "CBIIT LDSC 3.0.1 @ 6c67395"
license = "BSD-3-Clause"

[[panels]]
binding = "ref_ld"                     # DataBundleBinding.binding
mount = "/panels/ref_ld"
bundle = "wjixiang/catalog-ldsc-ref-ld-1000g-eur-basic"   # HfRepoId

[[nodes]]
kind = "ldsc_h2"
desc = "…"
doc = """…"""                          # agent 读这个——为它写清楚
timeout_secs = 900
artifact_prefix = "/artifacts/ldsc_h2"

[nodes.ports]
inputs = [{ type = "file" }]
outputs = [{ path = "ldsc_h2.log", format = "ldsc_log" }]

[nodes.params]
n_blocks = { type = "int", default = 200, min = 2.0, doc = "…" }
intercept_h2 = { type = "number", optional = true, doc = "…" }

[nodes.command]
interpreter = "sh"
script_file = "scripts/h2.sh"

[nodes.command.env]
LDSC_N_BLOCKS = "{{ n_blocks }}"
LDSC_INTERCEPT_H2 = "{{ intercept_h2 }}"
```

经验法则：

- `kind` 去掉 legacy 的 `_container` 后缀（`ldsc_h2_container` →
  `ldsc_h2`）：这个后缀当初只用于区分容器包装与原生 Rust 移植，而原生
  移植已经删了。引用旧 kind 的存量 DAG spec 需要重新生成。
- `image.reference` 是完整 `host/path@sha256:` 字符串；没有隐式命名空间，
  没有 env 覆盖。
- `panel.bundle` 是 `HfRepoId`（`owner/name`）；经运行时 `DataBundle` 目录
  解析，从不裸写对象键。数据包本身**不随插件分发**：`autonomics panels
  sync` 会把引用仓库的当前条目下载进本地 catalog 缓存（启动自检只做本地
  存在性检查，缺包时提示执行该命令）。
- 有 `default` 的参数进 schema 的 `properties`；无 default 且无 `optional`
  的参数进 `required`；`optional` 参数解析为 null、渲染为空字符串。
- 几行以上的脚本优先 `script_file` 而非内联 `script`；loader 在校验前内联
  文件，两者互斥。
- `env` 是脚本用 `[ -n … ]` 判断的参数的规范通道；`argv` 用于静态 runner
  参数。

## 已知陷阱

1. **`Resources::default()` 会翻转硬化默认值。** `Default::derive` 把
   `read_only_rootfs` 设成 `false`；字段上的
   `#[serde(default = "default_true")]` 只在表存在但键缺失时生效。
   `Resources` 必须手动 `impl Default`（已经这么做了），否则省略
   `[nodes.resources]` 的清单会静默放松只读根文件系统。
2. **`nodes = []` 与 `[[nodes]]` 冲突。** TOML 的表数组不能再声明成空内联
   数组。家族有条目时干脆省略 `nodes`。
3. **渲染器把 `#` 注释和单引号当字面量。** 脚本里散文式的撇号
   （`wrapper's`）曾被解析成未闭合引号。注释（`#` 到行尾）和引号区原样
   透传；引号内的 `{{param}}` 刻意不解析。
4. **单个 `{` 不能让扫描器死循环。** 只有 `{{` 开启模板；单个 `{` 是字面
   字节。（这曾让字节扫描器死循环——已有回归测试覆盖。）
5. **PanelCache 校验 checksum。** e2e 测试用一个伪造 `manifest.json` 的
   面板 stub 会在物化阶段失败。在假对象存储里提供真实的面板 manifest +
   payload。
6. **插件根发现是进程全局的。** 测试里永远不要 `set_var`
   `AUTONOMICS_PLUGIN_ROOT`——并行的 registry 构建可能观察到一个写到一半
   的插件。通过 `build_default_registry_with_container_execution(…,
   plugins_root)` 显式传入根目录。
7. **vendored 源码不能是 gitlink。** 复制的上游检出带自己的 `.git`；作为
   普通文件提交（`rm -rf <vendored>/.git`），否则克隆插件会缺 payload。
8. **f64 的 env 渲染会丢尾随 `.0`。** `30.0` 渲染成 `"30.0"`（serde_json），
   而 legacy 的 `format!` 产出 `"30"`。断言用 serde_json 的拼写；值在浮点
   解析后相等。

## 验收清单

- [ ] 每个 `[[nodes]]` kind 是 legacy `*_CONTAINER_KIND` 去掉 `_container`
      后缀。
- [ ] `image.reference` 钉 digest；`upstream`/`license` 已填。
- [ ] 面板是经 `panel_bundles` 绑定的 `HfRepoId`，不是裸 `panels`。
- [ ] 镜像构建树已移出 `containers/<tool>/`。
- [ ] 黄金测试断言镜像/输出/面板/资源字节级相等 + 脚本语义标记。
- [ ] 插件已推到 git、钉 `rev`；`plugins.toml` 引用它。
- [ ] 旧包装文件 + `pub mod` + `register(…)` 已删。
- [ ] `grep "<tool>_container::"` 在 `crates/ apps/` 下为空。
- [ ] `cargo test -p nodes-io -p data-engine -p container-plugin` 全绿。
- [ ] `ldsc_e2e` 风格的分发测试（sync → load → build → execute）对着已推送
      的仓库通过。
