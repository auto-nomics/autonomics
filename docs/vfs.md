# Virtual File System

Autonomics addresses files by filesystem domain. Canonical public addresses use
`<domain>://path`, for example `local://work/input.tsv` or
`ht://share/input.tsv`. A domain identifies a filesystem partition declared by
a mount; compute targets declare which domains they can access in place. A
compute target name and a filesystem domain name need not be identical.

`local` names the coordinator's mounted local namespace. It does not mean the
current worker's host filesystem. Bare host paths and `file://` retain their
existing host-path meaning at DAG boundaries. `vfs:///...` remains a legacy
address for the composite mount namespace; new published references use domain
URIs. The examples below using `vfs://` also document this compatibility form.
Existing three-slash domain addresses remain readable and canonicalize to the
two-slash form. `ht://share/input.tsv` means path `/share/input.tsv` in domain
`ht`; `share` is a path component, not a hostname. The domain root is `ht://`.
At DataFusion's URL boundary, use `vfs::uri::datafusion_uri(address)`; engine
file nodes apply this adapter automatically so standard URL parsing preserves
the first path component and existing percent escapes.

Historical compute/source identifiers may contain capitals or underscores.
Their filesystem addresses use lowercase with underscores replaced by dashes
(for example source `Csu` at `/data/Csu` publishes `csu://...`). A leading digit
or reserved scheme gets a `source-` prefix. Saved target revisions and mount
paths retain their original spelling; conflicting domain names are rejected.

## Agent address contract

The `vfs` tool accepts canonical domain addresses for `path`, `src`, `dst`,
and patch headers. Its schema and every agent kind's system prompt explain
domain discovery, escaping, local namespace identity, and cross-domain copies.
Successful file results publish canonical `path` / `uri` fields; listing names
are display basenames, while `virtual_path` records the compatibility mount
projection. Grep/glob matches and tree roots also use domain URIs. File contents,
image blocks, byte offsets, and consistency tokens are never URI-rewritten.

`vfs` op=`mount_list` returns each mount's canonical URI, effective domain,
domain root, virtual projection and read-only flag, including permission
submounts. Backend/source paths are metadata, not addresses to pass to file
tools. Old unnamed remote mounts can only publish legacy `vfs://` references;
runtime manifest migration assigns domains before normal operation.

The agent's dynamic compute context publishes actual filesystem-domain roots
independently of compute target names and refreshes when that mapping changes,
even without a compute policy revision. `compute_status` includes canonical
mount addresses. Plugin/environment workspaces and browser download results
publish `local://` addresses as well. Legacy inputs remain readable, and native
host paths keep their separate execution semantics.

## Domain addresses

Declare a domain on its primary mount:

```toml
[[backend]]
id = "cluster-storage"
type = "ssh"
alias = "cluster-login"
root = "/shared"

[[mount]]
path = "/data/cluster"
backend = "cluster-storage"
source = "/"
read_only = true
domain = "ht"
```

`ht://share/input.tsv` resolves to `/data/cluster/share/input.tsv` in the
virtual namespace and `/shared/share/input.tsv` on that SSH backend. The same
address is supported by file operations, DataFusion, node inputs, and artifact
exports. Domain stores enforce the same principal, permissions, read-only
policy, and replacement locks as the composite VFS store.
Domain addresses are also accepted for Wiki/deck blob prefixes, deck asset
inputs, and mounted plugin workspace selectors. Blob keys remain relative to
their configured area.

Names must be lowercase URI schemes beginning with a letter, followed by
letters, digits, `+`, `.`, or `-`. `vfs`, `file`, `http`, and `https` are reserved; remote compute
domains also cannot claim `local`. Each primary domain is unique. A permission
submount may inherit its parent's domain only when it preserves the backend
and object key. Named retained-output mounts must preserve the primary
projection of their remote retained directory.

Canonical paths percent-encode non-unreserved UTF-8 bytes, including literal
percent signs. For example the object key `space name/doi%3Ax.txt` becomes
`local://space%20name/doi%253Ax.txt`. Domain paths are decoded once before
traversal and mount checks. Unknown domains, root escapes, encoded separators,
and unencoded query/fragment syntax are refused. Legacy `vfs://` paths retain
their historical spelling semantics.

## Compatibility and migration

