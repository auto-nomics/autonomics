# Container Development Workspaces

## Model

Container development uses the same k3s data plane as `container_command`, but
replaces the ephemeral Job with a long-running Pod:

```text
create workspace  ->  autonomics-dev-<id> Pod + workspace PVC subPath
container_exec    ->  Kubernetes exec API, captured stdout/stderr
stop workspace    ->  delete Pod, retain PVC subPath
create again      ->  remount the same persistent source tree
image build       ->  Kaniko Job builds an OCI tar from the workspace
```

This is an Agent-facing exec protocol rather than a terminal takeover. The
model issues one captured command at a time; shell process state is not shared
between calls, while `/workspace` and installed files in the container root
filesystem persist only for the life of that Pod. A human can still attach
directly with:

```bash
kubectl -n "$AUTONOMICS_K3S_NAMESPACE" exec -it \
  autonomics-dev-<id> -c dev -- bash
```

## Codex image

Build and import the standard development image into k3s:

```bash
docker build \
  -f crates/container-runtime/src/image/Dockerfile.codex-agent \
  -t docker.io/library/autonomics-codex-agent:latest .

docker save -o /tmp/autonomics-codex-agent.tar \
  docker.io/library/autonomics-codex-agent:latest
sudo k3s ctr images import /tmp/autonomics-codex-agent.tar
```

The image contains the Codex CLI, Node.js, Git, ripgrep, Python, and a basic
C/C++ build toolchain. It runs as the non-root `node` user and keeps the
container alive with `sleep infinity`.

## Agent tools

The runtime host registers five tools for every agent:

- `container_workspace_create`: create or attach to `autonomics-dev-<id>`.
- `container_exec`: run exact argv or a POSIX shell command with captured output.
- `container_workspace_status`: list workspaces or inspect readiness.
- `container_workspace_stop`: stop the Pod without deleting its source tree.
- `container_image_build`: package a workspace as an OCI tar.

Example:

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

`container_exec` defaults to `/workspace`. Its `workdir` field must be an
absolute container path when supplied.

## Image packaging

`container_image_build` writes `.autonomics/image.Dockerfile` and runs:

```text
ARG BASE_IMAGE
FROM ${BASE_IMAGE}
WORKDIR /workspace
COPY . /workspace
```

The result is an OCI tar at:

```text
$AUTONOMICS_K3S_WORKSPACE_ROOT/dev/<id>/.autonomics/image.tar
```

Import it into the node image store with:

```bash
sudo k3s ctr images import \
  "$AUTONOMICS_K3S_WORKSPACE_ROOT/dev/<id>/.autonomics/image.tar"
```

Packaging is intentionally a build, not `docker commit`. Kubernetes does not
expose a safe cross-runtime "commit this Pod rootfs" operation, and a
reproducible build is easier to audit and rebuild. Changes made directly to
the mutable root filesystem (for example `apt-get install`) are not captured
after the Pod restarts. For an image that must preserve those changes, write a
root `Dockerfile` in the workspace with the required installation steps;
`container_image_build` uses that Dockerfile instead of the generated one.

The default builder is:

```text
gcr.io/kaniko-project/executor:v1.23.2
```

Override it with `AUTONOMICS_K3S_IMAGE_BUILDER`.

## Security boundary

- Development Pods are ordinary, non-privileged Pods.
- They do not mount service-account tokens.
- No `hostPath`, Docker socket, or containerd socket is exposed.
- `/workspace` is a PVC subPath and survives Pod deletion.
- The root filesystem is writable for interactive development, so workspace
  packaging is the reproducible path rather than rootfs mutation.
- `egress` permits outbound network access (needed by Codex, package managers,
  and image builders); `cluster` permits DNS only; `isolated` disables both.
