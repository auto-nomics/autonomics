# autonomics k3s execution

This layout is the single-node k3s baseline. The runtime uses the local
`PersistentVolume` paths both from the control process and from Jobs, but Job
specs never request `hostPath`; they mount the two PVCs only.

## Install

```bash
sudo mkdir -p /var/lib/autonomics/k3s/{workspace,panels}
kubectl apply -f infra/k3s/manifests.yaml
sudo chown -R "$(id -u):$(id -g)" /var/lib/autonomics/k3s
```

Configure the TUI/runtime process with the same paths:

```bash
export AUTONOMICS_K3S_NAMESPACE=autonomics
export AUTONOMICS_K3S_WORKSPACE_PVC=autonomics-workspace
export AUTONOMICS_K3S_WORKSPACE_ROOT=/var/lib/autonomics/k3s/workspace
export AUTONOMICS_K3S_PANEL_PVC=autonomics-panels
export AUTONOMICS_PANEL_CACHE_ROOT=/var/lib/autonomics/k3s/panels
export AUTONOMICS_K3S_PANEL_PVC_PREFIX=
```

`AUTONOMICS_K3S_CONTEXT` optionally selects a kubeconfig context. Jobs never
mount a service-account token and default to the `isolated` NetworkPolicy
profile. `cluster` permits DNS only; unrestricted egress is an explicit node
profile that the cluster administrator must enable separately.

## Development workspaces

Long-running development Pods use the same workspace PVC and the
`autonomics-dev-<id>` naming convention. They select the `egress` network
profile by default. The baseline manifest includes the explicit
`autonomics-egress` NetworkPolicy; remove that object if you want egress to
remain administrator-only.

Set an alternate Kaniko builder image with:

```bash
export AUTONOMICS_K3S_IMAGE_BUILDER=gcr.io/kaniko-project/executor:v1.23.2
```

See [Container Development](../../docs/container-development.md) for the
Codex image, Agent tools, and OCI tar import flow.

## Panel layout

Each panel is an immutable object-store prefix with this layout:

```text
/panels/1000g_eur/v3/
  manifest.json
  chr22/panel.bed
```

`manifest.json` lists every file with its size and SHA-256 digest. The runtime
downloads a panel to `/var/lib/autonomics/k3s/panels/<id>@<digest>`, verifies
all checksums, and atomically publishes the directory. A failed or partial
download is never mounted by a Job.

For multi-node k3s, replace the two local PVs with RWX storage such as NFS,
JuiceFS, Lustre, or a supported distributed CSI driver. The runtime contract
does not change.

## End-to-end test

Give the current user a readable kubeconfig, import the test image into the
node's containerd image store, and run the ignored integration test:

```bash
mkdir -p "$HOME/.kube"
sudo cp /etc/rancher/k3s/k3s.yaml "$HOME/.kube/autonomics-k3s.yaml"
sudo chown "$(id -u):$(id -g)" "$HOME/.kube/autonomics-k3s.yaml"
chmod 600 "$HOME/.kube/autonomics-k3s.yaml"

export KUBECONFIG="$HOME/.kube/autonomics-k3s.yaml"
export AUTONOMICS_K3S_NAMESPACE=autonomics
export AUTONOMICS_K3S_WORKSPACE_PVC=autonomics-workspace
export AUTONOMICS_K3S_WORKSPACE_ROOT=/var/lib/autonomics/k3s/workspace
export AUTONOMICS_K3S_PANEL_PVC=autonomics-panels
export AUTONOMICS_PANEL_CACHE_ROOT=/var/lib/autonomics/k3s/panels
export AUTONOMICS_K3S_PANEL_PVC_PREFIX=
export AUTONOMICS_CONTAINER_IT_IMAGE=docker.io/library/debian:bookworm-slim

sudo k3s ctr images pull "$AUTONOMICS_CONTAINER_IT_IMAGE"
cargo test -p nodes-io real_k3s_copies_input_to_declared_output -- --ignored --nocapture
```