At runtime startup, old manifests gain explicit domains without moving files.
Existing domain declarations remain unchanged. The local root uses `local`;
unnamed remote partitions use their unique backend name when it is a valid
scheme, otherwise a deterministic mount-derived name. Existing named-domain
permission submounts retain their inherited identity. The first original
manifest is saved as `vfs.toml.pre-domain.bak`; migration preserves operator
extension fields and replaces the manifest atomically. Repeated loading does
not reassign names or overwrite that backup.

`FileRef` readers infer placement from a domain URI and reject a conflicting
explicit `domain` field. Old records without a domain retain compatibility;
mounted legacy references restored from the file cache are upgraded using the
current mount table. Node execution canonicalizes File, FileSet, and nested
channel artifact outputs before publication. Content hashes remain unchanged.
The VFS listing/stat API exposes `uri` alongside the legacy virtual `path`, and
mount metadata exposes `domain`.

With a mounted storage context, FileSet exact-path and glob selectors compare
resolved virtual paths, so existing `vfs://` selectors can match canonical
domain references while preserving the selected reference and its metadata.
Workflow caches hash and revalidate domain-addressed outputs through VFS;
they never treat a domain URI as a host filesystem path. Typed remote directory
outputs keep their coordinator domain address in the dispatch envelope and are
rebound to an isolated worker directory only for execution, then published back
through the coordinator's checked mount table.

## Live SSH acceptance

The isolated SSH acceptance test creates and removes its own remote temporary
directory. It verifies distinct local/remote bytes, transfers in both directions,
read-only refusal, canonical metadata, and DataFusion CSV reads plus Parquet
writes/readback with escaped names.
Run it against an available SSH alias (normal test runs leave it ignored):

```bash
AUTONOMICS_VFS_TEST_SSH_ALIAS=ht cargo test -p vfs --test domain_ssh -- --ignored
```

Set `AUTONOMICS_VFS_TEST_PYTHON` when the remote Python 3 executable has a
nondefault name. The test does not change the deployed VFS manifest or submit
compute jobs.

## Concurrent writes

Object writes made through the DataFusion Object Store adapter are staged to a
unique sibling object and renamed into place only after the complete payload is
written. The adapter also coordinates readers and replacement writers per final
path. A reader therefore sees either the complete previous object or the
complete replacement, never the intermediate multipart write. Concurrent
writers to one path serialize at the final replacement; the last successful
complete writer wins.

This guarantee covers DataFusion reads and writes routed through the VFS Object
Store adapter. Direct OpenDAL operations that bypass that adapter do not
acquire its reader/writer coordination.

## Mount manifest

The runtime loads `state_dir/vfs.toml`. On first launch it creates that file with a root local mount plus any environment-configured `/data/s3` and `/data/oss` mounts, then loads it.

```toml
[[backend]]
id = "default"
type = "local"
root = "/"

[[backend]]
id = "oss-prod"
type = "oss"
bucket = "autonomics-data"
endpoint = "https://oss-cn-hangzhou.aliyuncs.com"
access_key_id = "..."
secret_access_key = "..."

[[mount]]
path = "/"
backend = "default"
source = "/mnt/disk3/test"

[[mount]]
path = "/data/oss"
backend = "oss-prod"
source = "/"
read_only = true
```

`source` explicitly names the file, directory, or object prefix being mounted, analogous to a Linux bind mount. For local backends it can be an absolute host path; it must lie inside the backend root. For S3/OSS it is the prefix inside the configured bucket, with `/` meaning the bucket root.

Paths are routed by the longest matching mount and rewritten from `source` to the virtual `path`. For example:

```text
vfs:///data/oss/ld_score/1000g_eur/part.parquet
```

is read from:

```text
oss://autonomics-data/ld_score/1000g_eur/part.parquet
```

A local parquet can be exposed the same way by mounting its containing directory:

```toml
[[backend]]
id = "gwas-local"
type = "local"
root = "/"

[[mount]]
path = "/data/local/gwas"
backend = "gwas-local"
source = "/mnt/disk2/gwas"
read_only = true
```

DataFusion can then use:

```text
vfs:///data/local/gwas/foo.parquet
```

Credentials remain runtime configuration in `vfs.toml`.

## Unified catalog mounts

A `[catalog]` section in `vfs.toml` selects a Hugging Face registry repository.
The runtime resolves packages into the shared panel cache and adds these
read-only local mounts:

```text
/catalog                                  catalog index and version manifests
/datasets/<id>@sha256-<digest>            immutable version
/bundles/<id>                             current stable alias
```

