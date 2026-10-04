# Container Plugin Authoring

This directory is the canonical entry point for building, reviewing, and
releasing manifest-based container plugins. A plugin is a self-contained Git
repository that declares one or more File-to-File analysis nodes, the pinned
container image that executes them, and any versioned data panels they need.

## Document Map

1. [Plugin authoring guide](authoring-guide.md) — the end-to-end tutorial,
   from choosing the node boundary through installing a published family.
2. [Manifest reference](manifest-reference.md) — the normative schema, field
   semantics, template rules, and validation behavior.
3. [Testing and release checklist](testing-and-release.md) — required test
   levels, image publication, Git pinning, and operational acceptance.
4. [Plugin node migration workflow](../plugin-node-migration.md) — the
   additional parity procedure for replacing an existing Rust container
   wrapper.

The older [node-plugin architecture note](../design/node-plugin-architecture.md)
explains the design history. The documents here and the `container-plugin`
source are authoritative when historical proposals differ.

## Lifecycle

```text
define contract → author plugin → validate manifest → test image/script
                → publish image → publish plugin Git repo → pin in plugins.toml
                → startup sync/load → compile node spec → execute container
```

At daemon startup, Autonomics reads the plugin declarations beside its plugin
root (normally `~/.autonomics/plugins.toml`), materializes Git sources at
pinned revisions, scans `~/.autonomics/plugins`, and validates every
`manifest.toml` fail-closed. A single invalid family prevents startup instead
of silently reducing the registry. Each `[[nodes]]` entry becomes a node
factory. When a node runs, its resolved parameters and declared outputs are
compiled into a `ContainerCommandSpec`, and Podman executes the digest-pinned
image.

## Core Rules

- One plugin family is one directory and normally one Git repository.
- Multiple `[[nodes]]` are allowed only when they share the same image and
  panel bindings. Different image or panel needs require separate plugins.
- Plugin nodes are File-to-File. Inputs and outputs are files, not in-memory
  DataFrames.
- `image.reference` must be a full immutable `host/path@sha256:<digest>`
  reference. Tags are metadata only and never resolve the image.
- Prefer `script_file` over an inline `script`. The loader inlines the file at
  startup and rejects unsafe paths.
- Pass user-controlled values through environment variables. Avoid rendering
  parameters directly into executable source.
- Omit `[nodes.resources]` to retain isolated networking and a read-only root
  filesystem. Any override needs an explicit justification in the plugin
  README.
- Do not bundle panel payloads in the plugin. Declare an `[[panels]]` bundle
  reference and let the runtime resolve and checksum it through the catalog.
- Git installation sources require a commit SHA. Tags and branches are
  mutable and are rejected.

## Development Sources

For local iteration, use a `path` source in `~/.autonomics/plugins.toml`:

```toml
[[plugin]]
name = "clusterprofiler"
path = "/mnt/projects/node-plugins/clusterprofiler"
```

Restart `autonomics serve` after changing manifest data or script files. The
published form must use a Git URL and immutable revision:

```toml
[[plugin]]
name = "clusterprofiler"
git = "https://github.com/auto-nomics/clusterprofiler-plugin.git"
rev = "<full-commit-sha>"
```

## Source of Truth

The Rust types and loader are the implementation-level authority:

- `crates/container-plugin/src/node_definition.rs` — manifest node schema and
  cross-field validation.
- `crates/container-plugin/src/manifest.rs` — family and image metadata.
- `crates/container-plugin/src/loader.rs` — fail-closed loading and script
  inlining.
- `crates/container-plugin/src/compile/` — parameter resolution, template
  rendering, and `ContainerCommandSpec` compilation.
- `crates/container-plugin/src/sync.rs` — Git/path source synchronization.

If this documentation and current source disagree, file a documentation or
implementation fix before relying on the disputed behavior.
