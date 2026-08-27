# Container Execution Design

## Goal

`container_command` is the DAG engine's execution path for external
bioinformatics tools. It runs one OCI image per node as a zero-retry ephemeral
container, uses object storage as the authoritative panel and artifact source,
and keeps a shared POSIX data plane for tools that require ordinary file
access.

There are two execution backends: k3s and Podman. Podman is the default for
single-host, rootless-friendly execution. K3s supports multi-node scheduling
through PVCs and NetworkPolicies and remains fully available by setting the
backend explicitly.

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

Each node receives a unique workspace directory. It is mounted at `/work` and
remains execution scratch, not long-term state. The control process and runtime
must resolve the same workspace root: `AUTONOMICS_K3S_WORKSPACE_ROOT` for k3s
or `AUTONOMICS_PODMAN_WORKSPACE_ROOT` for Podman.

### ArtifactRef

Declared outputs are streamed from the workspace to VFS object storage. The DAG
edge receives a `FileRef` whose path is `vfs://...` and whose fingerprint
contains size and SHA-256 rather than a worker-local mtime. The upload uses a
pending object followed by rename so consumers never observe a partial artifact.

## Execution Flow

```text
container_command
  1. resolve a unique workspace path
  2. stage upstream File/Data values from VFS into /work/.autonomics/inputs
  3. materialize and verify each immutable panel
  4. submit one ephemeral backend container
  5. wait for success, timeout, or terminal failure
  6. read capped container output
  7. verify all declared outputs exist
  8. stream outputs to VFS and compute fingerprints
  9. emit FileRef values on output ports
```

## K3s Job Policy

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

Container execution resources are process-level infrastructure. `SharedInfra`
constructs one `ContainerExecutionInfra` from the environment, retains it in an
`Arc`, and injects that same object into `DataEngineBuilder`. Every agent DAG
session shares the selected runtime client and panel cache through the shared
node registry.

The `nodes-io` plugin receives that injected object when its node registry is
built; it does not independently construct a second backend.

```text
AUTONOMICS_CONTAINER_BACKEND=podman
AUTONOMICS_PODMAN_PROGRAM=podman
AUTONOMICS_PODMAN_WORKSPACE_ROOT=$HOME/.local/state/autonomics/podman/workspace
AUTONOMICS_PANEL_CACHE_ROOT=$HOME/.autonomics/panels
```

With `AUTONOMICS_CONTAINER_BACKEND=k3s`, these variables configure the shared
cluster data plane:

```text
AUTONOMICS_K3S_NAMESPACE=autonomics
AUTONOMICS_K3S_CONTEXT=
AUTONOMICS_K3S_WORKSPACE_PVC=autonomics-workspace
AUTONOMICS_K3S_WORKSPACE_ROOT=/var/lib/autonomics/k3s/workspace
AUTONOMICS_K3S_PANEL_PVC=autonomics-panels
AUTONOMICS_PANEL_CACHE_ROOT=$HOME/.autonomics/panels
AUTONOMICS_K3S_PANEL_PVC_PREFIX=
AUTONOMICS_K3S_SERVICE_ACCOUNT=
AUTONOMICS_K3S_POLL_INTERVAL_MS=500
```

The single-node baseline in `infra/k3s/manifests.yaml` creates local PVs and
PVCs plus the isolated and cluster NetworkPolicies. For multiple nodes, replace
those PVs with an RWX distributed filesystem or CSI driver; the runtime and DAG
contract remain unchanged.

## Podman Backend

Set `AUTONOMICS_CONTAINER_BACKEND=podman` to execute one-shot
`container_command` workloads through a local Podman runtime. The backend:

- binds the workspace directory read-write at `/work`;
- binds verified panel-cache directories read-only;
- creates a fresh `/tmp` tmpfs;
- applies `--read-only`, `no-new-privileges`, CPU, memory, PID, shared-memory,
  UID/GID, working-directory, and image-pull options from the same request;
- creates a named container, attaches to it, and force-removes it after
  success, failure, or timeout.

Podman maps `isolated` and legacy `none` networking to `--network none`, and
`egress` to Podman's default network. The Kubernetes-specific `cluster` profile
is rejected explicitly. The control process must be able to execute the Podman
CLI and directly resolve the configured host paths. Consequently, `start.sh`
must be launched on the host or supplied with an explicitly configured remote
Podman endpoint; the default TUI container does not mount a Podman socket.

Persistent development workspaces remain K3s-only. The workspace tools are
registered in Podman mode for configuration compatibility but return a clear
validation error when invoked.

## Failure Behavior

- Invalid panel reference, missing manifest, checksum mismatch, or size change
  fails before container creation and leaves no public cache entry.
- A k3s deadline or Podman attach timeout removes the named workload and
  returns a timeout.
- Failed containers return backend status plus capped output.
- A successful container exit does not complete the node until every declared
  output exists.
- Output upload failure fails the node and does not publish a valid object.
- Concurrent nodes use unique workspaces and container names; artifact paths
  include the run name so publications cannot overwrite each other.

## K3s Distributed DAG Path

For k3s, the current scheduler remains local while remote execution is per node.
Because outputs already carry VFS addresses and content fingerprints, the next
stage can move scheduler decisions without changing node specs:

1. retain workspace and panel cache PVCs for POSIX data locality;
2. add node-affinity labels for cached panels;
3. make `FileRef` consumers always resolve through VFS;
4. profile Job scheduling overhead before introducing a run-level controller
   or workflow engine.

## Catalog-backed panels

`container_command.panel_bundles` references a runtime DataBundle id. The
bundle carries an immutable catalog source and digest without exposing object
keys in the DAG. At execution the node converts it to the same `PanelRef` used
by the inline transition form, verifies its `manifest.json`, materializes it in
the shared panel cache, and mounts it read-only.

## Specialized container nodes

Tool-specific wrappers may sit above `container_command`, but they must not
create a second execution backend. A wrapper owns the tool-image/panel binding,
generates the command and output contract, then delegates to
`ContainerCommandNode`.

For the repeatable migration path from an analysis tool to an OCI image, a
cataloged data package, and a thin DAG wrapper, see
[Container Node Migration Workflow](container-node-migration.md).

`ldsc_h2_container` is the first such wrapper. It accepts one tab-separated
LDSC sumstats File with `SNP`, `A1`, `A2`, `N`, and `Z` columns; plain `.tsv`
and gzip-compressed `.sumstats.gz` are both accepted. It internally binds
`localhost/atc/ldsc:3.0` to:

- `ldsc.ref_ld.1000g_eur.basic` at `/panels/ref_ld`
- `ldsc.w_ld.1000g_eur_hm3_no_mhc` at `/panels/w_ld`

It emits `ldsc_h2.log` as a VFS File artifact. The original Rust `ldsc` h²
and `ldsc_rg` factories are unregistered. `ldsc_rg_container` follows the same
two-File official command pattern and emits `ldsc_rg.log`; `sldsc` and the
other analysis nodes remain unchanged during this staged migration.

`magma_annotate_container` runs the official v1.10 static MAGMA executable and
binds `magma.gene_loc.ncbi37_3`; its native factory is unregistered.
`mrpresso_container` runs the pinned official R MRPRESSO package and emits its
native result object and printed log; its native factory is also unregistered.
`mvmr_container` follows the same file-to-file pattern for the pinned official
MVMR R package; its native factory is likewise unregistered.
Other MAGMA, MiXeR, HDL, MTAG, CPASSOC, LAVA, GenomicSEM, and MR nodes remain
transitional native implementations until their official image, provenance,
reference package, and end-to-end baseline are complete.
