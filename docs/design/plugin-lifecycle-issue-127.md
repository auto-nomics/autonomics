# Plugin lifecycle closure plan

Status: accepted design; P0 is implemented, P1/P2 are sequenced implementation work.

This plan closes the gaps recorded in [issue #127](https://github.com/auto-nomics/autonomics/issues/127): development and runtime state must be separable, observable, and recoverable without destroying a plugin author's work.

## Runtime boundary

A development workspace is mutable and remains in its editable status. `plugin_install` validates it, commits the evidence, and installs an immutable local snapshot selected by commit and tree digest. The live DAG registry can only point at that snapshot. Development edits do not claim to update runtime state.

`engine_inspect_cache(plugin_path)` is the diagnostic boundary between those two worlds. It reads the active manifest, every referenced `script_file`/inline script, and the development workspace from disk, then returns both exact contents, SHA-256 identities, stale-script counts, and unified diffs. After editing a script, an author can therefore prove whether the DAG will run the old or new copy before reinstalling.

Reinstall is the controlled transition from draft changes to a new immutable runtime pin. Existing tests require all of the following together:

1. a previously active plugin can be installed again without leaving the draft status;
2. script changes are reported stale before reinstall;
3. the clean reinstall points the runtime at the changed script;
4. inspection then reports active and development content identical.

This intentionally avoids a `force` flag that bypasses validation or edits the active mount directly.

## Cross-environment artifacts and multi-file ports

The plugin DSL will add two explicit, statically validated port extensions:

- A variadic file input: `{ type = "file", variadic = true, min = 1, max = 32 }`.
  The compiled schema accepts one to `max` path bindings. Execution receives
  `AUTONOMICS_INPUT_COUNT`, a stable-order JSON manifest in
  `AUTONOMICS_INPUTS_JSON`, and the existing indexed variables. `min` and `max`
  are graph-time invariants, not script conventions.
- A tar archive file output: `{ path = "result.tar.gz", archive = "tar_gz" }`.
  The runtime verifies that the path exists and is a valid gzip-compressed tar
  archive before marking the node successful. Its downstream format is `tar_gz`,
  and optional `members` metadata names the expected member suffixes for contract
  validation without requiring consumers to unpack during graph compilation.

Cross-environment edges use VFS artifact descriptors rather than host paths:

```text
{
  vfs_uri: "vfs://runs/<run>/nodes/<node>/<output>",
  digest: "sha256:<hex>",
  format: "<output format>",
  media_type: "<IANA type>"
}
```

An environment manifest gains optional `provides` and `consumes` arrays keyed by
these formats and media types. Graph construction validates that every edge whose
environments differ has a producer format accepted by the consumer, and marks the
edge `cross_environment: true` in run logs. Containers only receive staged VFS
artifacts. Host locations remain admin-configured mounts and are never exposed as
node contracts; a future whitelist only controls which safe host tree is backing
a named VFS mount.

Implementation is sequenced as:

1. Add variadic schema and execution staging behind a manifest schema bump.
2. Add deterministic archive validation and format propagation.
3. Add environment `provides`/`consumes` and static cross-edge validation.
4. Emit artifact descriptors and cross-edge markers in run reports.

## Versioning and governance

The governance model keeps content-addressed, reviewable versions:

- Plugins: local activation records commit, tree digest, validation report, and
  active-source kind; active source pins are immutable. Node `deprecated = true`
  preserves existing graphs while warning and rejecting new node creation.
  Archive/deprecation will remove only the live registration and retain the
  development workspace and immutable snapshots for audit and rollback.
- Images: approved environments are digest-pinned. Image build will emit an
  `installed_packages.json` manifest containing package manager, name, epoch,
  version, architecture, and license where available, and include that manifest
  digest in the environment/image identity.
- Data packages: package manifests already carry schema version, dataset version,
  per-file SHA-256 checksums, and metadata. Promotion and deprecation will operate
  on signed package identities, never mutable dataset directory state.

This makes plugin, image, and dataset governance a common rule: name a reviewed
immutable identity, verify content, preserve history, and make removal from the
live set reversible.
