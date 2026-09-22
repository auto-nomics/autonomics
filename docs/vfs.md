# Virtual File System

Autonomics exposes storage resources through one Unix-style namespace. The runtime mounts local directories, S3-compatible buckets, and GitHub Container Registry OSS buckets under `vfs://`; DataFusion then reads them with ordinary paths.

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

MAGMA nodes address their inputs through that virtual namespace. The reference
root contains prebuilt 1000 Genomes bundles for AFR, AMR, EAS, EUR, and SAS:

```text
vfs:///data/magma/references/g1000_<population>/bundle.json
vfs:///data/magma/references/g1000_<population>/g1000_<population>
```

The second path is a PLINK prefix; `.bed`, `.bim`, and `.fam` are resolved and
read through the same VFS mount. `<population>` accepts `AFR`, `AMR`, `EAS`,
`EUR`, and `SAS`.

`magma_gene` consumes reference panels as versioned panel bundles. A bundle
directory contains `bundle.json`, the PLINK prefix, and a precomputed gene
annotation generated from that exact panel:

```text
vfs:///data/magma/references/g1000_eas/bundle.json
vfs:///data/magma/references/g1000_eas/annotations/g1000_eas.NCBI37.3.window35.genes.annot
```

The node derives `g1000_<population>` automatically (`EAS` remains the default),
or uses an explicit `reference` ID for a custom panel bundle. It checks
genome-build/population metadata, verifies the required annotation checksum,
and reads both the PLINK files and annotation from that same bundle. For
example, `{"population":"EUR"}` selects the deployed `g1000_eur` bundle.
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

`susie_rss_container` reuses the cataloged `wjixiang/catalog-mixer-g1000-eur` package for signed
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
