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
DataFrame -- sink_file --> File -- run_command --> File -- source_file --> DataFrame
```

## Running external commands

The `run_command` node executes programs without a shell. Its first release is
file-to-file and is intended for Python, R, shell utilities, and bioinformatics
executables.

Example:

```json
{
  "program": "python",
  "args": [
    "/work/scripts/clean.py",
    "--input", "$input0",
    "--output", "$output0"
  ],
  "outputs": [
    {
      "path": "cleaned.csv",
      "format": "csv"
    }
  ],
  "workdir": "/work/runs/clean",
  "timeout_secs": 3600
}
```

Bindings are replaced only inside individual argv tokens:

- `$input0`, `$input1`, ...: sorted upstream File/FileSet paths.
- `$output0`, `$output1`, ...: resolved declared output paths.
- `$workdir`: resolved working directory.

The node also exports the resolved paths as environment variables, which is the
preferred interface for inline scripts:

- `AUTONOMICS_INPUT0`, `AUTONOMICS_INPUT1`, ...
- `AUTONOMICS_OUTPUT0`, `AUTONOMICS_OUTPUT1`, ...
- `AUTONOMICS_WORKDIR`
- `AUTONOMICS_INPUT_COUNT`, `AUTONOMICS_OUTPUT_COUNT`

Inline script mode is enabled with `script`. The node writes the source to a
private file under its workspace and invokes the configured interpreter with
that file as its first argument. Extra inline assets can be declared in `files`
and are materialized under `AUTONOMICS_FILES_DIR`; their keys must be safe
relative paths. This makes the complete script source part of the DAG manifest
and therefore part of its content hash.

```json
{
  "program": "bash",
  "script": "set -Eeuo pipefail\npython \"$AUTONOMICS_FILES_DIR/clean.py\" --input \"$AUTONOMICS_INPUT0\" --output \"$AUTONOMICS_OUTPUT0\"",
  "files": {
    "clean.py": "import shutil\nshutil.copyfile(__import__('os').environ['AUTONOMICS_INPUT0'], __import__('os').environ['AUTONOMICS_OUTPUT0'])"
  },
  "outputs": [{ "path": "cleaned.csv", "format": "csv" }],
  "timeout_secs": 600
}
```

Bash is launched with `--noprofile --norc` when `program` resolves to `bash`.
On Unix, each child gets its own process group; timeout, cancellation, and
normal completion clean up processes left in that group.

Values in `env` may add or override non-reserved environment variables. The
`AUTONOMICS_INPUT*`, `AUTONOMICS_OUTPUT*`, `AUTONOMICS_WORKDIR`,
`AUTONOMICS_SCRIPT`, and `AUTONOMICS_FILES_DIR` names are reserved.

Relative output paths resolve against `workdir`. When `workdir` is omitted, the
node creates a unique process-local scratch directory. The command must exit
successfully before `timeout_secs`, and every declared output must exist as a
regular file after execution. stdout and stderr are capped at 64 KiB each and
forwarded through the node event stream.

When an OpenDAL filesystem is registered with the engine, absolute File input
paths are staged into the node workspace before the process starts, and absolute
declared outputs are uploaded back to the same virtual path after the process
succeeds. Relative outputs remain local artifacts under the node workspace.

Set `AUTONOMICS_ALLOWED_PROGRAMS` to a comma-separated allowlist to restrict
the executable name, for example:

```text
AUTONOMICS_ALLOWED_PROGRAMS=python,Rscript,samtools,bcftools
```

When the variable is absent, the engine's existing trusted-process boundary
applies. Production deployments should set the allowlist.

## Incremental invalidation

`sink_file` and `run_command` record a local metadata fingerprint for each
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

1. `run_command` stages absolute paths through the registered OpenDAL
   filesystem; relative paths remain local to the node workspace.
2. Program filtering is by executable name. The child inherits the engine
   environment; `env` entries add to or override individual variables.
3. Incremental fingerprinting covers cached local outputs; external source-file
   invalidation still uses the existing manual dirty-mark API.
