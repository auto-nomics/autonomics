# DESeq2 differential-expression node

This directory contains the complete Stage 0-3 implementation for a reusable,
Podman-backed differential-expression node. The statistical engine is the
official R `DESeq2` package; this repository does not port the
negative-binomial model to Rust.

The first version targets bulk RNA-seq raw count matrices. Normalized
TPM/FPKM/CPM tables, transcript-level aggregation, and single-cell-specific
models are intentionally out of scope until separate contracts and baselines
are defined.

## Layout

```text
containers/deseq2/
  Dockerfile
  deseq2_runner.R
  test_deseq2.sh
  test_deseq2_data_boundary.sh
  fixtures/
```

## Node Contract

Node kind:

```text
deseq2_de_container
```

Input ports:

1. Count matrix File: tab-separated, first column `gene_id`, followed by one
   raw integer-count column per sample. Gene IDs must be unique and counts
   must be nonnegative integers.
2. Sample metadata File: tab-separated with columns `sample_id`, `condition`,
   and optional covariates such as `type`. `sample_id` must be unique. The
   exact set must match the count-matrix sample columns; row order is used as
   the authoritative sample order after that set check.

Initial parameters:

| Parameter | Default | Description |
| --- | --- | --- |
| `condition_reference` | required | Reference factor level, such as `untreated`. |
| `condition_test` | required | Test factor level, such as `treated`. |
| `covariates` | `[]` | Metadata columns added before `condition`; the node constructs the design itself. |
| `fit_type` | `parametric` | Passed to `DESeq2::DESeq()` as `parametric`, `local`, or `mean`. |
| `alpha` | `0.1` | Optimization and independent-filtering target FDR. |
| `threads` | `1` | Serialized `BiocParallel::SerialParam` initially, avoiding hidden nondeterminism. |
| `timeout_secs` | `3600` | Container wall-clock timeout. |
| `artifact_prefix` | `/artifacts/deseq2_de_container` | VFS output prefix. |

Example JSON specification for the checked-in fixture:

```json
{
  "condition_reference": "untreated",
  "condition_test": "treated",
  "covariates": ["type"],
  "fit_type": "parametric",
  "alpha": 0.1,
  "threads": 1
}
```

Arbitrary R formulas will not be accepted in the first version. The wrapper
constructs `~ <covariates> + condition` from validated identifiers and levels.
This keeps JSON parameters easy to validate and avoids embedding R code from a
DAG specification. Batch, paired, and multi-factor designs can be exposed later
as explicit fields after their baselines are defined.

Primary contrast:

```text
condition_test vs condition_reference
```

All metadata used by the design must contain complete values and at least one
observation per estimated coefficient. Categorical covariates become factors;
numeric covariates remain numeric. A design that is rank deficient is rejected
with a container diagnostic rather than silently dropped.

## Thin Wrapper

The registered factory is
`crates/node-bundles/nodes-io/src/deseq2_container.rs`. It delegates execution
to `ContainerCommandNode`, validates parameter names and ranges, constructs the
environment contract for the image runner, and declares the five outputs. It
contains no DESeq2 numerical logic and no inline R model code.

The wrapper always requests an isolated network, a read-only root filesystem,
2 CPUs, a 4 GiB memory limit, a 256-process limit, and no reference panels. The
registry test verifies that the node is discoverable through the IO plugin, and
the fake-runtime integration test verifies both staged inputs and all five VFS
artifacts.

## Outputs

The `nodes-io` wrapper declares five File artifacts:

1. `results.tsv`: `gene_id`, `baseMean`, `log2FoldChange`, `lfcSE`, `stat`,
   `pvalue`, and `padj` from `DESeq2::results()`.
2. `normalized_counts.tsv`: variance-stabilization-independent normalized
   counts from `counts(dds, normalized = TRUE)`.
3. `size_factors.tsv`: one estimated size factor per sample.
4. `deseq2_dataset.rds`: the complete `DESeqDataSet` after fitting.
5. `run_report.json`: tool versions, input dimensions, design variables,
   factor levels, contrast, result counts, and input/output checksums.

A combined stdout/stderr log should also be retained by the container runtime.
Result filtering is a downstream concern; the node returns the complete model
result table so thresholds can change without rerunning DESeq2.

## Container

The Stage 1 implementation uses:

- Base: `docker.io/rocker/r-ver:4.5.3`
  (`sha256:35394dcbf419ac29056848522006de3cd33c33191377abed182acaecd48eba37`)
