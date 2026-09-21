# Container Execution Design

## Goal

`container_command` is the DAG engine's execution path for external
bioinformatics tools. It runs one OCI image per node as a zero-retry ephemeral
container, uses object storage as the authoritative panel and artifact source,
and keeps a shared POSIX data plane for tools that require ordinary file
access.

Podman is the single execution backend: a rootless-friendly, single-host
runtime driven through the local Podman CLI. The `PodmanConnection` trait in
`crates/container-runtime/src/connection.rs` fixes the capability contract of
the connection layer, and `PodmanRuntime` is its production implementation.

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
remains execution scratch, not long-term state. The control process and Podman
must resolve the same workspace root: `AUTONOMICS_PODMAN_WORKSPACE_ROOT`.

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

## Configuration

Container execution resources are process-level infrastructure. `SharedInfra`
constructs one `ContainerExecutionInfra` from the environment, retains it in an
`Arc`, and injects that same object into `DataEngineBuilder`. Every agent DAG
session shares the Podman connection and panel cache through the shared
node registry.

The `nodes-io` plugin receives that injected object when its node registry is
built; it does not independently construct a second backend.

```text
AUTONOMICS_PODMAN_PROGRAM=podman
AUTONOMICS_PODMAN_WORKSPACE_ROOT=$HOME/.local/state/autonomics/podman/workspace
AUTONOMICS_PANEL_CACHE_ROOT=$HOME/.autonomics/panels
```

`AUTONOMICS_CONTAINER_BACKEND` no longer selects a backend. Setting it to
anything other than `podman` (e.g. the removed `k3s`) fails startup with an
explicit error.

## Podman Backend

One-shot `container_command` workloads execute through a local Podman
runtime. The backend:

- binds the workspace directory read-write at `/work`;
- binds verified panel-cache directories read-only;
- creates a fresh `/tmp` tmpfs;
- applies `--read-only`, `no-new-privileges`, CPU, memory, PID, shared-memory,
  UID/GID, working-directory, and image-pull options from the same request;
- creates a named container, attaches to it, and force-removes it after
  success, failure, or timeout.

Network profiles are `isolated` (default; `--network none`, legacy spelling
`none` accepted) and `egress` (Podman's default network). The Kubernetes-style
`cluster` profile was removed with the k3s backend and is rejected at spec
validation. The control process must be able to execute the Podman CLI and
directly resolve the configured host paths. Consequently, `start.sh` must be
launched on the host or supplied with an explicitly configured remote Podman
endpoint; the default TUI container does not mount a Podman socket.

## Failure Behavior

- Invalid panel reference, missing manifest, checksum mismatch, or size change
  fails before container creation and leaves no public cache entry.
- A Podman create/start deadline removes the named container and returns a
  timeout.
- Failed containers return backend status plus capped output.
- A successful container exit does not complete the node until every declared
  output exists.
- Output upload failure fails the node and does not publish a valid object.
- Concurrent nodes use unique workspaces and container names; artifact paths
  include the run name so publications cannot overwrite each other.

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
the pinned `autonomics/ldsc` manifest digest, with the GHCR namespace supplied
by `AUTONOMICS_IMAGE_PREFIX`, to:

- `ldsc.ref_ld.1000g_eur.basic` at `/panels/ref_ld`
- `ldsc.w_ld.1000g_eur_hm3_no_mhc` at `/panels/w_ld`

It emits `ldsc_h2.log` as a VFS File artifact. The original Rust `ldsc` h²
and `ldsc_rg` node factories are removed. `ldsc_rg_container` follows the same
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
