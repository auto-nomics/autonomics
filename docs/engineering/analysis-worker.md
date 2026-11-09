# 独立分析 worker

`autonomics` 负责 gateway、交互界面、模型与任务调度；`autonomics-worker` 是单独的 Cargo 包，供计算节点执行分析任务。部署时上传后者。

## 构建和部署

从同一份源码分别构建两个程序：

```sh
cargo build --release -p autonomics
cargo build --release -p autonomics-worker
./target/release/autonomics-worker identity
```

请单独构建 worker。Cargo 在一次构建多个包时会合并共享依赖的 features；同时构建主程序会重新为 worker 带入文献与写作节点。worker 的 release 配置自动去除符号表。分析引擎仍包含 DataFusion、Arrow、统计和机器学习，因此 worker 并非只有网络通信代码。

在已有 enroll 命令中，将 `--deploy` 改为 worker 的本地绝对路径，`--worker` 仍为远端安装路径：

```sh
./target/release/autonomics compute enroll Csu \
  --login Csu --data-alias Csu \
  --data-root /public/home/hpc268211106 \
  --worker /public/home/hpc268211106/bin/autonomics-worker \
  --deploy "$PWD/target/release/autonomics-worker" \
  --python /public/home/hpc268211106/.local/share/uv/python/cpython-3.12-linux-x86_64-gnu/bin/python3.12 \
  --partition freecpuQ \
  --token "$(cat ~/.autonomics/gateway.token)"
```

GUI 的“安装包路径”也填写 gateway 主机上的 `target/release/autonomics-worker` 绝对路径。远端 Python 继续用于 SSH 文件传输辅助程序；Rust worker 本身不需要 Python。容器分析按原配置使用 Podman 或 Apptainer，镜像和数据包独立准备。

更新主程序后需重启 gateway，使安装握手生效。gateway 与 worker 的 `source_revision` 必须一致；已有 ComputeTarget 的可执行文件哈希仍然固定，替换远端文件后需重新注册目标，生成新的配置修订。保留历史任务对应的旧路径时，应将新 worker 安装到另一条版本化路径。

TUI 的 Compute Dashboard 中，选择 Slurm 目标后按 `w` 可直接恢复连接。系统使用已保存的 SSH、Python、worker 和站点配置重新连接并校验；远端 worker 已匹配时直接完成。需要更新时，gateway 自动读取与自身可执行文件同目录的 `autonomics-worker`，验证其身份后部署到新的远端路径，生成目标新修订并恢复原有选择。发布和安装 gateway 时须将同一源码构建的独立 worker 一起安装到该目录；修复过程无需填写路径或重新配置目标。

恢复期间 Dashboard 右侧显示实时事件，包括 SSH 认证、stdout/stderr、worker 身份检查、上传进度及目标激活结果。显示保留最近 200 条事件，长行自动换行并跟随最新输出；失败信息保留在事件视图中。完成后按 Esc 返回目标详情。进度连接中断时不会自动重放修复请求，应先刷新检查目标状态。

## 执行边界

worker 使用 `data-engine/analysis` 功能集，包含统计、SQL、机器学习、深度学习、文件 I/O、数据源与容器插件。文献和写作节点由 coordinator 运行；新注册的 ComputeTarget 自动将这些节点列入 `local_kinds`。worker 的依赖图不包含 gateway、TUI、headless、KMS、文献库或 embedding 模型。

worker 提供两个入口：

- `identity [--plugins-root PATH]`：只读 JSON，报告程序身份、协议版本、源码修订、程序 SHA256，以及可选插件目录哈希。
- `task-worker --dispatch /absolute/attempt/task.json`：执行 transport 生成的一个 typed task。它读取 transport 设置的 `AUTONOMICS_WORKSPACE`、`AUTONOMICS_TASK_ID`、`AUTONOMICS_INPUTS` 和 `AUTONOMICS_INPUT_ARTIFACTS`，不提供交互式分析或服务监听入口。

注册时先计算远端文件 SHA256，再运行 `identity`，拒绝错误程序、协议不兼容、源码不匹配或自身哈希不一致的部署。执行时再次校验程序哈希和源码修订。

worker 将输入与持久化 transport manifest 对照，按 SHA256 验证并创建私有只读快照，然后构造节点注册表并执行任务。输出沿用 `typed-outputs/port-N/` 格式，由现有 transport 回传。未配置插件时使用空插件目录；未配置容器后端时禁用本机默认容器程序。配置 worker VFS 时继续校验 manifest 哈希并保留原始调用者身份。

worker 使用现有 Slurm heartbeat 报告输入校验、运行时准备、执行和输出收集阶段，逐步骤日志写入 stderr，由现有任务日志收集流程回传。该版本支持普通 typed analysis task；不提供私有 signed research/delegated-native 执行入口。

## 验证

```sh
cargo test --release -p autonomics-worker
cargo test -p runtime enroll_from_minimal_input_derives_everything --lib
cargo test -p runtime compute::cluster_connection::tests --lib
```

worker 测试覆盖真实子进程执行 SQL 与 Arrow 输出、文件输入回传、篡改输入拒绝、程序哈希和源码修订不匹配，以及输入快照的只读权限和符号链接处理。连接回归还覆盖 SSH 在接收命令前退出时的 stdout/stderr 保留、禁止重试，以及未送达完整命令时拒绝误报成功。测试在本地完成，不提交真实集群作业。

本次本机 release worker 为 221,826,048 字节（约 212 MiB）；拆分前的本机主程序为 432,618,000 字节（约 413 MiB），直接上传量减少约 48.7%。worker 已去除符号表，原主程序保留符号表；该数字比较的是实际部署文件，不单独衡量架构拆分的收益。具体大小随平台、工具链和 Cargo features 改变。
