# MAGMA Real-Scale Test Data (chr22)

Real 1000 Genomes EUR reference panel (chr22) + Height GWAS summary statistics.
Used for high-fidelity validation of the Rust port against original MAGMA v1.10.

**Archive**: `aliyun:autonomics-data/magma/real_chr22/` (rclone restore)

## Data Sources

- **PLINK genotype**: 1000 Genomes Phase 3 EUR (chr22), 489 individuals × 141,123 SNPs
  - Source: `1000G_Phase3_plinkfiles.tgz` from Alkes Price LDSCORE server (hg19/GRCh37)
  - Local: `/mnt/disk3/autonomics_tree/port_sldsc/reference/ldsc_data/`
- **Gene locations**: chr22 subset of `Rev.NCBI37.3.gene.loc` (442 genes, gene symbols, hg19)
  - Source: https://github.com/endeneon/MAGMA_analysis_protocol
- **GWAS summary stats**: Height (Yengo et al. 2022), chr22 SNPs only, Z→P converted
  - Source: `sumstats_107/PASS.Height.Yengo2022.sumstats.gz`
  - 15,916 SNPs matched to chr22 panel
- **Gene sets**: miR137 cell-type-specific gene sets (iPS/Glut/NPC/GABA/DA)
  - Source: https://github.com/endeneon/MAGMA_analysis_protocol

## Files

| File | Description |
|------|-------------|
| `g1k_eur_chr22.bed/.bim/.fam` | PLINK binary genotype (1000G EUR chr22) |
| `gene_loc_chr22.txt` | 442 gene locations (gene symbols, chr22, hg19) |
| `gwas_height_chr22.txt` | Height GWAS p-values (SNP, P, N) — 15,916 chr22 SNPs |
| `gene_sets.txt` | 5 cell-type gene sets (iPS/Glut/NPC/GABA/DA) |

## Golden Outputs

### Annotation (35kb window)
```
magma --annotate window=35 --snp-loc g1k_eur_chr22.bim --gene-loc gene_loc_chr22.txt --out real_annot
```
- `real_annot.genes.annot` — 442 genes, 107,196 SNPs mapped (76%)

### Gene Analysis (summary-stats, snpwise-mean)
```
magma --bfile g1k_eur_chr22 --pval gwas_height_chr22.txt ncol=N snp-id=SNP pval=P \
  --gene-annot real_annot.genes.annot --out gene_pval
```
- `gene_pval.genes.out` — 432 genes with ZSTAT/P (e.g. MICAL3 p=1.6e-14, CECR5 p=3.8e-9)
- `gene_pval.genes.raw` — full sufficient statistics + gene-gene correlations

### Gene-Set Analysis
```
magma --gene-results gene_pval.genes.raw --set-annot gene_sets.txt col=2,1 --out gsa_out
```
- `gsa_out.gsa.out` — only iPS set testable on chr22 (others have <2 genes on chr22)

## Notes

- chr22 alone is sufficient for annotation + gene analysis validation
- Gene-set analysis requires genome-wide data for meaningful set sizes; chr22-only
  tests the code path but not biological signal
- For full genome-wide gene-set testing, extract all 22 chromosomes from the archive:
  ```bash
  cd /mnt/disk3/autonomics_tree/port_sldsc/reference/ldsc_data
  tar xzf 1000G_Phase3_plinkfiles.tgz
  ```