Static mounts already present in `vfs.toml` win over generated aliases, making
the catalog overlay non-breaking. Full package build and publication commands
are documented in [Unified Data Catalog](data-catalog.md).

## Current LDSC mounts

The local converted panels are mounted under:

```toml
[[backend]]
id = "ldsc-local"
type = "local"
root = "/"

[[mount]]
path = "/data/ldsc"
backend = "ldsc-local"
source = "/mnt/projects/autonomics_projects/autonomics/reference/ldsc_data/parquet"
read_only = true

[[mount]]
path = "/data/ukbb"
backend = "ldsc-local"
source = "/mnt/disk3/ld_score"
read_only = true
```

The LDSC nodes use these VFS paths:

```text
vfs:///data/ldsc/1000g_eur.parquet
vfs:///data/ldsc/1000g_eur_m.parquet
vfs:///data/ldsc/baselineLD_v2_2_eur.parquet
vfs:///data/ldsc/baselineLD_v2_2_eur_m.parquet
vfs:///data/ukbb/UKBB.EUR.ldscore.parquet/
```

## Current MAGMA and MiXeR data mounts

Reference-data directories are mounted, while downloads, logs, and generated
results remain outside VFS:

```toml
[[backend]]
id = "magma-local"
type = "local"
root = "/"

[[mount]]
path = "/data/magma/inputs"
backend = "magma-local"
source = "/mnt/data/magma/data"
read_only = true

[[mount]]
path = "/data/magma/genes"
backend = "magma-local"
source = "/mnt/data/magma/resources/genes"
read_only = true

[[mount]]
path = "/data/magma/references"
backend = "magma-local"
source = "/mnt/data/magma/resources/references"
read_only = true

[[mount]]
path = "/data/mixer/resources"
backend = "magma-local"
source = "/data/mixer/resources"
read_only = true
```

MAGMA tooling addresses its inputs through that virtual namespace. The reference
root contains prebuilt 1000 Genomes bundles for AFR, AMR, EAS, EUR, and SAS:

```text
vfs:///data/magma/references/g1000_<population>/bundle.json
vfs:///data/magma/references/g1000_<population>/g1000_<population>
```

The second path is a PLINK prefix; `.bed`, `.bim`, and `.fam` are resolved and
read through the same VFS mount. `<population>` accepts `AFR`, `AMR`, `EAS`,
`EUR`, and `SAS`.

Each reference panel is a versioned panel bundle. A bundle
directory contains `bundle.json`, the PLINK prefix, and a precomputed gene
annotation generated from that exact panel:

```text
vfs:///data/magma/references/g1000_eas/bundle.json
vfs:///data/magma/references/g1000_eas/annotations/g1000_eas.NCBI37.3.window35.genes.annot
```

New bundles can be prepared with:

```bash
scripts/build_magma_panel_bundle.sh \
  /mnt/data/magma/resources/references/g1000_eas \
  /mnt/data/magma/resources/genes/NCBI37.3.gene.loc
```

The converted gene-location tables are also available under the MAGMA gene
mount:

```text
vfs:///data/magma/genes/parquet/NCBI37.3.gene_loc.parquet
vfs:///data/magma/genes/parquet/NCBI38.gene_loc.parquet
```

The legacy `/data/mixer/resources` layout remains available for host-side
diagnostics. Production MiXeR and SuSiE nodes no longer execute from this
mount:

```text
vfs:///data/mixer/resources/g1000_eur/bundle.json
vfs:///data/mixer/resources/g1000_eur/engine/precimed/mixer.py
vfs:///data/mixer/resources/g1000_eur/ld_mixer/1000G.EUR.chr@
```
The production `wjixiang/catalog-mixer-g1000-eur` catalog package carries the EUR/GRCh37 BIM,
signed-LD, and tag-SNP payloads. The rsID-addressed MiXeR runtime uses the
derived `wjixiang/catalog-mixer-g1000-eur-rsid` package, while SuSiE-RSS continues to use the
coordinate-addressed signed-LD package. The MiXeR and SuSiE-RSS OCI images
carry their official engines and pinned runtimes; DAG specs never reference
raw panel paths.

`susie_rss` reuses the cataloged `wjixiang/catalog-mixer-g1000-eur` package for signed
Pearson-r LD lookup. The wrapper owns the panel binding and does not expose raw
engine or panel paths in DAG specs.

## KEGG data mount

The local KEGG Parquet export is mounted read-only:

