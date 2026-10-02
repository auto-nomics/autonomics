# 容器插件构建

这个目录是构建、评审和发布 manifest 容器插件的权威入口。一个插件是
自包含的 Git 仓库，声明一个或多个 File-to-File 分析节点、执行这些节点
所用的 digest 固定容器镜像，以及它们需要的版本化数据 panel。

## 文档地图

1. [插件构建指南](authoring-guide_zh.md)：从确定节点边界到安装已发布家族的
   完整教学流程。
2. [Manifest 参考](manifest-reference_zh.md)：字段语义、模板规则、校验行为
   和规范性约束。
3. [测试与发布清单](testing-and-release_zh.md)：测试层级、镜像发布、Git
   rev 固定和运维验收。
4. [节点插件化迁移流程](../plugin-node-migration_zh.md)：替换既有 Rust
   容器包装时额外需要的 parity 流程。

[节点插件架构设计](../design/node-plugin-architecture.md)解释历史设计动机。
当历史提案与当前实现不一致时，以本目录文档和 `container-plugin` 源码为准。

## 生命周期

```text
定义契约 → 编写插件 → 校验 manifest → 测试镜像/脚本
         → 发布镜像 → 发布插件 Git 仓库 → 在 plugins.toml 固定 rev
         → 启动同步/加载 → 编译节点 spec → 执行容器
```

daemon 启动时，Autonomics 读取 plugin root 旁的声明文件（通常是
`~/.autonomics/plugins.toml`），按固定 rev 物化 Git 源，扫描
`~/.autonomics/plugins`，并以 fail-closed 方式校验每个 `manifest.toml`。
一个非法家族会阻止启动，而不是让 registry 静默缺节点。每个 `[[nodes]]`
条目注册一个 node factory。节点运行时，已解析参数和声明输出会编译成
`ContainerCommandSpec`，再由 Podman 执行 digest 固定镜像。

## 核心规则

- 一个插件家族是一个目录，通常也是一个 Git 仓库。
- 只有共享同一镜像和 panel 绑定的多个节点才能放在同一个插件中；镜像或
  panel 需求不同时应拆成不同插件。
- 插件节点是 File-to-File：输入和输出都是文件，不是内存 DataFrame。
- `image.reference` 必须是完整的不可变
  `host/path@sha256:<digest>` 引用；tag 只是元数据，不参与解析。
- 优先使用 `script_file` 而不是内联 `script`；loader 启动时会内联文件并
  拒绝不安全路径。
- 用户可控值通过环境变量传入脚本，避免直接渲染进可执行源码。
- 省略 `[nodes.resources]` 可保留隔离网络和只读 rootfs。任何覆盖都必须在
  插件 README 中说明理由。
- 不要把 panel payload 打进插件；声明 `[[panels]]` bundle 引用，由运行时经
  catalog 解析并校验 checksum。
- Git 安装源必须使用 commit SHA；tag 和 branch 可变，会被拒绝。

## 本地开发

本地迭代可在 `~/.autonomics/plugins.toml` 使用 `path` 源：

```toml
[[plugin]]
name = "clusterprofiler"
path = "/mnt/projects/node-plugins/clusterprofiler"
```

修改 manifest 或脚本后重启 `autonomics serve`。发布形态必须使用 Git URL
和不可变 revision：

```toml
[[plugin]]
name = "clusterprofiler"
git = "https://github.com/auto-nomics/clusterprofiler-plugin.git"
rev = "<完整-commit-sha>"
```

## 事实来源

Rust 类型和 loader 是实现层权威：

- `crates/container-plugin/src/node_definition.rs`：manifest 节点 schema
  与跨字段校验。
- `crates/container-plugin/src/manifest.rs`：家族与镜像元数据。
- `crates/container-plugin/src/loader.rs`：fail-closed 加载与脚本内联。
- `crates/container-plugin/src/compile/`：参数解析、模板渲染和
  `ContainerCommandSpec` 编译。
- `crates/container-plugin/src/sync.rs`：Git/path 源同步。

如果文档与当前源码不一致，先修复文档或实现，不要依赖有争议的行为。