- Bioconductor release: 3.22
- Official `DESeq2` 1.50.2 source:
  `https://bioconductor.org/packages/3.22/bioc/src/contrib/DESeq2_1.50.2.tar.gz`
- DESeq2 source SHA-256:
  `514f23ae8d274623d80978c30bfa1c6566acd98188bf1aa563970959ea59522f`
- Upstream source commit: `d90821a`
- License: LGPL (>= 3)

The image contains only R, DESeq2, and its runtime dependencies. Count
matrices, metadata, annotations, and credentials stay outside the image. No
reference panel or genome build is required for this analysis.

The runner reads input and output locations from the runtime environment
variables `AUTONOMICS_INPUT0`, `AUTONOMICS_INPUT1`, and `AUTONOMICS_OUTPUT0`
Direct image invocation defaults to UID/GID 1000; the generic DAG runtime maps
the non-root control-process UID/GID and runs with isolated networking and a
read-only root filesystem.

Published immutable image:

```text
$AUTONOMICS_IMAGE_PREFIX/deseq2@sha256:8b2e2a78d87293e6cae6dbed2e283dff1dd8461a7f993cb347ca9810698b2b3e
```

The tag `$AUTONOMICS_IMAGE_PREFIX/deseq2:1.50.2` resolves to this digest. The
thin wrapper pins the digest and resolves the registry host from `GHCR_ENDPOINT`,
matching the other official-tool container nodes.

## Data-Package Decision

Stage 2 deliberately binds no catalog data package and declares no panel mount.
Both DESeq2 inputs are analysis-owned user files, and the model requires neither
a genome build nor an annotation database. The pasilla fixture is repository
test data, not an image or catalog dependency.

The boundary is checked by:

```sh
containers/deseq2/test_deseq2_data_boundary.sh
```

The wrapper asserts an empty `panel_bundles` list and an empty `panels` list in
its generated `ContainerCommandSpec`.

## Test Data

`fixtures/pasilla_gene_counts.tsv` and
`fixtures/pasilla_sample_metadata.tsv` provide the official DESeq2/pasilla
baseline: 14,599 Drosophila genes, 7 samples, a `treated` versus `untreated`
condition, and a single-read/paired-end library-type covariate. The default
fixture model should therefore be:

```text
~ type + condition
condition_test = treated
condition_reference = untreated
```

Validate the local data contract without R or Podman:

```sh
containers/deseq2/test_deseq2_fixture.sh
```

Build and run the complete official-package baseline:

```sh
containers/deseq2/test_deseq2.sh
```

The full test builds the image, runs the pasilla model twice, requires every
output checksum to match `fixtures/pasilla_baseline.json`, independently
recomputes DESeq2 median-of-ratios size factors and normalized counts, checks
`baseMean` against normalized counts, validates p-value domains, verifies the
RDS is a `DESeqDataSet`, and rejects mismatched sample IDs, non-integer counts,
and rank-deficient designs.

Run the Rust contract, registry, fake-runtime, and real Podman tests:

```sh
cargo test -p nodes-io deseq2_container
cargo test -p nodes-io stages_two_inputs_and_publishes_five_outputs
cargo test -p nodes-io real_official_deseq2_runs_in_podman_and_matches_baseline -- --ignored
```

For a localhost-image real Podman test, first materialize the published digest
under the equivalently named local repository:

```sh
podman pull "$AUTONOMICS_IMAGE_PREFIX/deseq2@sha256:8b2e2a78d87293e6cae6dbed2e283dff1dd8461a7f993cb347ca9810698b2b3e"
podman tag "$AUTONOMICS_IMAGE_PREFIX/deseq2@sha256:8b2e2a78d87293e6cae6dbed2e283dff1dd8461a7f993cb347ca9810698b2b3e" \
  localhost/autonomics/deseq2:1.50.2
AUTONOMICS_DESEQ2_IMAGE_ENDPOINT=localhost \
  cargo test -p nodes-io real_official_deseq2_runs_in_podman_and_matches_baseline -- --ignored
```

## Acceptance Criteria

1. [x] The fixture script passes.
2. [x] The pinned image builds and reports `DESeq2 1.50.2`.
3. [x] The pasilla command completes twice with identical result checksums.
4. [x] The result table retains all 14,599 genes and the expected DESeq2 columns.
5. [x] Unit tests verify the generated `ContainerCommandSpec`, input staging, and
   output declarations.
6. [x] A registry test proves the thin wrapper builds with no panel bundles.
7. [x] A real Podman end-to-end test publishes all five outputs through VFS.
8. [x] The recorded numerical baseline matches the official-image repeated run
   before the wrapper is registered as a reusable node kind.
