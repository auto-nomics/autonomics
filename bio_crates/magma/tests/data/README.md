# MAGMA Test Data

Synthetic dataset generated to validate the Rust port of MAGMA v1.10 against the
original C++ implementation. All data is deterministic (fixed seeds).

## Input Data

| File | Description | Format |
|------|-------------|--------|
| `sim_geno.bed/.bim/.fam` | PLINK binary genotype (200 individuals × 500 SNPs, chr1, 100kb–5Mb) | PLINK 1 binary |
| `gene_loc.txt` | Gene locations (20 genes, Entrez-style IDs) | `gene_id  chr  start  end  strand` |
| `gwas_pval.txt` | GWAS summary statistics (500 SNPs, N=50000) | `SNP  P  N` (header) |
| `gene_sets.txt` | Gene-set definitions (7 sets) | `set_name  gene1  gene2 ...` (per row) |
| `gene_covar.txt` | Gene-level continuous covariates (3 variables × 20 genes) | `GENE  LENGTH  EXPRESSION  GC_CONTENT` |
| `covariates.txt` | Individual-level covariates (PC1–PC3, 200 individuals) | `FID  IID  PC1  PC2  PC3` |
| `phenotypes.txt` | Quantitative phenotype (200 individuals) | `FID  IID  QT1` |

## Golden Outputs (from original MAGMA v1.10)

### Annotation
| Output | Pipeline command |
|--------|-----------------|
| `annot.genes.annot` | `magma --annotate window=35 --snp-loc sim_geno.bim --gene-loc gene_loc.txt --out annot` |

### Gene Analysis (raw data — PC regression, binary phenotype from .fam)
| Output | Pipeline command |
|--------|-----------------|
| `gene_raw.genes.out` | `magma --bfile sim_geno --gene-annot annot.genes.annot --out gene_raw` |
| `gene_raw.genes.raw` | (same as above) |

### Gene Analysis (raw data — SNP-wise mean model, QT phenotype + covariates)
| Output | Pipeline command |
|--------|-----------------|
| `gene_raw_qt.genes.out` | `magma --bfile sim_geno --gene-annot annot.genes.annot --pheno file=phenotypes.txt --covar file=covariates.txt --gene-model snp-wise=mean --out gene_raw_qt` |
| `gene_raw_qt.genes.raw` | (same as above) |

### Gene Analysis (summary stats — SNP p-values + reference LD)
| Output | Pipeline command |
|--------|-----------------|
| `gene_pval.genes.out` | `magma --bfile sim_geno --pval gwas_pval.txt N=50000 snp-id=SNP pval=P --gene-annot annot.genes.annot --out gene_pval` |
| `gene_pval.genes.raw` | (same as above) |

### Gene Analysis (multi-model: PCreg + snpwise-mean + snpwise-top)
| Output | Pipeline command |
|--------|-----------------|
| `gene_multi.genes.out` | `magma --bfile sim_geno --gene-annot annot.genes.annot --gene-model multi=all --out gene_multi` |
| `gene_multi.genes.raw` | (same as above) |

### Gene-Set Analysis
| Output | Pipeline command |
|--------|-----------------|
| `gsa_pval.gsa.out` | `magma --gene-results gene_pval.genes.raw --set-annot gene_sets.txt --out gsa_pval` |
| `gsa_raw_qt.gsa.out` | `magma --gene-results gene_raw_qt.genes.raw --set-annot gene_sets.txt --out gsa_raw_qt` |

### Gene Property (Covariate) Analysis
| Output | Pipeline command |
|--------|-----------------|
| `gprop_pval.gsa.out` | `magma --gene-results gene_pval.genes.raw --gene-covar gene_covar.txt --out gprop_pval` |

## Regenerating

The original MAGMA binary is at `reference/magma` (built from `reference/`).
To regenerate all golden outputs:

```bash
MAGMA=../../../reference/magma

# Annotation
$MAGMA --annotate window=35 --snp-loc sim_geno.bim --gene-loc gene_loc.txt --out annot

# Raw data gene analysis (binary pheno, PCreg)
$MAGMA --bfile sim_geno --gene-annot annot.genes.annot --out gene_raw

# Raw data gene analysis (QT pheno + covariates, snpwise-mean)
$MAGMA --bfile sim_geno --gene-annot annot.genes.annot \
  --pheno file=phenotypes.txt --covar file=covariates.txt \
  --gene-model snp-wise=mean --out gene_raw_qt

# Summary-stats gene analysis (pval)
$MAGMA --bfile sim_geno --pval gwas_pval.txt N=50000 snp-id=SNP pval=P \
  --gene-annot annot.genes.annot --out gene_pval

# Multi-model
$MAGMA --bfile sim_geno --gene-annot annot.genes.annot \
  --gene-model multi=all --out gene_multi

# Gene-set analysis
$MAGMA --gene-results gene_pval.genes.raw --set-annot gene_sets.txt --out gsa_pval
$MAGMA --gene-results gene_raw_qt.genes.raw --set-annot gene_sets.txt --out gsa_raw_qt

# Gene property analysis
$MAGMA --gene-results gene_pval.genes.raw --gene-covar gene_covar.txt --out gprop_pval
```
