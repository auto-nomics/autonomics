# Plugin Node Migration Workflow

The canonical workflow for migrating a hardcoded container node into a
manifest plugin. This is the plugin-era successor to
[Container Node Migration Workflow](container-node-migration.md): that
document describes how a tool becomes an image + catalog panel + thin
wrapper; this one describes how that thin wrapper becomes a data-only
manifest, published as a git repository.

The reference implementation is the `ldsc` family (`ldsc_h2`,
`ldsc_munge`, `ldsc_rg`), migrated end to end and
exercised by `crates/container-plugin/tests/ldsc_migration.rs` (golden)
and `ldsc_e2e.rs` (distribution).

## Overview

One plugin = one directory = one git repository. A plugin directory is a
self-contained unit that holds everything a tool node needs to exist:

```text
ldsc/
├── manifest.toml          # node contract: params, ports, panels, image
├── scripts/               # execution scripts, referenced relatively
│   ├── h2.sh
│   ├── munge.sh
│   └── rg.sh
├── Dockerfile             # image provenance (build + push still via GHCR)
├── ldsc-python3/          # vendored upstream source (no embedded .git)
├── test_*.sh              # image baselines
└── README.md
```

At startup the runtime host runs `container_plugin::sync::sync` (from
`state_dir/plugins.toml`), materializing each plugin under
`state_dir/plugins`, then the loader scans that root, validates every
`manifest.toml` fail-closed, and registers one `ManifestNodeFactory` per
`[[nodes]]` entry.

## Scope

Migrate when the node is a `container_command`-backed wrapper: one or
more related tool invocations sharing an image and panel set. Do **not**
migrate pure-Rust in-process transforms — they have no image, no panels,
and no reason to leave the compile-time registry.

Native FFI bundles are also in scope when the vendored code carries a
contaminating license or a native ABI the binaries should not link
(statically-linked GPL C++, libstdc++/pthread): those must leave the
process tree even though they were never `container_command` wrappers.
The `grf` family is the reference case — 23 in-process nodes over a
40 MB vendored GPL-3 C++ core, rebuilt as file-port plugin nodes backed
by the official R package, with the forest-exchange Arrow blob replaced
by a `forest.rds` artifact.

A plugin family may hold several `[[nodes]]` when (and only when) they
share the image and panel bindings — `ldsc`'s h²/munge/rg share one
image and one panel pair, so they are one plugin. If two analyses need
different images or different panels, they are two plugins.

## Step 0: Pre-flight DSL capability check

Before writing any TOML, read the old wrapper's `XxxSpec`, `validate`,
and `container_spec`, and confirm the plugin DSL can express every
behaviour. The three things that most often do **not** map directly:

1. **Optional flags** — a param with no default whose absence means
   "omit the flag" (e.g. `intercept_h2`, `chisq_max`). Express it as
   `optional = true`, pass it through `env`, and let the script test
   `[ -n "$VAR" ]`. The v0 template language has no conditionals; the
   script owns all `if` logic.
2. **Gzip input handling** — the legacy `decompress_gzip_inputs` helper
   has no plugin equivalent. Inline a `case *.gz) gzip -dc …` stanza in
   the script itself.
