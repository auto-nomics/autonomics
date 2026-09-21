# TwoSampleMR container

Official R `TwoSampleMR` package (MRC IEU, 0.7.9, commit `3d119f2`) packaged
for the Podman-backed DAG runtime. The image also carries the official PLINK2
2.0.0-a.6.26 binary used for deterministic offline instrument clumping.

The image is published as
`$AUTONOMICS_IMAGE_PREFIX/twosamplemr:0.7.9` with immutable manifest digest
`sha256:c270de9978906ee48cbba2ac484ba3df9e86dddc69efd908c6f908963114002a`.
The production wrapper pins this digest and resolves the registry namespace
from `AUTONOMICS_IMAGE_PREFIX`.

## Provenance

- R runtime: `rocker/r-ver:4.5.1`
- CRAN snapshot: 2026-09-13 (Ubuntu Noble binary repository)
- TwoSampleMR source archive SHA-256:
  `6848c344c5eead601ff52e9a88b2c23c2b61b0ce4f848e331e7494b657be64e5`
- MRMix commit: `56afdb2`
- RadialMR commit: `a30ff11`
- MRPRESSO commit: `3e3c92d`
- PLINK2 asset SHA-256:
  `f578a450af382d7dd6665aecf0ca1d280971c2b3d5bb5556efbf9266c4c8da0f`

## Build and Test

```sh
./containers/twosamplemr/test_twosamplemr.sh
```

The baseline loads the official `test_commondata.RData` fixture shipped with
TwoSampleMR and verifies the upstream `mr()` golden estimates for IVW
(`0.4459`) and MR Egger (`0.5025`).

## Thin Wrapper

`crates/node-bundles/nodes-io/src/twosamplemr_container.rs` accepts one
SNP-merged tab-separated File containing:

- `snp`
- `beta_exposure`, `se_exposure`
- `effect_allele_exposure`, `other_allele_exposure`, `eaf_exposure`
- `beta_outcome`, `se_outcome`
- `effect_allele_outcome`, `other_allele_outcome`, `eaf_outcome`
- optional `pval_exposure`

It binds `plink.ref.1000g_eur.binary`, clumps instruments with official
PLINK2, then executes `TwoSampleMR::harmonise_data()` and
`TwoSampleMR::mr()`. The node publishes the MR result table, harmonised table,
complete RDS result, and combined PLINK2/R log. The committed chr22 fixture
verifies the complete catalog-backed DAG path and reduces ten candidate
variants to two index instruments.
