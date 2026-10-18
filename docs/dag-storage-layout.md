# DAG storage layout

Autonomics separates a DAG run's storage into three roles. The split follows
the distinction that makes Nextflow workspaces understandable:

1. **Task work directory** — cache and execution workspace.
2. **Published artifact tree** — final deliverables in VFS.
3. **Task receipt tree** — execution evidence and DataFrame evidence.

Keeping these roles explicit prevents a task cache from being mistaken for a
result store, and prevents audit receipts from being mistaken for canonical
data.

## 1. Task work directory

Default:

```text
$AUTONOMICS_DAG_WORK_ROOT
# legacy spelling: $AUTONOMICS_PODMAN_WORKSPACE_ROOT
~/.autonomics/state/work
```

The host directory is mounted into each container at `/work`. It contains the
command, staged inputs, declared outputs, undeclared intermediates, and
`.autonomics` metadata. An auto-generated workdir is content-addressed:

```text
work/{identity[0..2]}/{identity[2..]}
```

This is the resume/cache layer, comparable to a Nextflow task `work/`
directory. If no publish prefix is configured, declared outputs remain here
and travel downstream as local `FileRef`s. A downstream node therefore depends
on this directory until the run's outputs are published or otherwise copied.

The tree is exposed read-only through VFS at its host-absolute identity path
so local work-dir `FileRef`s remain resolvable across consumers. It is not the
publish tree.

Retention is intentionally separate:

```text
AUTONOMICS_WORK_DIR_GC_AGE_SECS
```

Unset, zero, or invalid means persistent workdirs are not reclaimed. A live
task holds `.autonomics-lock`, so GC cannot remove a directory while it is in
use. Deleting an unpublished workdir deletes the only copy of its declared
outputs.

## 2. Published artifact tree

A container node opts into final publication with its Nextflow-style publish
prefix:

```text
artifact_prefix = "/artifacts/experiment"
```

Each declared output is copied atomically to:

```text
vfs://{artifact_prefix}/{run_name}/{output.path}
```

The pending object is hashed and uploaded, then atomically renamed. The value
returned on the node output port is the immutable VFS `FileRef`, including
size and `sha256:` content hash. The work-dir copy remains as a local task
cache, but downstream nodes use the published artifact, not the mutable
work-dir path.

This is the equivalent of Nextflow's `publishDir`:

- the publish tree contains selected, declared deliverables;
- it does not contain every undeclared intermediate;
- publication is a copy, so local cache cleanup does not remove a published
  result;
- a node without `artifact_prefix` behaves like a Nextflow process without
  `publishDir`: its output remains cache-local.

The exact physical backend is selected by the VFS mount table
(`$AUTONOMICS_STATE_DIR/vfs.toml`), so the publish tree may be local or remote
object storage without changing DAG semantics.

## 3. Task receipt tree

Default:

```text
$AUTONOMICS_DAG_RECEIPTS_ROOT
# legacy spelling: $AUTONOMICS_DAG_TASKS_ROOT
~/.autonomics/state/dag-tasks
```

Each local executor attempt creates a receipt such as:

```text
dag-tasks/{pid}-{timestamp}-{sequence}/
  task.json
  status.json
  attempt.json
  stdout.log
  stderr.log
  outputs/port-N.arrow       # DataFrame evidence, not canonical data
```

Receipts are audit evidence, not canonical outputs and not the task cache.
DataFrame evidence is capped by:

```text
AUTONOMICS_DAG_EVIDENCE_MAX_BYTES
```

Retention is controlled independently by:

```text
AUTONOMICS_DAG_TASKS_GC_AGE_SECS
AUTONOMICS_DAG_TASKS_GC_INTERVAL_SECS
AUTONOMICS_DAG_TASKS_GC_MAX_BYTES
```

A receipt owned by a dead process may expire by age; the watermark sweep can
also reclaim older receipts when the tree exceeds its byte ceiling. Receipt
deletion must not delete published VFS artifacts.

## Role matrix

| Role | Default root / namespace | Purpose | Lives after `run_dag` | Cleanup |
|---|---|---|---:|---|
| Task workdir | `state/work` | execution, staging, resume cache | yes | opt-in workdir age GC |
| Published artifact | VFS under `artifact_prefix` | immutable final output | yes | VFS/backend retention policy |
| Receipt | `state/dag-tasks` | manifests, status, logs, DataFrame evidence | yes | DAG receipt age/watermark GC |

