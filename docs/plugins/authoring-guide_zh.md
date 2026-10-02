# 插件构建指南

本指南讲解如何从零创建插件，面向已经明确分析目标、但需要把工具映射到
Autonomics 容器节点契约的开发者。

字段级规则见 [Manifest 参考](manifest-reference_zh.md)，发布门槛见
[测试与发布清单](testing-and-release_zh.md)。

## 1. 确定节点边界

先定义科学契约，而不是罗列上游命令行参数：

- 哪个表或文件进入节点？
- 哪个文件离开节点？
- 哪些 identifier、列名、分隔符、缺失值和重复值规则有意义？
- 哪些参数确实应该出现在 DAG spec 中？
- 哪些实现细节应该固定在镜像或脚本里？

优先做小而可预测的节点，而不是包装上游所有选项的通用 wrapper。成熟默认值
固定在 manifest，只暴露分析者需要调整的参数。

只有使用同一镜像和 panel 绑定的多个相关调用才能共享一个插件。如果镜像或
panel 不同，应拆分插件。

## 2. 初始化仓库

在插件 checkout 根目录下创建 lowercase kebab-case 目录，例如
`/mnt/projects/node-plugins/clusterprofiler`：

```bash
cd /path/to/autonomics
cargo run -p container-plugin --bin nodedev -- \
  init --name clusterprofiler --dir /mnt/projects/node-plugins
```

`nodedev init` 只会生成带占位镜像的最小 manifest。其余布局需要自己补齐：

```text
clusterprofiler/
├── manifest.toml
├── scripts/
│   └── clusterprofiler_ora.R.sh
├── Dockerfile
├── README.md
├── test_clusterprofiler_ora.sh
└── fixtures/
    ├── gene_table.tsv
    └── pathways.gmt
```

在第一次可评审改动前初始化 Git：

```bash
cd /mnt/projects/node-plugins/clusterprofiler
git init -b main
```

## 3. 起草 Manifest 契约

声明镜像、端口、参数、命令和资源。以下是 Bioconductor 类节点的有用起点：

```toml
schema_version = 1
plugin_name = "clusterprofiler"

[image]
reference = "ghcr.io/auto-nomics/autonomics/clusterprofiler@sha256:<digest>"
tag = "0.1.0"
upstream = "Bioconductor clusterProfiler"
license = "Artistic-2.0"

[[nodes]]
kind = "clusterprofiler_ora"
desc = "Runs clusterProfiler over-representation analysis."
doc = """
Input 0 is a TSV gene table. Input 1 is a GMT gene-set file. The node writes
an enrichment TSV and a JSON run report.
"""
timeout_secs = 3600

[nodes.ports]
inputs = [
  { type = "file", label = "gene_table", accepted_formats = ["tsv"] },
  { type = "file", label = "gene_sets", accepted_formats = ["gmt"] },
]
outputs = [
  { path = "clusterprofiler_ora.tsv", format = "clusterprofiler_ora_tsv" },
  { path = "clusterprofiler_ora.json", format = "clusterprofiler_ora_json" },
]

[nodes.params]
gene_col = { type = "string", default = "gene", doc = "Gene identifier column" }
pvalue_cutoff = { type = "number", default = 0.05, min = 0.0, max = 1.0 }

[nodes.command]
interpreter = "Rscript"
script_file = "scripts/clusterprofiler_ora.R.sh"

[nodes.command.env]
AUTONOMICS_GENE_COL = "{{ gene_col }}"
AUTONOMICS_PVALUE_CUTOFF = "{{ pvalue_cutoff }}"
```

开发期间可以临时使用一个合法 digest 或已发布 digest 来测试解析，但绝不能
发布未构建、未推送的镜像引用。评审前必须替换为已推送镜像的 manifest digest。

## 4. 编写执行脚本

脚本是节点契约和工具之间的适配器。它必须确定性执行、尽早校验输入，并生成
所有声明输出。

通过环境变量读取值，不要把用户值拼进代码：

```r
input_path <- Sys.getenv("AUTONOMICS_INPUT0")
gene_col <- Sys.getenv("AUTONOMICS_GENE_COL")
output_path <- Sys.getenv("AUTONOMICS_OUTPUT0")
```

推荐脚本结构：

