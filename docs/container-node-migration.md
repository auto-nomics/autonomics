# Container Node Migration Workflow

This is the canonical workflow for migrating an analysis node to the container
execution path:

```text
tool image
  -> cataloged data package
  -> thin DAG node wrapper
```

The workflow keeps three concerns separated:

| Concern | Owner | Artifact |
|---|---|---|
| Tool execution environment | OCI image | `containers/<tool>/Dockerfile` |
| Reference data | data catalog | immutable catalog package |
| Analysis contract and compatibility | DAG wrapper | `nodes-io` node factory |

Agents select an analysis and provide its inputs. They do not select tool
images, assemble panel mounts, or guess which panels are compatible.

## Non-negotiable policy

Container nodes must execute the official original implementation: an upstream
CLI, author-released package, or author-published OCI image. Do not package a
Rust port as the containerized analysis engine. Existing native Rust nodes stay
registered only as transitional fallbacks until their official replacement has
a reproducible end-to-end baseline.

## Scope

Use this workflow when:

- the tool is an external command-line implementation;
- it needs ordinary POSIX file access;
- it depends on shared reference panels;
- reproducibility depends on exact tool and data versions.

Do not use it for pure Rust in-process transforms that do not need external
artifacts or panels.

## Stage 0: Intake

Before building anything, record:

1. Tool name and version.
2. Source tree or upstream release.
3. Exact input contract.
4. Exact output contract.
5. Reference-data requirements.
6. A known-good command line.
7. A numerical or file-level baseline.
8. License and distribution constraints.

The intake note should state which combinations are known to be compatible,
for example:

```text
tool: original LDSC 3.0.1
input: tab-separated SNP/A1/A2/N/Z sumstats; gzip optional
panels: 1000G EUR LD Score + no-MHC weights
output: raw LDSC log
```

## Stage 1: Build the tool image

Create:

```text
containers/<tool>/Dockerfile
containers/<tool>/test_<tool>_<analysis>.sh
```

The image contains only:

- the tool implementation;
- its language runtime;
- pinned dependencies;
- minimal system utilities required by the tool.

It must not contain reference panels, GWAS inputs, user data, credentials, or
cluster configuration.

### Image rules

- Prefer a minimal, pinned base image.
- Pin dependency versions.
- Keep the image small enough for local import into k3s.
- Run as a non-root user where practical.
- Make the primary entrypoint deterministic.
- Include only files required at runtime.
- Prefer a digest-pinned image reference once the workflow leaves local
  development. Official tool wrappers pin their digest and resolve the ACR
  registry host from `ACR_ENDPOINT`.

### Build and validate

```bash
podman build \
  -f containers/<tool>/Dockerfile \
  -t localhost/atc/<tool>:<version> \
  <tool-source-context>

podman run --rm localhost/atc/<tool>:<version> --help
```

Then import the image into k3s:

```bash
podman save \
  -o /tmp/atc-<tool>-<version>.tar \
  localhost/atc/<tool>:<version>

sudo k3s ctr images import /tmp/atc-<tool>-<version>.tar
```

Acceptance for this stage:

- image builds reproducibly;
- tool help or equivalent smoke command succeeds;
- image does not embed data panels;
- k3s can run the image;
- a small local command produces the expected baseline.

## Stage 2: Build and publish data packages

Create a staging directory that contains only the payload needed by one
logical data package:

```text
/mnt/data/<tool>_panel_staging/<panel-name>/
  <payload files>
```

Build the normalized package:

```bash
cargo run -p data-catalog --bin autonomics-catalog -- build \
  /mnt/data/<tool>_panel_staging/<panel-name> \
  /mnt/data/<tool>_catalog_packages/<panel-name>-v<version> \
  --id <tool>.<panel-name> \
  --version v<version> \
  --kind <tool>_<panel-role> \
  --metadata population=<POP> \
  --metadata genome_build=<BUILD> \
  --metadata description="<short human description>"
```

Validate:

```bash
cargo run -p data-catalog --bin autonomics-catalog -- validate \
  /mnt/data/<tool>_catalog_packages/<panel-name>-v<version>
```

Publish:

```bash
cargo run -p data-catalog --bin autonomics-catalog -- publish \
  /mnt/data/<tool>_catalog_packages/<panel-name>-v<version> \
  --config ~/.autonomics/vfs.toml
```