3. **Column-name / list overrides** (munge's nine column flags) — each
   becomes an `optional = true` string param carried via a dedicated env
   variable, with the script building its own `--flag $val` list.

If the DSL genuinely cannot express something (a new param type, array
subscripting, conditional templates), stop and extend
`crates/container-plugin` **before** migrating — do not special-case it
inside the manifest or the script.

## Step 1: Create the plugin directory

Create `<tool>/` under the plugin checkout root (development path
`/mnt/projects/node-plugins/`), with:

- `manifest.toml` — see the authoring conventions below.
- `scripts/*.sh` — the execution script, referenced via
  `script_file = "scripts/<name>.sh"`.
- `Dockerfile` + vendored source + `test_*.sh` — moved verbatim from
  `containers/<tool>/` (repoint the test scripts' `root=` to the plugin
  directory).
- `README.md` — provenance, layout, migration-parity notes.

**Move the image build tree in the same change.** The plugin is the
single source of truth for the tool; a wrapper whose Dockerfile still
lives under `containers/` is only half-migrated.

If a fixture under `containers/<tool>/` is referenced by a still-Rust
test (as `ldsc_munge.rs` referenced the vendored munge fixture),
repoint that test to the plugin directory, falling back to a skip when
the plugin checkout is absent.

## Step 2: Golden parity test

Add a test under `crates/container-plugin/tests/<tool>_migration.rs` that
compiles the manifest and asserts field-for-field parity with the old
wrapper's `container_spec`. The pattern (see `ldsc_migration.rs`):

```rust
let manifest = load_manifest(&root);            // TOML + inline script_file
let node = node_by_kind(&manifest, "…");
let compiled = compile_container_spec(node, &manifest.image,
                                      &manifest.panels, &json!({}))?;

assert_eq!(compiled.image, "<full digest-pinned reference>");
assert_eq!(compiled.outputs[0].path, "….log");
assert_eq!(compiled.outputs[0].format.as_deref(), Some("…_log"));
assert_eq!(compiled.panel_bundles[0].panel_id, "wjixiang/catalog-…");
assert_eq!(compiled.timeout_secs, …);
assert_eq!(compiled.artifact_prefix, "/artifacts/…");
// semantic script markers, not byte equality
assert!(script.contains("--ref-ld-chr /panels/ref_ld/LDscore."));
```

Two rules make the golden test honest:

- **Image/outputs/panels/resources are byte-exact.** The whole point of
  the migration is that the compiled spec is the same contract.
- **Script is semantic, not byte-exact.** The plugin drives flags through
  env and its own `if` stanzas instead of Rust string-building, so
  variable names and layout differ. Assert on the load-bearing tokens
  (`--ref-ld-chr /panels/…`, `--out "$AUTONOMICS_WORKDIR/…"`, the gzip
  `case` stanza), and record any deliberate variable-name delta
  (`OUT_PREFIX` vs legacy `out_prefix`) in a comment.

## Step 3: Publish to git

```sh
git init --initial-branch=main
# vendored source must be plain files, not a gitlink:
rm -rf <vendored>/.git
git add -A && git commit
gh repo create auto-nomics/<tool>-plugin --private --source=.
git push -u origin main
REV=$(git rev-parse HEAD)
```

Then declare the source in `~/.autonomics/plugins.toml` (or the
deployment's equivalent):

```toml
[[plugin]]
name = "ldsc"
git = "git@github.com:auto-nomics/ldsc-plugin.git"
rev = "6f7118d61dd60ca7ce95d7d524ccec3880d96026"   # pinned commit SHA
```

**Pin `rev` to a commit SHA.** Tags and branches move; images are
digest-pinned, data bundles are digest-pinned, and plugin code is
rev-pinned — the same immutability rule one layer up. `sync` rejects
non-SHA refs.

Migration is not complete until the published rev passes the wave
end-to-end test (the `plugin_wave_e2e.rs` pattern: sync every declared
family, load, register, assert kinds). Families migrated in the same
wave are declared together in `plugins.toml` and verified by one shared
e2e test, so the install path is exercised as a unit before the wave
ships.

## Step 4: Remove the old wrapper

Delete `crates/node-bundles/nodes-io/src/<tool>_container.rs`, its
`pub mod` line, and its `registry.register(…)` block. Then sweep for
residual references:

```sh
grep -rn "<tool>_container::" crates/ apps/ --include='*.rs'
```

Everything that referenced the wrapper must move to the plugin or be
deleted:

- kind-list expectations in `data-engine` tests (`list_nodes`, `get_node_spec`)
- `catalog()` bundle fixtures and build cases in
  `data-engine/tests/bundle_bound_nodes.rs`
- any `use nodes_io::<tool>_container::…` in integration tests

**Migrate the whole family at once, or accept an explicitly-marked
temporary shared-constants module.** The `ldsc` pilot first migrated only
`ldsc_h2`, leaving `ldsc_munge`/`ldsc_rg` pointing at a throwaway
`ldsc_image` constants module — that module became dead code the moment
the rest of the family landed and had to be deleted. When a family shares
one image and panel set, prefer migrating every `[[nodes]]` entry in one
change.

## Manifest authoring conventions

```toml
schema_version = 1
plugin_name = "ldsc"

[image]
reference = "ghcr.io/auto-nomics/autonomics/ldsc@sha256:2dad70…"
tag = "3.0.1-allele-filter"          # display only; never resolves
upstream = "CBIIT LDSC 3.0.1 @ 6c67395"
license = "BSD-3-Clause"

[[panels]]
binding = "ref_ld"                     # DataBundleBinding.binding
mount = "/panels/ref_ld"
bundle = "wjixiang/catalog-ldsc-ref-ld-1000g-eur-basic"   # HfRepoId

[[nodes]]
kind = "ldsc_h2"
desc = "…"
doc = """…"""                          # agent reads this — write for them
timeout_secs = 900
artifact_prefix = "/artifacts/ldsc_h2"

[nodes.ports]
inputs = [{ type = "file" }]
outputs = [{ path = "ldsc_h2.log", format = "ldsc_log" }]

[nodes.params]
n_blocks = { type = "int", default = 200, min = 2.0, doc = "…" }
intercept_h2 = { type = "number", optional = true, doc = "…" }

[nodes.command]
interpreter = "sh"
script_file = "scripts/h2.sh"

[nodes.command.env]
LDSC_N_BLOCKS = "{{ n_blocks }}"
LDSC_INTERCEPT_H2 = "{{ intercept_h2 }}"
```

Rules of thumb:

- `kind` drops the legacy `_container` suffix (`ldsc_h2_container` →
  `ldsc_h2`): that suffix only ever distinguished a container wrapper from
  a native Rust port, and the native ports are gone. DAG specs referencing
  the old kind must be regenerated.
- `image.reference` is the full `host/path@sha256:` string; no implicit
  namespace, no env override.
- `panel.bundle` is an `HfRepoId` (`owner/name`); it resolves through the
  runtime `DataBundle` directory, never a raw object key. The bundle
  itself is **not** shipped in the plugin: `autonomics panels sync`
  downloads the current entry for each referenced repo into the local
  catalog cache (the startup preflight only checks presence locally and
  points at that command when bundles are missing).
- Params with a `default` appear in the schema's `properties`; params
  without a default and without `optional` appear in `required`; `optional`
  params resolve to null and render as empty strings.
- Prefer `script_file` over inline `script` for anything longer than a few
  lines; the loader inlines the file before validation, and the two fields
  are mutually exclusive.
- `env` is the canonical channel for params the script tests with
  `[ -n … ]`; argv is for static runner arguments.

## Known pitfalls

1. **`Resources::default()` flips the hardened default.** `Default::derive`
   sets `read_only_rootfs = false`; the field's `#[serde(default = "default_true")]`
   only fires when the table is present but the key is missing. `Resources`
   must `impl Default` manually (it does), or a manifest that omits
   `[nodes.resources]` silently relaxes the read-only rootfs.
2. **`nodes = []` conflicts with `[[nodes]]`.** A TOML array-of-tables
   cannot also be declared as an empty inline array. Omit `nodes` entirely
   when the family has entries.
3. **The renderer treats `#` comments and single quotes literally.** A prose
   apostrophe in a script (`wrapper's`) was once parsed as an unterminated
   quote. Comments (`#` to end of line) and quoted regions are passed
   through verbatim; `{{param}}` inside quotes is intentionally not
   resolved.
4. **A lone `{` must not hang the scanner.** Only `{{` opens a template;
   a single `{` is a literal byte. (This once produced an infinite loop in
   the byte scanner — covered by a regression test.)
5. **PanelCache verifies checksums.** An e2e test that stubs a panel with
   a bogus `manifest.json` fails at materialization. Provide a real panel
   manifest + payload in the fake object store.
6. **Plugin-root discovery is process-global.** Never set
   `AUTONOMICS_PLUGIN_ROOT` inside a test — a parallel registry build can
   observe a half-written plugin. Pass the root explicitly through
   `build_default_registry_with_container_execution(…, plugins_root)`.
7. **Vendored source must not be a gitlink.** A copied upstream checkout
   carries its own `.git`; commit it as plain files (`rm -rf <vendored>/.git`)
   or clones of the plugin will lack the payload.
8. **f64 env rendering drops trailing `.0`.** `30.0` renders as `"30.0"`
   (serde_json) where the legacy `format!` produced `"30"`. Assert with the
   serde_json spelling; the values are equal after float parsing.

## Acceptance checklist

- [ ] Every `[[nodes]]` kind is the legacy `*_CONTAINER_KIND` minus the
      `_container` suffix.
- [ ] `image.reference` is digest-pinned; `upstream`/`license` are filled.
- [ ] Panels are `HfRepoId`s bound through `panel_bundles`, not raw `panels`.
- [ ] The image build tree moved out of `containers/<tool>/`.
- [ ] Golden test asserts byte-exact image/outputs/panels/resources and
      semantic script markers.
- [ ] Plugin pushed to git at a pinned `rev`; `plugins.toml` references it.
- [ ] Old wrapper file + `pub mod` + `register(…)` removed.
- [ ] `grep "<tool>_container::"` is empty across `crates/ apps/`.
- [ ] `cargo test -p nodes-io -p data-engine -p container-plugin` green.
- [ ] `ldsc_e2e`-style distribution test (sync → load → build → execute)
      passes against the pushed repository.