1. 从环境变量读取所有参数和路径。
2. 对空字符串、非法枚举以及 manifest DSL 无法表达的交叉约束立即失败。
3. 校验输入存在性、列、identifier、数值、重复值和文件格式。
4. 只在科学契约明确允许时做规范化。
5. 调用固定版本工具。
6. 写稳定输出 schema。
7. 对生物学上合法的空结果，写出承诺的空表或报告，而不是让节点失败。

Shell 脚本应使用严格模式：

```sh
#!/bin/sh
set -eu
```

R 脚本应避免隐式类型转换，并用可行动的信息显式 `stop()`。Python 脚本应显式
解析路径和数值，不执行不可信输入。

## 5. 构建镜像

Dockerfile 是插件溯源的一部分，不是本地便利脚本。使用 digest 固定基础镜像，
并安装精确运行时依赖。例如：

```dockerfile
FROM docker.io/bioconductor/bioconductor:3.21-R-4.5.2@sha256:<digest>

RUN Rscript -e ' \
  BiocManager::install(
    c("clusterProfiler", "data.table", "jsonlite"),
    ask = FALSE,
    update = FALSE
  ) \
'

RUN Rscript -e ' \
  stopifnot(requireNamespace("clusterProfiler", quietly = TRUE)); \
  stopifnot(requireNamespace("data.table", quietly = TRUE)); \
  stopifnot(requireNamespace("jsonlite", quietly = TRUE)) \
'

USER 1001
ENV HOME=/tmp
ENTRYPOINT ["Rscript"]
```

镜像必须能在 rootfs 只读时运行。缓存、临时文件和输出只能放在 `/work` 或
其他运行时提供的可写挂载。如果工具需要 `HOME`，指向 `/tmp`，不要放宽只读
rootfs。

在插件 README 中记录包版本和上游来源。若要求严格可复现，使用包管理器快照
或 vendored 源码；最终插件镜像 digest 是可执行层面的 lock。

## 6. 添加 Fixture 和冒烟测试

小型 fixture 能让脚本行为无需完整 DAG 即可评审。至少测试：

- 正常路径。
- 缺少必需列。
- 格式错误输入。
- 适用时的合法空结果。
- 每个可选 flag 分支。

冒烟测试应执行与 loader 内联内容相同的脚本。Podman 测试可把 fixture 挂到
`/work`，设置相同的 `AUTONOMICS_INPUT*`、`AUTONOMICS_OUTPUT*` 和参数变量，
并断言输出文件与 schema。

## 7. 在 Autonomics 中添加 Manifest 测试

新插件应在 `crates/container-plugin/tests/` 下有聚焦集成测试。测试应定位
插件 checkout、解析并内联 manifest，然后断言：

- 镜像 digest 固定。
- 期望 node kind 注册。
- 输入输出路径符合文档契约。
- 命令解释器和脚本标记按预期编译。
- 环境模板包含所有预期参数。
- 资源默认值或显式覆盖正确。

测试只可在可选插件 checkout 不存在时跳过；解析失败不能跳过。

## 8. 通过 Path 源开发

把本地 checkout 加入 `~/.autonomics/plugins.toml`：

```toml
[[plugin]]
name = "clusterprofiler"
path = "/mnt/projects/node-plugins/clusterprofiler"
```

每次修改 manifest 或脚本后重启 daemon。symlink 不会绕过校验，只是在快速
开发时避免重复安装未变更源码。

## 9. 发布家族

发布是显式操作：

1. 用 Podman 构建镜像。
2. 运行插件冒烟测试和 Autonomics 聚焦测试。
3. 推送版本化 image tag 到 GHCR。
4. 把已推送镜像的 repository digest 写入 `manifest.toml`。
5. 提交完整插件目录。
6. 推送插件 Git 仓库。
7. 在 `~/.autonomics/plugins.toml` 引用精确 commit SHA。

绝不要把本地 image ID 写成 `image.reference`。运行时节点拉取并执行的是仓库
manifest digest，不是本地 config-layer ID。

## 10. 编写插件文档

每个插件 README 都应说明：

- 科学目的和精确输入输出 schema。
- 参数语义与校验规则。
- 上游工具和包版本。
- 镜像构建与发布溯源。
- 数据 panel 或注释假设。
- 与上游 CLI / library API 的已知差异。
- 测试命令与 fixture 覆盖。
- 任何安全或资源覆盖的理由。

README 是插件契约的一部分。即使 manifest 可以解析，评审者也应拒绝含义不透明
的 wrapper。
