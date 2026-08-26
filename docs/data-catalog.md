# Unified Data Catalog

The data catalog turns externally prepared files and directories into immutable,
versioned object-storage datasets. Catalog entries are exposed through the
DataBundle registry and stable `/bundles/<id>` VFS paths. Tool wrappers can bind
these entries to compatible images without asking agents to hand-assemble
panel mounts.

## Catalog configuration

Add a `[catalog]` section to `state_dir/vfs.toml`. The backend must also appear
in that file's backend list:

```toml
[[backend]]
id = "warehouse"
type = "s3"
bucket = "autonomics-catalog"
endpoint = "https://garage.example.invalid"

[catalog]
backend = "warehouse"
source = "/"                 # catalog prefix in the backend
index = "index.json"
prefix = "entries"
enabled = true
agent_visible = true
```

On startup the runtime:

1. opens the catalog backend;
2. reads `source/index.json`;
3. validates the index and its current-version invariants;
4. mounts the catalog root at `/catalog`;
5. mounts each current entry at both:
   - `/datasets/<id>@sha256-<digest>` for immutable references;
   - `/bundles/<id>` for stable compatibility aliases;
6. injects each alias into the existing DataBundle catalog.

The runtime also retains a `CatalogService` over the same backend. Agents can
search and inspect it without restarting:

| Tool | Purpose |
|---|---|
| `catalog_search` | Search current datasets by free text, kind, and tags |
| `catalog_describe` | Return manifest metadata, files, digests, and VFS paths |
| `catalog_list_files` | List payload paths, byte sizes, and SHA-256 values |
| `catalog_list_versions` | List historical versions for a dataset id |
| `catalog_refresh` | Reload `index.json` and current manifests |

Search records derive `description` from `metadata.description` (or
`payload.description`) and `tags` from `metadata.tags` / `payload.tags`.
Publication authors should therefore include concise metadata and usage hints
in every package. `catalog_refresh` updates only the discovery snapshot; it
does not rebuild VFS mounts or the DataBundle registry, so restart the runtime
before binding a newly published entry to a DAG node. It also does not add
optimistic concurrency for multiple simultaneous publishers.

User entries in `data_bundles.toml` still override catalog entries. Built-in
entries remain below both. This keeps current LDSC, MAGMA, MiXeR, LAVA, and
HDL nodes untouched while the catalog becomes the authoritative deployment
source.

## Build and publish

A package input can be a directory, a single file, or a tar archive
(`.tar`, `.tar.gz`, `.tgz`, or `.tar.zst`). A package recipe may be stored as
`package.json` beside the payload.

```bash
cargo run -p data-catalog -- build \
  /input/1000g_eur \
  /packages/1000g_eur-v3 \
  --id 1000g_eur \
  --version v3 \
  --kind vcf \
  --metadata population=EUR \
  --metadata genome_build=GRCh37
```

The output is always the normalized layout:

```text
package/
  manifest.json
  payload/
    chr22.vcf.gz
    chr22.vcf.gz.tbi
```

`manifest.json` records canonical metadata, payload size and SHA-256 values,
a type-specific JSON payload, and the canonical digest of the unsigned
manifest. Validate before publication with:

```bash
cargo run -p data-catalog -- validate /packages/1000g_eur-v3
```

Publish using the backend credentials already configured in `vfs.toml`:

```bash
cargo run -p data-catalog -- publish \
  /packages/1000g_eur-v3 \
  --config ~/.autonomics/vfs.toml
```

The publisher writes immutable payload objects and `manifest.json`, then
advances the root `index.json` generation and marks exactly one entry current
per dataset id. Re-publishing the same id and digest is idempotent. The current
index update uses a pending object plus rename; it does not yet provide
multi-writer optimistic concurrency, so use one publishing identity per catalog
or add an external lock service when multiple publishers are required.

Inspect the catalog or generated mounts:

```bash
cargo run -p data-catalog -- list --config ~/.autonomics/vfs.toml
cargo run -p data-catalog -- mounts --config ~/.autonomics/vfs.toml
```

## Object layout

```text
index.json
entries/
  1000g_eur/
    v3/
      sha256-<digest>/
        manifest.json
        chr22.vcf.gz
        chr22.vcf.gz.tbi
```

Only `index.json` is mutable. Version directories are immutable and should
never be overwritten after publication.

## Container panel references

`container_command` can now use a catalog-backed panel without embedding an
object-store prefix:

```json
{
  "panel_bundles": [
    {
      "panel_id": "1000g_eur",
      "mount_path": "/panels/1000g_eur"
    }
  ]
}
```

The runtime resolves the panel through DataBundle metadata, reads
`manifest.json` through `/catalog`, verifies every listed file, and mounts the
resulting shared-cache directory read-only in the k3s Job. The older inline
`panels` form remains supported for transition and tests.

## Migration policy

- Migrate one analysis at a time through the
  [Container Node Migration Workflow](container-node-migration.md).
- Keep an existing native node registered until its OCI image, catalog panels,
  wrapper contract, and end-to-end baseline are ready.
- Publish reference data as catalog packages before switching the node.
- Keep `/bundles/<id>` aliases stable.
- Prefer `/datasets/<id>@sha256-<digest>` in new DAG specs when reproducibility
  must pin an exact version.
- Retain `data_bundles.toml` as a temporary local override mechanism.
- Convert bioinformatics nodes through thin wrappers over `container_command`,
  not by duplicating the container runtime.
