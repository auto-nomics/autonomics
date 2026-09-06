# container-runtime

Execution primitives for ephemeral analysis containers on Podman.

- `PodmanConnection`: the capability contract of the Podman connection layer.
  One request maps to one ephemeral container: create it, attach to its
  start, force-remove it after success, failure, or timeout.
- `PodmanRuntime`: the production connection, implemented on the local Podman
  CLI (`podman create` / `start --attach` / `rm --force`). No shell is
  involved; requests are translated to argv only.
- `PanelCache`: downloads immutable object-store panels, verifies SHA-256
  checksums, and atomically publishes POSIX cache directories.
- `PanelRef`, `WorkspaceRef`, and `ContainerRunRequest`: the data contract used
  by `container_command`.

Object storage accessed through the application VFS is authoritative. The
shared workspace tree and panel cache are the runtime data plane; the control
process and Podman must resolve them on the same host. Containers run with
`no-new-privileges`, a read-only rootfs by default, resource limits, and
`--userns keep-id`, with stdin detached so attached starts never steal the
host terminal.