Record the resulting catalog IDs. They become the wrapper's declared bundle
bindings.

### Package rules

- One package describes one logical resource.
- Do not combine unrelated panel roles.
- Keep package IDs stable across versions.
- Put population, genome build, and tool role in metadata.
- Use the immutable digest when a workflow must pin an exact version.
- After publishing a new entry, restart the runtime before binding it to a DAG
  wrapper; `catalog_refresh` updates discovery metadata only, not VFS mounts or
  the DataBundle registry.

Acceptance for this stage:

- package validates;
- payload checksums and sizes are recorded;
- package is published to the intended catalog;
- `catalog list` or `catalog_search` can find it;
- the package has no build tooling or transient files.

## Stage 3: Wrap the node

Create a thin node factory under `nodes-io`:

```text
crates/node-bundles/nodes-io/src/<tool>_<analysis>_container.rs
```

The wrapper owns:

- fixed tool-image tag;
- required catalog panel IDs;
- panel mount paths;
- command template;
- input and output file contract;
- tool-specific parameter validation;
- timeout and resource defaults;
- output artifact prefix.

It must not:

- create a second Kubernetes client;
- duplicate the container backend;
- ask the Agent to choose compatible panels;
- embed panel object keys;
- hide required tool inputs behind implicit global state.

The wrapper delegates execution to `ContainerCommandNode`. It only constructs
the `ContainerCommandSpec` and resolves its bound `DataBundle` records.

### Port contract

Prefer File-to-File nodes:

```text
File input
  -> wrapper
  -> File artifact
```

If the input is an existing file, use `file_reference` before the wrapper.
If the input is a DataFrame, use `dataframe_to_file` before the wrapper.

### Wrapper acceptance

A migration is not complete until:

1. unit tests validate the generated `ContainerCommandSpec`;
2. registry tests prove the node builds with its required catalog bundles;
3. a real k3s test runs the tool image;
4. PanelCache verifies and mounts the catalog package;
5. input staging reaches the container;
6. declared outputs are uploaded to VFS;
7. a numerical or file-level baseline matches;
8. the old native node is either still registered or explicitly unregistered,
   with the migration recorded in docs.

## Migration checklist

- [ ] Record tool, version, license, source, and baseline command.
- [ ] Build a minimal OCI image without reference data.
- [ ] Run image smoke tests locally and inside k3s.
- [ ] Stage one logical data package per panel role.
- [ ] Validate and publish the catalog package.
- [ ] Record the package ID, version, and digest.
- [ ] Implement a thin `nodes-io` wrapper.
- [ ] Bind the wrapper to the exact compatible image and panel IDs.
- [ ] Register the wrapper in the IO plugin.
- [ ] Add unit tests for the generated container spec.
- [ ] Add a registry build test.
- [ ] Add a real k3s end-to-end test.
- [ ] Compare the result against the recorded baseline.
- [ ] Document the migration and remove the old node registration only when
      the replacement is ready.

## Ownership boundaries

| Layer | Owns | Does not own |
|---|---|---|
| OCI image | tool binary, runtime, dependencies | panels, user data |
| data catalog | immutable payloads and metadata | execution semantics |
| PanelCache | verification and materialization | tool logic |
| `ContainerCommandNode` | k3s execution, staging, mounts, outputs | analysis contract |
| specialized wrapper | analysis contract and compatibility | second runtime |
| Agent | analysis selection and inputs | mount plumbing |

## Reference implementation

The LDSC migration is the reference case:

```text
containers/ldsc/Dockerfile
ldsc.ref_ld.1000g_eur.basic
ldsc.w_ld.1000g_eur_hm3_no_mhc
crates/node-bundles/nodes-io/src/ldsc_h2_container.rs
crates/node-bundles/nodes-io/src/ldsc_munge_container.rs
crates/node-bundles/nodes-io/src/ldsc_rg_container.rs
```

Its flow is:

```text
file_reference
  -> ldsc_h2_container
  -> ContainerCommandNode
  -> k3s Job
  -> VFS ldsc_h2.log
```

The wrapper fixes the image and panel binding. Agents provide a standard
tab-separated LDSC sumstats File and do not mount panels themselves.

MAGMA annotation is the second reference case:

```text
containers/magma/Dockerfile
magma.gene_loc.ncbi37_3
crates/node-bundles/nodes-io/src/magma_annotate_container.rs
```

