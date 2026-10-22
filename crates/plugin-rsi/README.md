# Plugin RSI architecture

`plugin-rsi` is the daemon-owned substrate for supervised plugin and runtime
environment evolution. Agents get narrow tools; this crate owns state, git,
validation, publication, and live-registry transitions.

## Module map

| Path | Responsibility |
| --- | --- |
| `src/foundation` | Errors, plugin names, validation reports, and safe workspace file access |
| `src/intake` | Structured requests, observation feedback, and agent profiles |
| `src/plugin` | Plugin store/operator, lifecycle, source registry, distiller, and workspace validation |
| `src/plugin/validation` | The six deterministic plugin gates, split by concern and orchestrated by `WorkspaceValidator` |
| `src/environment` | Runtime-environment registry, manifests, store, validation, development, distiller, and image publication |
| `src/publication` | Trusted GitHub plugin publication and pull-request adapters |
| `src/runtime` | State layout plus the top-level plugin infrastructure facade |
| `src/vcs` | The sole daemon-owned git adapter |
| `src/tools` | Narrow agent-facing plugin and environment tools |

`lib.rs` is intentionally a facade: runtime and external callers depend on its
re-exported stable API rather than assembling internals themselves.

## Validation flow

`WorkspaceValidator` owns one immutable `ValidationContext`, mutable
`ValidationState`, and an explicit gate sequence:

```text
manifest -> policy -> kind_collision -> script_static -> registry_compile -> secret_scan
```

A failure in a gate that makes later evidence meaningless stops the run and
marks only the not-yet-run downstream gates as blocked. Each policy concern is
isolated in `PolicyValidator`; for example, the greenfield panel restriction is
`PolicyValidator::validate_panel_policy`, making policy changes and issue
diagnosis easy to locate.
