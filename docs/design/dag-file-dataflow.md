# DataFrame and file dataflow

This document describes the mixed DataFrame/File execution model used by the
DAG engine. The design keeps DataFrame execution lazy while allowing external
programs to participate through explicit file artifacts.

## Data model

Every DAG edge carries one typed value:

- `NodeValue::DataFrame`: a DataFusion `DataFrame`.
- `NodeValue::File`: one file artifact address.
- `NodeValue::FileSet`: an ordered collection of file artifact addresses.

Ports declare the expected payload with `PortType::DataFrame`, `PortType::File`,
`PortType::FileSet`, or `PortType::Any`. Validation happens in two layers:

1. Graph validation checks declared output/input port types before execution.
2. The scheduler checks actual runtime values against declared input and output
   port types.

DataFrame-to-DataFrame edges retain the existing Arrow schema compatibility
check. Non-DataFrame edges only require matching payload types.

## Bridge nodes

`sink_file` is now a bridge rather than a terminal sink:

```text
DataFrame input -> File output
```

Its write behavior is unchanged, but after a successful write it emits a
`FileRef` containing the normalized path, format, and local filesystem
fingerprint when available.

`source_file` supports both its legacy form and the file bridge form:

- With no upstream edge, it reads `spec.path` exactly as before.
- With an upstream File edge, it reads the upstream `FileRef.path` and ignores
  the fallback path.
- The effective format order is explicit spec format, upstream FileRef format,
  then file extension.

A path-less `source_file` can be added to the graph before it is connected.
Running one without an upstream file and without a fallback path fails at node
execution.

The canonical round trip is:

```text
DataFrame -- sink_file --> File -- container_command --> File -- source_file --> DataFrame
existing file -- file_ref_source --> File -- container_command --> File
```

## Running container commands

The `container_command` node runs a file-to-file external command in an
ephemeral k3s Job. Object storage through the runtime VFS is authoritative;
the Job sees a workspace PVC subPath and immutable read-only panel mounts.

```json
{
  "image": "docker.io/biocontainers/samtools:v1.21",
  "command": [
    "samtools", "view",
    "-b", "$input0",
    "-o", "$output0"
  ],
  "outputs": [{ "path": "result.bam", "format": "bam" }],
  "timeout_secs": 3600,
  "network": "isolated",
  "pull_policy": "missing",
  "panels": [
    {
      "id": "1000g_eur",
      "digest": "sha256:...",
      "source": "/panels/1000g_eur/v3",
      "mount_path": "/panels/1000g_eur"
    }
  ]
}
```

The workspace PVC subPath is mounted at `/work`. Inputs are materialized under
`/work/.autonomics/inputs`, inline scripts under
`/work/.autonomics/script`, and helper files under `/work/.autonomics/files`.
The node exposes the standard `AUTONOMICS_INPUT*` / `AUTONOMICS_OUTPUT*`
environment contract, with all paths rewritten to their `/work` equivalents.

By default the Job has an isolated network profile, a read-only root
filesystem, `RuntimeDefault` seccomp, no service-account token, no privilege
escalation, and the control process uid/gid. Panels are verified against their
object-store manifest before the cache directory becomes visible. Production
images should be referenced by digest, and only tools that genuinely need
network access should use `cluster` or `egress`.

Declared output paths must be safe paths relative to `/work`. After a
successful exit, every output must exist as a regular file. The node streams it
to VFS object storage, computes SHA-256, and emits a remote `FileRef`; the
workspace copy remains only as execution scratch.

## Incremental invalidation

`sink_file` and `container_command` record a local metadata fingerprint for each
emitted file. Before an incremental run, clean nodes with cached File or
FileSet outputs are checked. If size or nanosecond mtime differs, the node and
its descendants are marked dirty and re-executed.

This detects local artifact mutation. Remote object-store fingerprints and
opt-in content hashes are future extensions.

## Code generation

The R code generator distinguishes edge payload types:

- DataFrame edges continue to materialize `_edge_{node}_{port}.csv` files.
- File edges pass path-bearing variables directly.

For example, `sink_file -> source_file` assigns the written path to the sink's
output variable and passes that variable to `source_file`; no edge CSV is
generated for the File edge.

## Migration notes

Node implementations that consume tabular input now call
`NodeInput::dataframe()` and receive a clear error if a File value is connected
by mistake. Constructors for tests use `NodeInput::new_dataframe(port, df)`.
Output construction remains `PortOutputs::insert(port, df)` for DataFrames and
`PortOutputs::insert_file(port, file)` for files.

The first release has three deliberate boundaries:

1. `container_command` stages absolute paths through the registered OpenDAL
   filesystem; relative paths remain local to the node workspace.
2. Network access defaults to `none`; explicit `mounts` are the only
   additional host paths the workload can reach.
3. Incremental fingerprinting covers cached local outputs; external source-file
   invalidation still uses the existing manual dirty-mark API.
