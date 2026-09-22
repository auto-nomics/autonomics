# Unified Data Catalog

The data catalog turns externally prepared files and directories into immutable,
versioned Hugging Face datasets. The runtime resolves catalog entries into the
shared local panel cache and exposes stable `/bundles/<id>` VFS paths. Container
nodes bind those cached panels without mounting remote storage directly.

## Repository layout

Catalog hosting uses one Hugging Face dataset repository per package:

```text
<owner>/<registry-index>/
  index.json

<owner>/<package-name>/
  index.json
  <version>/<digest>/
    manifest.json
    payload files
```

The registry `index.json` is a thin dependency list:

```json
{
  "schema_version": 2,
  "generation": 30,
  "repositories": ["wjixiang/catalog-gcta-gene-list-hg19"],
  "entries": []
}
```

Each package repository owns its full version index. A package entry records
its package id, HF repository, version, kind, canonical manifest digest, and
current pointer. Payload paths are content-addressed by version and digest.

The Hugging Face repo (`owner/name`) is the durable identity of a package.
The `id` field is kept as a display alias for backward compatibility with
existing DAG bindings; new code can address a package by its repo instead.
[`CatalogIndex`] lookups accept either form: `select(id, ...)` for legacy
lookups and `select_by_repo(owner/name)` for repo-based lookups.

## Runtime configuration

Add a `[catalog]` section to `state_dir/vfs.toml`:

```toml
[catalog]
repository = "wjixiang/catalog-index"
repository_prefix = "wjixiang/catalog"
enabled = true
agent_visible = true
```

`repository` is the registry repository. `repository_prefix` is the default
prefix used when publishing a package that does not specify an exact package
repository. Authentication follows the hf-hub defaults: `HUGGING_FACE_TOKEN`,
`HF_TOKEN`, `HF_TOKEN_PATH`, or the cached token file.

On startup the runtime opens the HF registry, opens the shared local panel
cache, and generates read-only mounts for installed current entries:

```text
/catalog                                  local catalog cache
/datasets/<id>@sha256-<digest>            immutable installed version
/bundles/<id>                             current stable alias
```

The runtime does not mount HF repositories through VFS. Agents and containers
see only verified local cache contents.

## Build, publish, and install

A package input can be a directory, a single file, or a tar archive
(`.tar`, `.tar.gz`, `.tgz`, or `.tar.zst`). A package recipe may be stored as
`package.json` beside the payload.

```bash
cargo run -p data-catalog -- build \
  /input/1000g_eur \
  /packages/1000g_eur-v3 \
  --id 1000g_eur \
  --version v3 \
  --kind vcf
```

The normalized package is:

```text
package/
  manifest.json
  payload/
    chr22.vcf.gz
    chr22.vcf.gz.tbi
```

Validate and publish it:

```bash
cargo run -p data-catalog -- validate /packages/1000g_eur-v3

cargo run -p data-catalog -- publish \
  /packages/1000g_eur-v3 \
  --repo wjixiang/catalog-index \
  --package-repo wjixiang/1000g-eur \
  --create-repo
```

Publishing uploads payload in commits of at most 900 files, commits the
package manifest and package-local index, then registers the package repository
in the thin registry. The registry update is the publication boundary.

Install by package repository:

```bash
cargo run -p data-catalog -- install wjixiang/1000g-eur
```

The local cache index keeps both user declarations and resolved installations:

```json
{
  "schema_version": 2,
  "repositories": ["wjixiang/1000g-eur"],
  "entries": []
}
```

Running `autonomics-catalog update` resolves every declared repository,
downloads missing current versions, verifies every payload checksum, and fills
in the resolved entries.

Search the registry and inspect local state:

```bash
cargo run -p data-catalog -- search
cargo run -p data-catalog -- list
cargo run -p data-catalog -- mounts
```

## Container panel references

Container commands use stable panel ids:

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

The runtime resolves the panel through the local catalog cache and mounts the
verified directory read-only into the ephemeral container.