## Operator decision rules

- Need to resume a container task cheaply or consume an unpublished local
  output? Keep its workdir.
- Need the result after cache cleanup? Set `artifact_prefix` and publish it.
- Need to know what ran? Keep the receipt (or first copy its small metadata
  into a durable audit store).
- Do not use `dag-tasks` as a result store.
- Do not treat an unpublished `/mnt/.../state/work/...` path as a permanent
  deliverable.

## Central lifecycle & storage governance

The daemon's central `DataEngineManager` owns the process-wide lifecycle
services. Low-level per-tree GC (lock checks for workdirs, owner-liveness for
receipts) remains in place; the central layer never bypasses those safety
checks.

### Session and output-cache lifecycle

Maintenance passes run in a fixed order: release idle output caches → evict
idle session actors → prune the result cache → enforce the local byte budget.

- `AUTONOMICS_DAG_SESSION_IDLE_SECS` (default 86400): sessions with no command
  for this long are stopped. Active runs are never evicted. Eviction targets
  the current *generation*: once a session is being evicted it rejects queued
  runs, and a client asking for the same session id afterwards gets a fresh
  session whose storage references the old actor cannot release.
- `AUTONOMICS_DAG_OUTPUT_CACHE_IDLE_SECS` (default 3600): idle sessions drop
  their in-memory node outputs and fingerprints; topology, history and
  artifacts are untouched, so the next run re-executes instead of leaking
  memory.
- `AUTONOMICS_DAG_MAINTENANCE_INTERVAL_SECS` (default 900; `0` disables the
  background loop).

### Cross-session result cache

Only successful `FileRef` outputs are cached, keyed by node fingerprint;
DataFrame/Channel values stay session-local.

```text
AUTONOMICS_DAG_RESULT_CACHE_TTL_SECS=604800     # 7 days
AUTONOMICS_DAG_RESULT_CACHE_MAX_ENTRIES=4096
```

A restore additionally requires node id + fingerprint match, and re-verifies
the file: missing local files, or files whose size/mtime/content no longer
match the recorded fingerprint, are skipped so the node re-executes. `vfs://`
outputs are validated through the mounted object storage. Files inside the
managed roots that are still referenced by the cache carry a *cache-owner*
reference and survive run-aware cleanup until the entry expires.

### Run-aware clean and the local byte budget

```text
AUTONOMICS_DAG_CLEAN_KEEP_RUNS=100
AUTONOMICS_DAG_STORAGE_MAX_BYTES=0
```

`AUTONOMICS_DAG_STORAGE_MAX_BYTES` is a budget over the **local managed
roots** (`$AUTONOMICS_DAG_WORK_ROOT` + `$AUTONOMICS_DAG_RECEIPTS_ROOT`) only —
not a whole-disk quota. The default `0` disables automatic deletion entirely.
When enabled and over budget, the central cleaner deletes old-run workdirs and
receipt roots oldest-first down to 75% of the budget, after rebuilding the
reference index from `DagHistory`.

Protected from deletion, always:

- directories referenced by an active session or the result cache;
- directories belonging to the newest `keep_runs` runs;
- immutable VFS artifacts, the history DB, and logs.

VFS artifacts (`vfs://` URIs) are counted in the central index (per
run/session/cache-owner, with logical fingerprint bytes) and reported through
`storage_status()` and every clean report, but the conservative policy of this
round never deletes them — their retention belongs to the VFS backend.

The manager exposes two explicit operations:

- `storage_status()` — session count, cache entries, local bytes, tracked
  artifact count/logical bytes, and the reference-index snapshot;
- `clean_storage(keep_runs, dry_run)` — an explicit run-aware clean;
  `dry_run` reports candidate/protected counts without touching the disk.

History-DB retention (dedup, pruning, VACUUM) is deliberately out of scope
here and will be designed separately.

## Current limitations

History reports may still contain absolute workdir and receipt paths. Those
are references, not ownership: receipt GC can invalidate an audit-path lookup,
and unpublished workdir GC can invalidate a cache-local output. The central
run-aware cleaner described above pins those references before deleting either
tree. The storage contract above is the first step: published VFS values are
already independent of the workdir once `artifact_prefix` is set.