It runs the official v1.10 static executable and emits both the raw MAGMA log
and the `.genes.annot` artifact.

MR-PRESSO is the no-panel R reference case:

```text
containers/mrpresso/Dockerfile
crates/node-bundles/nodes-io/src/mrpresso_container.rs
```

The image installs the official package from its pinned upstream commit. The
wrapper calls `MRPRESSO::mr_presso` directly and emits the official result
object as an RDS artifact plus its printed log.

MVMR is the multi-input official R reference case:

```text
containers/mvmr/Dockerfile
crates/node-bundles/nodes-io/src/mvmr_container.rs
```

The wrapper stages one tab-separated instrument table, builds the official
`format_mvmr` input, calls the selected upstream functions, and emits the
native result object and printed log.

MiXeR is the source-backed Python/C++ reference case:

```text
containers/mixer/Dockerfile
mixer.g1000_eur_rsid
crates/node-bundles/nodes-io/src/mixer_container.rs
```

The image compiles official `libbgmg.so` from the clean v2.2.1 source checkout
and invokes `precimed/mixer.py fit1/fit2`. The large GRCh37 EUR BIM/LD/tag-SNP
package stays in the catalog; the image contains only tool code and pinned
runtimes.

PLINK2 is the offline LD-clumping reference case:

```text
containers/plink2/Dockerfile
plink.ref.1000g_eur.binary
containers/plink2/fixtures/chr22.sumstats.tsv
crates/node-bundles/nodes-io/src/plink2_clump_container.rs
```

The image carries only the official v2.0.0-a.6.26 binary. The 1000G EUR
Phase3 BED/BIM/FAM panel stays in the catalog, and the wrapper fixes the panel
binding plus PLINK2 column selectors. The k3s smoke test runs chr22 against
the committed fixture and verifies deterministic `.clumps` output in VFS.

SuSiE-RSS is the R + signed-LD panel reference case:

```text
containers/susie/Dockerfile
mixer.g1000_eur
containers/susie/fixtures/chr21.sumstats.tsv
crates/node-bundles/nodes-io/src/susie_rss_container.rs
```

The image installs official `susieR` 0.16.6 at commit `ef213fe` and compiles
the official gsa-mixer `libbgmg` helper needed to query signed LD pairs. The
1000G EUR signed-LD panel stays in the catalog. The wrapper takes one
`snp/chrom/z` sumstats File, mounts `mixer.g1000_eur`, calls
`susieR::susie_rss()`, and emits TSV/RDS/log artifacts to VFS.

The production wrapper pins the image repository and manifest digest, while
the runtime resolves the ACR endpoint from `ACR_ENDPOINT`. This keeps the exact
image immutable without embedding a deployment-specific registry host.

LAVA also has the official multiple-locus scan case:

```text
crates/node-bundles/nodes-io/src/lava_scan_container.rs
```

`lava_scan_container` follows the upstream batch workflow: it calls
`process.input()` once, reads the caller's loci table, iterates each selected
locus with `process.locus()`, and invokes the official `run.univ.bivar()`
workflow. `locus_ids` and `chr` subset the scan; process and analysis failures
are logged and skipped per locus. The node emits combined univariate and
bivariate TSVs, an RDS result object, and the complete log. It binds exactly
the same `lava.ref.ukb_eur` or tutorial panel contract as `lava_container`.
The local Podman baseline scans official tutorial loci `100` and `230`.

SMR/HEIDI is the BESD eQTL plus PLINK LD-panel reference case:

```text
containers/smr/Dockerfile
smr.eqtl.westra_hg19
smr.eqtl.eqtlgen_hg19
plink.ref.1000g_eur.binary
containers/smr/fixtures/chr22.westra.ma
crates/node-bundles/nodes-io/src/smr_heidi_container.rs
```

The image carries only the official SMR 1.4.2 executable and its bundled
runtime libraries. The default Westra BESD, the optional eQTLGen BESD selected
by `eqtl_source=eqtlgen`, and the 1000G EUR LD reference stay in catalog
packages. The wrapper accepts one GCTA-COJO `.ma` File, fixes the verified
image/data compatibility contract, and emits the official `.smr` result and
execution log. A chr22 Westra k3s baseline verifies both the output row and SMR
p-value; a chr22 eQTLGen baseline verifies the alternate panel binding.

