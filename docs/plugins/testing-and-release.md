# Plugin Testing and Release Checklist

This checklist takes a plugin from local validation to an immutable published
family. Do not publish because the manifest parses; publish only when the
contract, image, script, distribution, and documentation have all been tested.

## Test Pyramid

| Level | Question answered | Required evidence |
| --- | --- | --- |
| Manifest load | Does the data contract parse and validate? | Focused Rust test loads and inlines the real manifest. |
| Script fixtures | Does the adapter enforce the contract? | Happy path plus malformed/missing/empty-input cases. |
| Image smoke | Does the exact image run the adapter? | Test executes the published digest with mounted fixtures. |
| Spec compile | Do params and outputs become the intended command? | Golden assertions for defaults, env, outputs, security, and errors. |
| Registry/e2e | Does the node work through the DAG runtime? | Small real or fake DAG run with all declared outputs. |
| Distribution | Can a clean host install it? | Fresh clone at exact SHA, load, registry, image pull, panel sync. |

Run plugin-specific tests with an explicit plugin root rather than changing the
process-wide plugin root:

```bash
NODE_PLUGINS_ROOT=/mnt/projects/node-plugins \
  cargo test -p container-plugin --test clusterprofiler_plugin
```

Tests may skip only when the optional plugin checkout is absent. A parse or
compile failure must fail the test.

## Manifest Test

Add an integration test that:

1. Reads the real `manifest.toml`.
2. Inlines each `script_file` exactly as the loader does.
3. Asserts the immutable image reference.
4. Finds every expected node kind.
5. Compiles representative valid parameter values.
6. Asserts command, environment, outputs, artifact prefix, timeout, network,
   and read-only rootfs behavior.
7. Asserts useful failures for missing, unknown, ill-typed, out-of-bounds, and
   gate-violating parameters.

Do not set `AUTONOMICS_PLUGIN_ROOT` globally in tests. Parallel tests can
otherwise observe one another's temporary roots.

## Script and Image Tests

Script tests should exercise at least:

- A valid happy path with stable output schemas.
- A missing required input or column.
- A malformed identifier, delimiter, numeric value, or duplicate key.
- A biologically valid empty result, if the contract permits one.
- Every optional branch that changes tool invocation.

Image tests must use the same image build provenance intended for release and
mount the fixture workspace at `/work`. Set the runtime-provided input/output
variables and the plugin's parameter variables. Assert both output content and
schema, not only exit status.

Before release, run the pushed digest:

```bash
podman pull ghcr.io/auto-nomics/autonomics/clusterprofiler@sha256:<digest>
```

The digest test catches tag reuse, failed pushes, registry replication lag, and
accidental use of a local image ID.

## Panel Tests

For every `[[panels]]` entry:

1. Confirm the catalog dataset exists and is intended for this plugin.
2. Run `autonomics panels sync` from a clean or representative cache.
3. Verify the materialized mount path and read-only behavior.
4. Exercise one consumer that validates expected panel files and identifiers.
5. Document licensing, upstream version, and refresh policy in the plugin README.

Daemon startup only checks local panel presence; it does not make readiness
depend on network downloads. Panel provisioning is therefore a separate release
and deployment test.

## Image Publication

1. Build from the committed Dockerfile and source context.
2. Run all image and script fixtures before pushing.
3. Push an explicit version tag to the plugin image repository.
4. Inspect the registry-assigned repository digest:

   ```bash
   podman image inspect \
     ghcr.io/auto-nomics/autonomics/clusterprofiler:<tag> \
     --format '{{json .RepoDigests}}'
   ```

5. Copy the `sha256:<64-hex>` repository digest into `image.reference`.
6. Keep or update display-only `tag` metadata.
7. Pull the exact digest reference and rerun the smoke test.

Never publish a local image ID, mutable tag reference, or all-zero placeholder.
The plugin Git commit and image digest form the reproducibility lock.

## Git Publication

The plugin repository must contain:

- `manifest.toml`
- every referenced script and static file
- image build context and pinned dependency provenance
- fixtures and executable test scripts
- licensing and attribution information
- README contract and operational documentation

Before publishing:

```bash
git status --short
git diff --check
git log -1 --format='%H'
```

Commit the manifest digest and its matching build inputs together. Push the
default branch or release branch, then install from a full 40-character commit
SHA:

```toml
[[plugin]]
name = "clusterprofiler"
git = "https://github.com/auto-nomics/clusterprofiler-plugin.git"
rev = "<full-commit-sha>"
```

Tags and branches are not valid installation revisions. If either image or
plugin source changes, publish a new immutable pair.

## Clean-Room Distribution Test

From an empty state directory:

1. Write only the published `[[plugin]]` entry to `plugins.toml`.
2. Run plugin sync and confirm the exact commit is checked out.
3. Load all plugins and confirm expected kinds are registered.
4. Pull the exact image digest.
5. Sync required panels.
6. Execute one representative DAG or registry test.

This catches missing files, private assumptions, accidental network use,
unavailable panels, and kind collisions with existing families.

## Release Review

Block release when:

- Any required test level is missing or skipped without a missing checkout.
- The image reference is not a real pushed repository digest.
- Dependencies are not reproducibly documented or pinned.
- A script interpolates user-controlled values into executable code.
- Network egress, writable rootfs, host users, GPUs, or broad resources lack
  documented justification.
- Input, output, parameter, panel, licensing, or known-difference documentation
  is incomplete.
- Kind names collide or encode a different scientific contract than the docs.
- The Git tree is dirty, contains unrelated artifacts, or omits fixtures.
- Clean-room installation or execution fails.

A release is approved only when a reviewer can reproduce the image digest,
plugin commit SHA, panel bindings, test commands, and expected node behavior
from the plugin repository.
