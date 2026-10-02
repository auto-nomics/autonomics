# Manifest Reference

This is the normative reference for `manifest.toml` in schema version 1. The
implementation authority is `crates/container-plugin/src/manifest.rs` and
`crates/container-plugin/src/node_definition.rs`. Unknown fields are rejected,
so a manifest that parses here is not forward-compatible with a future schema.

## Minimal Shape

```toml
schema_version = 1
plugin_name = "clusterprofiler"

[image]
reference = "ghcr.io/auto-nomics/autonomics/clusterprofiler@sha256:<64-hex-digest>"

[[nodes]]
kind = "clusterprofiler_ora"
desc = "One-line node description."
doc = "Agent-facing contract documentation."

[nodes.ports]
outputs = [{ path = "result.tsv" }]

[nodes.command]
interpreter = "Rscript"
script_file = "scripts/clusterprofiler_ora.R.sh"
```

`panels` and `nodes` default to empty arrays, but a releasable plugin has at
least one node. `script_file` is preferred over an inline `script`.

## Family Fields

| Field | Required | Meaning |
| --- | --- | --- |
| `schema_version` | yes | Must be `1`. |
| `plugin_name` | yes | Non-empty family name. Use lowercase kebab-case and make it match the repository/install directory name. |
| `image` | yes | Shared executable image metadata for the family. |
| `panels` | no | Catalog panel bindings needed by every node in the family. |
| `nodes` | release required | File-to-File node declarations sharing the image and panel bindings. |

## `[image]`

| Field | Required | Meaning |
| --- | --- | --- |
| `reference` | yes | Full immutable reference `host/path@sha256:<digest>`; no scheme or tag is allowed in the executable reference. |
| `tag` | no | Display-only human tag. It never participates in pull resolution. |
| `upstream` | no | Upstream tool, version, and source revision when applicable. |
| `license` | no | SPDX identifier for the software executed by the image. |

The registry host must be lowercase and may include a numeric port. Repository
path segments must be non-empty lowercase components and cannot be `.` or
`..`. The digest must be `sha256:` followed by exactly 64 hexadecimal
characters. The all-zero placeholder is rejected.

Use the digest returned by the registry after pushing, not a local image ID.

## `[[panels]]`

Each entry binds one catalog panel to one read-only container mount:

```toml
[[panels]]
binding = "gene_sets"
mount = "/panels/gene_sets"
bundle = "example/catalog-gene-sets"
```

- `binding` is the runtime `DataBundle` slot name and must be unique in the family.
- `mount` is an absolute container path and must be unique in the family.
- `bundle` is a Hugging Face dataset id in `owner/name` form.
- All nodes in the family receive the same panel bindings. If only one node
  needs a different image or panel set, split the plugin family.
- Panel payloads are resolved and checksum-verified through the data catalog;
  they are never committed to the plugin repository.

## `[[nodes]]`

| Field | Required | Default | Meaning |
| --- | --- | --- | --- |
| `kind` | yes | — | Workspace-wide registry identifier: non-empty lowercase `[a-z0-9_]`. |
| `desc` | yes | — | Short listing description. |
| `doc` | yes | — | Complete agent-facing input/output/parameter contract. |
| `deprecated` | no | `false` | Marks a legacy node while preserving its kind. |
| `timeout_secs` | no | `3600` | Must be greater than zero. |
| `artifact_prefix` | no | `/artifacts/{kind}` | Absolute VFS prefix used to publish outputs. |
| `ports` | yes | — | File inputs and required outputs. |
| `params` | no | empty | Parameter schema declarations. |
| `command` | yes | — | Container invocation. |
| `resources` | no | hardened defaults | Runtime resource and security profile. |

### `[nodes.ports]`

`inputs` is an array of file-port specifications:

```toml
inputs = [
  { type = "file", label = "gene_table", accepted_formats = ["tsv"] },
]
```

- `type` must currently be `file`; plugin nodes are File-to-File by policy.
- `label` is the DAG port name. Omit it only for an anonymous single input.
- `accepted_formats` makes the first value the primary format label and the
  remainder accepted alternates. Declare it with `label`; format data on an
  anonymous input is not compiled into the port contract.

`outputs` must contain at least one entry:

```toml
outputs = [
  { path = "enrichment.tsv", format = "clusterprofiler_ora_tsv", label = "enrichment" },
]
```

- `path` is relative to `/work`, with no absolute path, `.`, `..`, or NUL.
- Every path must exist after a successful run or the node fails.
- `format` is the label carried downstream.
- `label` defaults to the output file stem.