HyPrColoc is the no-panel beta/SE matrix case:

```text
containers/hyprcoloc/Dockerfile
containers/hyprcoloc/fixtures/test-summary-stats.tsv
crates/node-bundles/nodes-io/src/hyprcoloc_container.rs
```

The image carries the official R package at `v0.0.2` and pinned dependencies.
The first contract is the documented independent-study analysis: one
SNP-aligned TSV with beta/SE columns for each trait. The wrapper emits the
official result table, full RDS object, and log. Optional LD,
trait-correlation, and sample-overlap matrices remain a separate contract
until their packaging and validation rules are defined.

GCTA is the official executable plus mixed-panel summary-statistics case:

```text
containers/gcta/Dockerfile
plink.ref.1000g_eur.binary
gcta.gene_list.hg19
containers/gcta/fixtures/chr22.ma
containers/gcta/fixtures/chr22.fastGWA
crates/node-bundles/nodes-io/src/gcta_container.rs
```

The image extracts the official Linux 1.95.3 AppImage at build time so runtime
jobs need neither FUSE nor privileges, then pins the published image by digest.
The 1000G EUR PLINK reference and official hg19 gene list stay in catalog
packages. Four File-to-File factories expose the verified summary-statistics
surface: COJO stepwise selection, SBLUP SNP-effect prediction, gene-based
fastBAT, and ACAT-V rare-variant aggregation. The committed official chr22
sample verifies all four output contracts in k3s. GREML, MLMA, fastGWA, GSMR,
and mtCOJO require cohort genotypes, phenotypes, GRMs, or additional LD-score
panels and remain separate migrations rather than unsafe bindings to the
1000G panel.

HDL-L is the official R block-LD SVD case:

```text
containers/hdl-l/Dockerfile
hdl.ref.ukb_eur
containers/hdl-l/test_hdl_l_podman.sh
scripts/build_hdl_ukb_panel.sh
crates/node-bundles/nodes-io/src/hdl_l_container.rs
crates/node-bundles/nodes-io/src/hdl_l_scan_container.rs
```

The image installs official `HDL` 1.4.3 at commit `e6b055d` and runs through
rootless Podman under the local `localhost/atc/hdl:1.4.3` tag. No registry or
k3s backend is required. The official Zenodo UKB EUR payload is normalized into
one catalog package with `LD/*_LDSVD.rda`, `LD/HDLL_LOC_snps.RData`, and
matching per-block BIM files. The wrapper takes two official-format GWAS
summary Files plus `chr` and `piece`, invokes `HDL::HDL.L`, and emits TSV, RDS,
and the official log. The registered `hdl_l_scan` runtime uses the same image
and panel binding, reads `NEWLOC` from `HDLL_LOC_snps.RData`, iterates official
`chr/piece` blocks, isolates per-block failures, and emits a combined TSV, an
RDS result list, and the official log. This panel is not interchangeable with
`lava.ref.ukb_eur`, `lava.ref.1000g_test`, `plink.1000g_eur`, or the native
LAVA PLINK contract. The full official package is published as `hdl.ref.ukb_eur`
v1.0 with digest `sha256:411c7ae1db876ec3e17941367a74567175ca151f8f93dc6e5e1d06bb8f3a3f54`;
the published panel has passed chr1/piece9 region and scan Podman baselines.
The native Rust HDL-L crate and both native nodes are removed.

MTAG is the official Python 2 plus single-prefix LD Score case:

```text
containers/mtag/Dockerfile
mtag.ld_ref.1000g_eur_w_ld
containers/mtag/test_mtag_containers.sh
crates/node-bundles/nodes-io/src/mtag_container.rs
```

The image carries the official MTAG 1.0.8 source at commit `9e17f3c`, its
pinned Python 2 dependencies, and no reference data. MTAG's embedded LDSC API
binds one directory prefix to both `ref_ld_chr` and `w_ld_chr`, so the upstream
1000G EUR `eur_w_ld_chr` LD Score/`M_5_50` payload is published as the
dedicated catalog package `mtag.ld_ref.1000g_eur_w_ld`. The split LDSC ref/w-LD
packages and the LAVA UKBB eigen `.bcor` panel are not interchangeable with
that contract. The rootless-Podman baseline verifies a deterministic two-trait
GWAS fixture, catalog panel materialization, both official result tables, and
numerical summary markers.
