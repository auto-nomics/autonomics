# K3s Container Execution Design

## Goal

`container_command` is the DAG engine's execution path for external
bioinformatics tools. It runs one OCI image per node as a zero-retry Kubernetes
Job, uses object storage as the authoritative panel and artifact source, and
keeps a shared POSIX data plane for tools that require ordinary file access.

There is intentionally one execution backend: k3s.

## Data Contract

### PanelRef

A panel is an immutable object-store prefix:

```json
{
  "id": "1000g_eur",
  "digest": "sha256:...",
  "source": "/panels/1000g_eur/v3",
  "mount_path": "/panels/1000g_eur"
}
```

The prefix must contain `manifest.json`. The manifest lists schema version,
panel id, panel version, bundle digest, and every file's path, size, and
SHA-256 digest. `PanelCache` downloads files into a hidden directory, verifies
all checksums and sizes, writes a completion marker, and atomically renames the
directory to `<id>@<digest>`. A failed or partial download is never mounted.

### WorkspaceRef

Each node receives a unique subPath of the workspace PVC. It is mounted at
`/work` and remains execution scratch, not long-term state. The control process
must see the same PVC at `AUTONOMICS_K3S_WORKSPACE_ROOT` so it can stage inputs
and inspect outputs before publication.

### ArtifactRef

Declared outputs are streamed from the workspace to VFS object storage. The DAG
edge receives a `FileRef` whose path is `vfs://...` and whose fingerprint
contains size and SHA-256 rather than a worker-local mtime. The upload uses a
pending object followed by rename so consumers never observe a partial artifact.

## Execution Flow

```text
container_command
  1. resolve unique workspace PVC subPath
  2. stage upstream File/Data values from VFS into /work/.autonomics/inputs
  3. materialize and verify each immutable panel
  4. submit one batch/v1 Job
  5. wait for success, timeout, or terminal failure
  6. read capped Pod logs
  7. verify all declared outputs exist
  8. stream outputs to VFS and compute fingerprints
  9. emit FileRef values on output ports
```

## Job Policy

- `completions`, `parallelism`: 1
- `backoffLimit`: 0
- `restartPolicy`: Never
- `activeDeadlineSeconds`: node timeout
- `ttlSecondsAfterFinished`: 3600
- no service-account token mount
- non-privileged, no privilege escalation
- read-only root filesystem by default
- `RuntimeDefault` seccomp profile
- control-process uid/gid by default, with non-root enforcement
- `/tmp` emptyDir; optional sized `/dev/shm` emptyDir
- workspace PVC mounted read-write at `/work`
- panel PVC mounted read-only at each declared panel path

Network profiles are labels interpreted by namespace NetworkPolicies:

- `isolated`: deny ingress and egress; this is the default.
- `cluster`: deny ingress and permit DNS only.
- `egress`: explicit administrator-enabled egress profile.
- `none`: accepted as a legacy spelling of `isolated`.

No Job requests a `hostPath` volume. Cluster storage is provisioned as PVCs.
`pids_limit` is carried as the `autonomics.io/pids-limit` annotation because the
upstream Pod API does not expose a per-job PID cgroup field; an admission
controller or kubelet-level `podPidsLimit` policy must enforce it.

## Configuration

The k3s backend is created once by the `nodes-io` plugin and shared by every
node factory instance.

```text
AUTONOMICS_K3S_NAMESPACE=autonomics
AUTONOMICS_K3S_CONTEXT=
AUTONOMICS_K3S_WORKSPACE_PVC=autonomics-workspace
AUTONOMICS_K3S_WORKSPACE_ROOT=/var/lib/autonomics/k3s/workspace
AUTONOMICS_K3S_PANEL_PVC=autonomics-panels
AUTONOMICS_PANEL_CACHE_ROOT=/var/lib/autonomics/k3s
AUTONOMICS_K3S_PANEL_PVC_PREFIX=panels
AUTONOMICS_K3S_SERVICE_ACCOUNT=
AUTONOMICS_K3S_POLL_INTERVAL_MS=500
```

The single-node baseline in `infra/k3s/manifests.yaml` creates local PVs and
PVCs plus the isolated and cluster NetworkPolicies. For multiple nodes, replace
those PVs with an RWX distributed filesystem or CSI driver; the runtime and DAG
contract remain unchanged.

## Failure Behavior

- Invalid panel reference, missing manifest, checksum mismatch, or size change
  fails before Job submission and leaves no public cache entry.
- Job deadline or control-plane timeout deletes the Job and returns a timeout.
- Failed Jobs return the terminal condition plus capped logs.
- A successful container exit does not complete the node until every declared
  output exists.
- Output upload failure fails the node and does not publish a valid object.
- Concurrent nodes use unique workspaces and Job names; artifact paths include
  the Job name so publications cannot overwrite each other.

## Distributed DAG Path

The current scheduler remains local while remote execution is per node. Because
outputs already carry VFS addresses and content fingerprints, the next stage can
move scheduler decisions without changing node specs:

1. retain workspace and panel cache PVCs for POSIX data locality;
2. add node-affinity labels for cached panels;
3. make `FileRef` consumers always resolve through VFS;
4. profile Job scheduling overhead before introducing a run-level controller
   or workflow engine.