### `[nodes.params]`

Each key is a parameter name. Its table supports:

| Field | Meaning |
| --- | --- |
| `type` | Required: `bool`, `int`, `number`, `string`, or `string_array`. |
| `default` | JSON value matching `type`. A required parameter has no default and `optional = false`. |
| `optional` | Defaults to `false`. An absent optional resolves to null, which renders as an empty string in `argv`/`env`. |
| `doc` | Agent-facing parameter semantics and units. |
| `min`, `max` | Inclusive numeric bounds. |
| `exclusive_min`, `exclusive_max` | Exclusive numeric bounds. |
| `min_len`, `max_len` | Item-count bounds; valid only for `string_array`. |
| `requires` | Boolean gates: when this parameter resolves to `true`, every named boolean target must also resolve to `true`. |

The compiled JSON Schema is an object with `additionalProperties: false`.
Missing required values, unknown keys, type violations, and bounds violations
fail before a container starts.

### `[nodes.command]`

| Field | Meaning |
| --- | --- |
| `interpreter` | Required `command[0]`, such as `sh`, `bash`, `python`, or `Rscript`. |
| `argv` | Fixed arguments following the interpreter. |
| `script` | Inline script. Mutually exclusive with `script_file`. |
| `script_file` | Relative script path, preferred for reviewable plugins. The loader inlines it before validation. |
| `env` | Additional environment variables. |
| `files` | Static inline text files materialized under `/work/.autonomics/files`. |

`script_file` must use only normal relative components. Absolute paths,
`.`, `..`, and NUL are rejected. Place scripts under `scripts/` in the plugin
repository.

At execution the runtime automatically provides:

- `AUTONOMICS_INPUT0`, `AUTONOMICS_INPUT1`, …
- `AUTONOMICS_OUTPUT0`, `AUTONOMICS_OUTPUT1`, …
- `AUTONOMICS_INPUT_COUNT` and `AUTONOMICS_OUTPUT_COUNT`
- `AUTONOMICS_WORKDIR`
- `AUTONOMICS_SCRIPT` and `AUTONOMICS_FILES_DIR` when a script is present

### Template Rules

A placeholder is `{{ param }}`, where the token contains only ASCII
letters, digits, or `_`. Placeholders may appear in `argv`, `env`, and script
source and must reference a declared parameter.

- `argv` and `env` render checked values without shell re-parsing.
- Null renders as the empty string.
- String arrays join with a single space on `argv`/`env` surfaces.
- Script rendering applies interpreter-aware quoting outside quoted literals
  and comments. Do not rely on this for command construction.
- `files` values are opaque pass-through data in schema version 1; do not put
  placeholders there.

The secure authoring pattern is to render parameters into environment values
and let the script parse and validate them.

### `[nodes.resources]`

Omitting the table retains the hardened defaults:

```toml
[nodes.resources]
read_only_rootfs = true
```

| Field | Values / default | Meaning |
| --- | --- | --- |
| `network` | `isolated` (default), `egress` | `isolated` has no network devices; every `egress` plugin must document why. |
| `read_only_rootfs` | `true` by default | `/work`, `/tmp`, and `/dev/shm` remain writable runtime mounts. |
| `pull_policy` | `missing` default; also `always`, `newer`, `never` | Podman pull policy. Production manifests normally omit it. |
| `cpus` | positive number | CPU limit. |
| `memory` | runtime size string, e.g. `8Gi` | Memory limit. |
| `pids_limit` | positive integer | Process limit. |
| `shm_size` | runtime size string | Shared-memory size. |
| `gpus` | `all`, count, or `device=0,2` | GPU passthrough; absent means no GPU. |
| `user` | image/runtime-specific override | Use only when a non-root internal user is required. |

Resource overrides are part of the security review. The plugin README must
explain every explicit override.

## Validation Behavior

Startup loading is fail-closed. A plugin release is blocked when:

- TOML parsing fails or an unknown field is present.
- The image reference or panel bundle id is invalid.
- A panel mount or binding is duplicated.
- `script` and `script_file` are both set, or a script path is unsafe/missing.
- A node kind is malformed, times out at zero, or has no outputs.
- An output path is unsafe.
- A parameter uses array length bounds on a non-array type.
- A boolean gate names a missing or non-boolean target.
- A command placeholder references an undeclared parameter.
- A node kind collides with another plugin loaded in the same root.

Parameter defaults, types, bounds, and gates are exercised again when a node
spec is compiled for execution.
