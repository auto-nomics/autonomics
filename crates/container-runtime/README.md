# container-runtime

K3s-only execution primitives for ephemeral analysis containers.

- `ContainerRuntime`: submits one Kubernetes Job per container node.
- `K3sRuntime`: creates, watches, logs, and cleans up Jobs.
- `PanelCache`: downloads immutable object-store panels, verifies SHA-256
  checksums, and atomically publishes POSIX cache directories.
- `PanelRef`, `WorkspaceRef`, and `ContainerRunRequest`: the data contract used
  by `container_command`.

Object storage accessed through the application VFS is authoritative. Workspace
and panel PVCs are the runtime data plane; no `hostPath` volume is exposed to a
workload Pod. Jobs run one non-privileged container with a read-only rootfs,
`RuntimeDefault` seccomp, no service-account token, and a zero-retry Job policy.
