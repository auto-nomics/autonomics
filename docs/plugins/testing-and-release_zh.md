# 插件测试与发布检查表

本检查表覆盖从本地校验到不可变发布家族的全过程。不要因为 manifest 能解析就
发布；只有契约、镜像、脚本、分发和文档都经过测试后才能发布。

## 测试金字塔

| 层级 | 回答的问题 | 必需证据 |
| --- | --- | --- |
| Manifest 加载 | 数据契约能否解析并校验？ | 聚焦 Rust 测试加载并内联真实 manifest。 |
| 脚本 fixture | 适配器是否执行契约？ | 正例，以及缺失/非法/空输入用例。 |
| 镜像冒烟 | 精确镜像能否运行适配器？ | 使用发布摘要和挂载 fixture 执行测试。 |
| Spec 编译 | 参数与输出是否编译成预期命令？ | 对默认值、env、输出、安全和错误做黄金断言。 |
| Registry/e2e | 节点能否穿过 DAG 运行时工作？ | 使用真实或假 DAG 运行并检查全部声明输出。 |
| 分发 | 干净主机能否安装？ | 按 exact SHA 全新 clone、加载、注册、拉镜像、同步面板。 |

运行插件专属测试时应显式指定插件根目录，不要修改进程级插件根：

```bash
NODE_PLUGINS_ROOT=/mnt/projects/node-plugins \
  cargo test -p container-plugin --test clusterprofiler_plugin
```

测试只可在可选插件 checkout 缺失时跳过。解析或编译失败必须让测试失败。

## Manifest 测试

添加集成测试，要求它：

1. 读取真实 `manifest.toml`。
2. 按 loader 语义内联每个 `script_file`。
3. 断言不可变镜像引用。
4. 找到每个预期节点 kind。
5. 用代表性合法参数值编译 spec。
6. 断言 command、环境变量、输出、artifact 前缀、超时、网络和只读 rootfs。
7. 断言缺失、未知、类型错误、越界和门控违规参数会产生有效错误。

不要在测试中全局设置 `AUTONOMICS_PLUGIN_ROOT`，否则并行测试可能互相看到
临时根目录。

## 脚本与镜像测试

脚本测试至少覆盖：

- 输出 schema 稳定的合法正例。
- 缺少必需输入或列。
- 非法标识符、分隔符、数值或重复键。
- 契约允许时的生物学空结果。
- 每个会改变工具调用的可选分支。

镜像测试必须使用拟发布的镜像构建来源，并把 fixture 工作区挂载到 `/work`。
设置运行时提供的输入/输出变量和插件参数变量，同时断言输出内容与 schema，
而不能只看退出码。

发布前必须运行 push 后的摘要：

```bash
podman pull ghcr.io/auto-nomics/autonomics/clusterprofiler@sha256:<digest>
```

摘要测试能发现 tag 复用、push 失败、registry 复制延迟和误用本地 image ID。

## 面板测试

对每个 `[[panels]]` 条目：

1. 确认目录 dataset 存在且确实服务该插件。
2. 在干净或代表性缓存中运行 `autonomics panels sync`。
3. 校验物化挂载路径和只读行为。
4. 用一个消费者检查预期面板文件和标识符。
5. 在插件 README 记录许可证、上游版本和刷新策略。

daemon 启动只检查本地面板是否存在，不会让就绪状态等待网络下载。因此面板
供给需要作为独立的发布与部署测试。

## 镜像发布

1. 使用已提交的 Dockerfile 和源上下文构建。
2. push 前运行全部镜像和脚本 fixture。
3. 向插件镜像仓库 push 明确版本 tag。
4. 查看 registry 分配的 repository digest：

   ```bash
   podman image inspect \
     ghcr.io/auto-nomics/autonomics/clusterprofiler:<tag> \
     --format '{{json .RepoDigests}}'
   ```

5. 将 `sha256:<64位十六进制>` repository digest 写入 `image.reference`。
6. 保留或更新仅展示用的 `tag` 元数据。
7. 拉取精确 digest 引用并重跑冒烟测试。

绝不发布本地 image ID、可变 tag 引用或全零占位摘要。插件 Git commit 与镜像
摘要共同构成可复现性锁。

## Git 发布

插件仓库必须包含：

- `manifest.toml`
- 所有被引用脚本和静态文件
- 镜像构建上下文与钉版依赖来源
- fixture 和可执行测试脚本
- 许可证与归属信息
- README 契约和运维文档

发布前执行：

```bash
git status --short
git diff --check
git log -1 --format='%H'
```

manifest 摘要必须与其匹配的构建输入放在同一提交中。推送默认或发布分支后，
使用完整 40 位 commit SHA 安装：

```toml
[[plugin]]
name = "clusterprofiler"
git = "https://github.com/auto-nomics/clusterprofiler-plugin.git"
rev = "<完整commit-sha>"
```

tag 和 branch 不是有效安装 revision。镜像或插件源码任一变化，都发布新的不可变对。

## 干净环境分发测试

使用空 state 目录：

1. 只写入已发布的 `[[plugin]]` 条目到 `plugins.toml`。
2. 运行插件 sync，确认 checkout 到精确 commit。
3. 加载全部插件，确认预期 kind 注册。
4. 拉取精确镜像摘要。
5. 同步必需面板。
6. 执行一个代表性 DAG 或 registry 测试。

该测试能发现缺失文件、私有路径假设、意外联网、面板不可用以及与既有家族的
kind 冲突。

## 发布评审

以下情况阻断发布：

- 任一必需测试层缺失，或非因 checkout 缺失而跳过。
- 镜像引用不是真实 push 后的 repository digest。
- 依赖不可复现或未钉版。
- 脚本把用户可控值插入可执行代码。
- 网络出口、可写 rootfs、host user、GPU 或宽松资源限制缺少文档说明。
- 输入、输出、参数、面板、许可证或已知差异文档不完整。
- kind 冲突，或与文档表达的科学契约不一致。
- Git 树不干净、包含无关产物或缺少 fixture。
- 干净环境安装或执行失败。

只有评审者能够根据插件仓库复现镜像 digest、插件 commit SHA、面板绑定、测试
命令和预期节点行为时，才批准发布。
