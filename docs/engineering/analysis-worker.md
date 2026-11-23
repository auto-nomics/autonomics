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

SSH 主机与 Slurm 集群共用自动接入流程。将同一构建的 `autonomics-worker` 放在 gateway 可执行文件旁边后，最小接入命令为：

```sh
autonomics compute enroll ht --backend ssh --login ht --container-runtime-config ./podman.json
autonomics compute enroll Csu --backend slurm --login Csu --partition freecpuQ --container-runtime-config ./sif.json
```

按部署方式提供 gateway 地址和 token。省略 `--worker` 时，系统在远端 `$HOME/.local/share/autonomics/workers/<唯一目录>/autonomics-worker` 部署随附 worker，并校验身份、源码版本与 SHA256，自动生成私有 profile 和端点记录。`--deploy` 可以指定其他本地安装包；显式提供 `--worker` 且省略 `--deploy` 时，只校验该远端已有程序，兼容旧调用。省略 `--backend` 继续按 Slurm 解释，SSH 请求不接受 partition/account。

新 SSH 和 Slurm 目标默认在登录主机创建 `~/.autonomics/compute/<目标ID>`。用 `--storage-root` 指定其他绝对路径，用 `--data-alias` 指定共享文件系统的数据入口。Slurm 根目录必须对登录节点、数据节点和计算节点可见。显式传入旧版 `--data-root` 或 `--scratch` 时沿用旧布局。

统一目录包括 `data/`、`dag-tasks/`、`work/`、`images/`、`runtime/` 和 `panels/`。远端 `data/` 的访问入口是 `<目标域>://`，允许读写，实际能力受 SSH 账号权限约束。任务保留产物有独立的只读域。镜像和运行时目录只由执行器访问。Slurm 的 SIF 文件由运维放入 `images/sif/`，并在目标的清单里登记远端路径和摘要。

已有实例可停机切换。先停止新任务并等待正在运行的任务结束，备份注册表和 VFS 配置。远端建议用新目标 ID 接入统一布局，复制所需数据，准备镜像和清单，验证后选择新目标。保留旧目标对应的数据源和任务目录，供旧 URI 和回滚使用。本地可在 `<state_dir>/storage-layout-v1` 创建标记文件后重启。在启用前，把需要保留的数据和缓存放到 `<state_dir>/data` 和 `<state_dir>/panels`，核对已有 `vfs.toml` 根挂载，原路径上的历史产物应继续可读。新 Podman 仓库位于 `images/podman/`，运行状态位于 `runtime/podman/`。通过镜像仓库或 OCI 归档重新填充镜像，保留旧仓库。回滚时恢复配置并移除标记，再重启。

GUI 的“接入计算目标”统一选择 SSH / Slurm，路径和传输选项收在“高级选项”。TUI 新建远端目标时在 Container runtime JSON 中填写运行时配置；现有目标仍可编辑高级配置。自动接入完成只证明连接和 worker 身份已通过校验，实际任务能力仍需 readiness 和任务执行验收。

在已有 enroll 命令中，将 `--deploy` 改为 worker 的本地绝对路径，`--worker` 仍为远端安装路径：

```sh
./target/release/autonomics compute enroll Csu \
  --login Csu --data-alias Csu --container-runtime-config ./sif.json \
  --storage-root /public/home/hpc268211106/autonomics-compute \
  --worker /public/home/hpc268211106/bin/autonomics-worker \
  --deploy "$PWD/target/release/autonomics-worker" \
  --python /public/home/hpc268211106/.local/share/uv/python/cpython-3.12-linux-x86_64-gnu/bin/python3.12 \
  --partition freecpuQ \
  --token "$(cat ~/.autonomics/gateway.token)"
```

需要自定义安装包时，GUI 的“安装包路径”填写 gateway 主机上的 `target/release/autonomics-worker` 绝对路径。远端 Python 继续用于 SSH 文件传输辅助程序；Rust worker 本身不需要 Python。容器分析按原配置使用 Podman 或 Apptainer，镜像和数据包独立准备。

更新主程序后需重启 gateway，使安装握手生效。gateway 与 worker 的 `source_revision` 必须一致；已有 ComputeTarget 的可执行文件哈希仍然固定，替换远端文件后需更新目标配置。保留历史任务对应的旧路径时，应将新 worker 安装到另一条版本化路径。