```toml
[[backend]]
id = "kegg-local"
type = "local"
root = "/"

[[mount]]
path = "/data/kegg_data"
backend = "kegg-local"
source = "/mnt/disk3/kegg_scraper/data/kegg_data"
read_only = true
```

`magma_kegg_align` reads the NCBI gene-location Parquet plus these KEGG tables:

```text
vfs:///data/kegg_data/entity_gene.parquet
vfs:///data/kegg_data/link_pathway_ko.parquet
vfs:///data/kegg_data/entity_pathway.parquet
vfs:///data/kegg_data/link_genome_pathway.parquet
```

## GO data mount

The GO graph, GAF/GPI tables, and NCBI human `gene2go` mapping are mounted
read-only under `/data/go_data`:

```toml
[[backend]]
id = "go-local"
type = "local"
root = "/"

[[mount]]
path = "/data/go_data"
backend = "go-local"
source = "/mnt/data/go_data"
read_only = true
```

Important tables and MAGMA-ready gene-set files include:

```text
vfs:///data/go_data/gene2go_human.parquet
vfs:///data/go_data/ontology_nodes.parquet
vfs:///data/go_data/ontology_edges.parquet
vfs:///data/go_data/magma_go_sets.NCBI37.3.propagated.10_1000.txt
vfs:///data/go_data/magma_go_sets.NCBI38.propagated.10_1000.txt
```

The `gene2go_human.parquet` table contains 446,675 human NCBI GeneID-to-GO rows.
The MAGMA-ready files remove `NOT` qualifiers, propagate annotations through GO
`is_a` and `part_of` edges, intersect with each MAGMA gene universe, and retain
sets containing 10-1000 genes.

## Central resource downloader

Resource downloads are configured separately in `state_dir/resource-downloader.toml`. Missing manifests resolve to a deny-by-default policy. HTTP(S) allow rules can be added under `[[policy.allow]]`; S3/OSS profiles can be declared under `[[object_store_profiles]]`. Downloaded files are installed atomically in the agent VFS, by default under `/downloads`.

## Browsing the VFS (humans)

The daemon exposes the whole namespace over HTTP at `/api/v1/vfs`, and both
frontends build their file browsers on it:

- **GUI (workbench)** — the `VFS 文件` rail button opens the sidebar container:
  a lazy directory tree with a lock badge on read-only mounts. Text files open
  as `vfs-file` editor tabs (editable with Ctrl+S save and a stale-copy
  conflict check), images preview inline, everything else downloads. The
  toolbar creates folders/files and uploads into the active directory; row
  actions cover rename and delete.
- **TUI** — the command palette's `VFS browser` opens a two-pane overlay
  (directory list + text preview). `Enter` descends/previews, `h`/`Backspace`
  ascends, `m`/`n`/`R`/`d` run mkdir/new-file/rename/delete with inline input
  and y/N confirmation; read-only mounts block creation shortcuts.

### HTTP endpoints

All endpoints enforce the mount table server-side: read-only mounts reject
every mutation regardless of what the client allows. Errors answer
`{"error": message}` with the status suggested by the error class
(404/403/409/400/500).

| Endpoint | Purpose |
| --- | --- |
| `GET /vfs/list?path=&recursive=&limit=&offset=` | Paginated, mount-aware directory listing (synthetic mount-point entries first) |
| `GET /vfs/stat?path=` | Entry metadata plus the covering mount's `read_only` flag |
| `GET /vfs/mounts` | The declarative mount table |
| `GET /vfs/file?path=&download=` | Chunked byte stream; single-part `Range` supported (206/416) |
| `GET /vfs/text?path=&max_bytes=` | Bounded UTF-8 read for editors (default 1 MiB, cap 10 MiB) |
| `PUT /vfs/file?path=` | Raw byte upload (staging+rename atomic); group body limit 50 MiB |
| `PUT /vfs/text` | `{path, contents, original}` editor save; stale `original` → 409 |
| `POST /vfs/mkdir` | `{path}` |
| `DELETE /vfs/entry?path=&recursive=` | Refuses `/`; non-empty dirs need `recursive` (else 409) |
| `POST /vfs/move` | `{src, dst}` — writable on both ends |
| `POST /vfs/copy` | `{src, dst}` — readable source, writable destination |

The listing, mutation and safety semantics live in `vfs::browse`
(`crates/vfs/src/browse.rs`), shared with the agent-facing `vfs` tool so the
two surfaces cannot drift: what an agent sees through `vfs ls` is exactly what
the browsers show.
