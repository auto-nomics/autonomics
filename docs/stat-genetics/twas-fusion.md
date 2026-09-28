# TWAS / FUSION

The `twas_fusion` node (`twas` plugin) performs transcriptome-wide
association testing with the official FUSION `FUSION.assoc_test.R` script.
The image pins the upstream `fusion_twas` commit
`9346c1222bffbb34499fa7a8e23c1b701b55cb05`; no numerical logic is
reimplemented.

## Runtime Data

The node mounts the immutable `wjixiang/catalog-fusion-gtex-v8` catalog package. The current
package is `v1.0.0` with digest
`sha256:22ad786d571ca9cb24279a7ca433d7e2c1c9c978d1b5e0d5385a4e15bb22ef93`.
It contains:

- GTEx v8 FUSION weight archives for 49 tissues under `weights/`;
- the 1000G EUR FUSION LDREF for chromosomes 1-22 under `LDREF/`.

The first run downloads and checksum-verifies the package into the shared
PanelCache. Later runs reuse the immutable cache entry.

## Input

Input is one whitespace-delimited File, normally produced by `dataframe_to_file` with
`format: "tsv"`. Required columns are:

| Column | Meaning |
| --- | --- |
| `SNP` | Variant ID matching the FUSION LDREF BIM |
| `A1` | First allele for orientation and QC |
| `A2` | Second allele for orientation and QC |
| `Z` | GWAS z-score |

The node performs one chromosome per invocation. Set `chr` to a value from 1
through 22 and `tissue` to the exact GTEx package tissue name, such as
`Whole_Blood`. Tissue names use underscores rather than spaces.

The deployed LDREF uses GRCh37 rsIDs such as `rs7281456`. The wrapper passes
alleles through FUSION's official QC and flips z-scores when the input allele
orientation differs from LDREF.

## Parameters

| Parameter | Default | Meaning |
| --- | ---: | --- |
| `tissue` | required | GTEx tissue name |
| `chr` | required | Autosome 1-22 |
| `force_model` | `null` | Pin `blup`, `lasso`, `top1`, or `enet`; default selects the best cross-validation model |
| `use_nofilter_weights` | `false` | Use `.nofilter.pos` instead of the filtered `.pos` |
| `max_impute` | `0.5` | Maximum missing LDREF SNP fraction per feature |
| `min_r2pred` | `0.7` | Minimum mean LD imputation accuracy |
| `perm` | `0` | Maximum permutations; zero disables |
| `perm_minp` | `0.05` | P-value threshold for starting permutations |

Two further settings are node-level constants of the plugin manifest
rather than per-instance parameters: `artifact_prefix`
(`/artifacts/twas_fusion`, immutable output prefix) and `timeout_secs`
(`3600`, container timeout).

## Outputs

| Port | Artifact | Contents |
| --- | --- | --- |
| 0 | `twas_fusion.dat` | Official association table |
| 1 | `twas_fusion.log` | Official run log plus wrapper provenance |
| 2 | `twas_fusion.mhc.dat` | MHC results that FUSION separates from the main table; empty for chr21 |

The main table starts with `PANEL`, `FILE`, `ID`, `CHR`, `P0`, `P1`, `HSQ`,
`BEST.GWAS.ID`, `BEST.GWAS.Z`, `EQTL.ID`, `EQTL.R2`, `EQTL.Z`,
`EQTL.GWAS.Z`, `NSNP`, `NWGT`, `MODEL`, `MODELCV.R2`, `MODELCV.PV`, `TWAS.Z`,
and `TWAS.P`. `FILE` refers to the temporary extraction path, while `ID`,
chromosome, and coordinates provide stable feature identity.

## Example

```json
{
  "tissue": "Whole_Blood",
  "chr": 21,
  "force_model": "top1"
}
```

Run `test_fusion_twas.sh` from the `twas` plugin checkout
(`/mnt/projects/node-plugins/twas`) to rebuild the image, verify the
catalog panel and registry digest, and execute the real container
regression.
