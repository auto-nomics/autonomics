# 容器内开发工作区

## 模型

容器内开发复用 `container_command` 的 k3s 数据面，但把一次性 Job 换成长期运行的 Pod：

```text
create workspace  ->  autonomics-dev-<id> Pod + workspace PVC subPath
container_exec    ->  Kubernetes exec API，捕获 stdout/stderr
stop workspace    ->  删除 Pod，保留 PVC subPath
create again      ->  重新挂载同一个持久源码树
image build       ->  Kaniko Job 从工作区构建 OCI tar
```

这是面向 Agent 的 exec 协议，不是终端接管。模型每次发出一个可捕获输出的命令；各次 exec 不共享 shell 进程状态，但 `/workspace` 会持久保留。容器根文件系统中的临时安装在 Pod 生命周期内有效。人仍然可以直接进入：

```bash
kubectl -n "$AUTONOMICS_K3S_NAMESPACE" exec -it \
  autonomics-dev-<id> -c dev -- bash
```

## Codex 镜像

构建并导入标准开发镜像：

```bash
docker build \
  -f crates/container-runtime/src/image/Dockerfile.codex-agent \
  -t docker.io/library/autonomics-codex-agent:latest .

docker save -o /tmp/autonomics-codex-agent.tar \
  docker.io/library/autonomics-codex-agent:latest
sudo k3s ctr images import /tmp/autonomics-codex-agent.tar
```

镜像包含 Codex CLI、Node.js、Git、ripgrep、Python 和基础 C/C++ 构建链，使用非 root `node` 用户，并通过 `sleep infinity` 保持运行。

## Agent 工具

运行时为每个 Agent 注册五个工具：

- `container_workspace_create`：创建或接入 `autonomics-dev-<id>`。
- `container_exec`：执行精确 argv 或 POSIX shell 命令并捕获输出。
- `container_workspace_status`：列出工作区或查看就绪状态。
- `container_workspace_stop`：停止 Pod，保留源码树。
- `container_image_build`：把工作区打包为 OCI tar。

示例：

```json
[
  {
    "workspace_id": "codex-demo",
    "image": "docker.io/library/autonomics-codex-agent:latest",
    "network": "egress",
    "cpus": 4,
    "memory": "8Gi"
  },
  {
    "workspace_id": "codex-demo",
    "command": "codex --version && git status --short --branch"
  }
]
```

`container_exec` 默认在 `/workspace` 执行；提供 `workdir` 时必须是容器内绝对路径。

## 镜像打包

`container_image_build` 默认生成：

```dockerfile
ARG BASE_IMAGE
FROM ${BASE_IMAGE}
WORKDIR /workspace
COPY . /workspace
```

产物路径：

```text
$AUTONOMICS_K3S_WORKSPACE_ROOT/dev/<id>/.autonomics/image.tar
```

导入节点镜像库：

```bash
sudo k3s ctr images import \
  "$AUTONOMICS_K3S_WORKSPACE_ROOT/dev/<id>/.autonomics/image.tar"
```

这里刻意采用构建而不是模拟 `docker commit`。Kubernetes 没有安全的跨运行时“提交 Pod 根文件系统”接口；可复现构建也更容易审计和重建。直接 `apt-get install` 到根文件系统的变更不会在 Pod 重启后保留。若最终镜像必须保留系统级变更，请在工作区根目录写一个 `Dockerfile` 记录安装步骤；`container_image_build` 会优先使用这个 Dockerfile。

## 安全边界

- 开发 Pod 是普通非特权 Pod。
- 不挂载 service-account token。
- 不暴露 `hostPath`、Docker socket 或 containerd socket。
- `/workspace` 是 PVC subPath，Pod 删除后仍保留。
- 根文件系统为交互开发保持可写；因此可复现打包以工作区为准。
- `egress` 允许出站网络；`cluster` 仅允许 DNS；`isolated` 禁止入站和出站。
