# container-runtime

Runtime-neutral execution primitives for ephemeral analysis containers.

The crate currently provides:

- `ContainerRuntime`: the execution backend trait.
- `PodmanRuntime`: a rootless-first CLI backend for Podman.
- `ImageManager`: local image inspection, listing, pull, remove, and
  policy-driven `ensure_image` operations.
- Declarative request, mount, pull-policy, result, and error types.

The backend intentionally uses the Podman CLI rather than requiring a daemon
API. This keeps the host-owned TUI/runtime and the user's rootless Podman
session in the same security and lifecycle domain.

## Module layout

- `runtime`: the container execution trait and runtime aliases.
- `types`: backend-neutral request, response, mount, and pull-policy types.
- `podman`: the rootless Podman container backend.
- `process`: child-process setup, output capture, timeout, and cleanup.
- `image`: image-management contracts, Podman implementation, and JSON parsing.
- `error`: shared error types.