TUI 的 Compute Dashboard 中，选择 SSH 或 Slurm 目标后按 `w` 可直接恢复连接。系统使用已保存的 SSH、Python、worker 和站点配置重新连接并校验；远端 worker 已匹配时直接完成。需要更新时，gateway 自动读取与自身可执行文件同目录的 `autonomics-worker`，验证其身份后部署到新的远端路径，更新该目标的当前配置；正在运行的 DAG 继续使用启动时的执行器。发布和安装 gateway 时须将同一源码构建的独立 worker 一起安装到该目录；修复过程无需填写路径或重新配置目标；旧 SSH 目标没有 worker profile 时，也会自动补装并生成配置。

恢复期间 Dashboard 右侧显示实时事件，包括 SSH 认证、stdout/stderr、worker 身份检查、上传进度及目标激活结果。显示保留最近 200 条事件，长行自动换行并跟随最新输出；失败信息保留在事件视图中。完成后按 Esc 返回目标详情。进度连接中断时不会自动重放修复请求，应先刷新检查目标状态。

### Worker 配置的存储

每个 SSH 或 Slurm 目标的 `backend.settings.typed_worker` 随目标一起保存在网关的 `<状态目录>/compute/registry.json`。GUI 可直接编辑这个 JSON 对象。执行和重启直接读取注册表。保存目标时仍可显式导入网关本地的配置文件。离线镜像清单和插件仍由远端主机提供。

### 直接 SSH 目标使用 Podman

SSH 目标在接入和编辑时都要求 Podman。填写远端插件目录、Podman 可执行文件、离线镜像清单及各自的 SHA-256。编辑时，表单会回填已登记的 worker 信息，其他选项可在「高级 worker 配置（JSON）」中修改。Slurm 目标接入时需要网关本地的 SIF 镜像清单，可选择 Apptainer 或 SingularityCE。本地目标使用网关侧 Podman。

命令行接入使用 `--container-runtime-config <json>`。SSH 的 JSON 包含 `type: "podman"`、`plugins_root`、`plugins_sha256` 和带二进制及镜像清单路径与摘要的 `podman` 对象。Slurm 的 JSON 包含 `type: "sif"`、`image_manifest` 和可选的 `runtime`（`apptainer` 或 `singularity_ce`）。

旧注册表中缺少这些配置的未删除远端目标会导致网关启动失败。升级前需要为这些目标补齐运行时配置。

SSH 站点需要将 worker 放在 cgroup v2 委派父目录下的专用叶子里。父目录须可由远端账号写入、没有内部进程，并为子组启用 CPU、内存和 PID 控制器。保存配置验证 profile，远端 worker 在启动容器前验证实际委派和镜像。SSH 进程组无法证明容器与辅助进程都已停止时，取消会报告状态未确认并保留远端工作区。

### Slurm SIF runtime

在「计算目标 → 编辑」中选择 Apptainer 或 SingularityCE。两种运行时使用相同的 OCI digest 到 SIF 路径和 SHA-256 映射。Apptainer 使用清单中的 `program`。SingularityCE 默认使用计算节点上的 `/usr/bin/singularity`，可在目标配置中用 `singularity_executable` 指定其他绝对路径。旧目标省略 `sif_runtime` 时使用 Apptainer。保存配置后，新运行使用更新后的目标，已有运行继续使用其绑定的执行器。


## 计算目标配置

计算目标按 `id` 保存一份当前配置，不再区分 target、policy 或 registry revision。保存同一 `id` 会替换配置；如果该目标已经选中，之后启动的 DAG 自动使用更新后的配置，无需再次选择。现有 DAG 保持启动时绑定的执行器和环境。API 的保存、选择、删除、注册与修复请求均不再传 `expected_revision` 或 `target_revision`。

`compute/registry.json` 使用格式版本 2。加载旧格式时，当前选中的目标保留其原先激活的配置，其余目标保留各自最后保存的配置，之后每个 `id` 只有一条记录。原作业记录迁入按目标 `id` 命名的目录；原执行证据目录继续以只读方式提供访问。已有作业所属的 SSH 连接不能改成另一账户，另一个账户应使用新的目标 `id`。worker 源码版本、程序 SHA256、环境与产物哈希仍用于兼容性校验和溯源。

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
