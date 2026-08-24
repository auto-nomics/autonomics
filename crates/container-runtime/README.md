# container-runtime

Runtime-neutral execution primitives for ephemeral analysis containers.

The crate currently provides:

- `ContainerRuntime`: the execution backend trait.
- `PodmanRuntime`: a rootless-first CLI backend for Podman.
- Declarative request, mount, pull-policy, result, and error types.

The backend intentionally uses the Podman CLI rather than requiring a daemon
API. This keeps the host-owned TUI/runtime and the user's rootless Podman
session in the same security and lifecycle domain.
