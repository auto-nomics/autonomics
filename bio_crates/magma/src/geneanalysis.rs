//! Gene-level analysis — the core MAGMA computation.
//!
//! For the summary-stats (`--pval`) path:
//! 1. For each gene, load genotypes for its SNPs from the PLINK reference panel
//! 2. Compute the SNP-SNP correlation matrix
//! 3. Eigendecompose to get λ values
//! 4. Convert SNP p-values → χ²(1) statistics, sum them
//! 5. Use Imhof's method for the gene-level p-value
//! 6. Convert to Z-statistic
//!
//! For the raw-data path:
//! 1. For each gene, load genotypes + phenotype
//! 2. Compute sufficient statistics (X'X, X'y, y'y)
//! 3. Apply the chosen model (PCreg, snpwise-mean, etc.)

use std::collections::HashMap;
use std::path::Path;

use faer::Mat;

use crate::error::{MagmaError, Result};
use crate::geneinput::{GeneAnnot, SnpPvalData};
use crate::plink::{BedFile, MISS};
use crate::stats::{
    DEFAULT_PVAL_TRUNCATE_HIGH, DEFAULT_PVAL_TRUNCATE_LOW, imhof_pvalue, pval_to_chisq1,
    pval_to_zstat, truncate_pval,
};

/// Gene analysis results for one gene.
#[derive(Debug, Clone)]
pub struct GeneResult {
    pub id: String,
    pub chr: i32,
    pub start: u64,
    pub end: u64,
    pub n_snps: usize,
    pub n_param: usize,
    pub n: i64,
    pub zstat: f64,
    pub pval: f64,
}

/// Configuration for the summary-stats gene analysis.
#[derive(Debug, Clone)]
pub struct PvalAnalysisConfig {
    /// P-value truncation bounds [low, high].
    pub truncate_low: f64,
    pub truncate_high: f64,
    /// Fixed sample size (if not using per-SNP N from file).
    pub fixed_n: Option<i64>,
    /// Rare variant aggregation: SNPs with MAF <= this AND MAC <= threshold are
    /// collapsed into burden scores. Set to 0.0 to disable.
    pub rare_maf_threshold: f64,
    pub rare_mac_threshold: i32,
}

impl Default for PvalAnalysisConfig {
    fn default() -> Self {
        Self {
            truncate_low: DEFAULT_PVAL_TRUNCATE_LOW,
            truncate_high: DEFAULT_PVAL_TRUNCATE_HIGH,
            fixed_n: None,
            rare_maf_threshold: 0.01,
            rare_mac_threshold: 100,
        }
    }
}

/// Run summary-stats gene analysis (`--pval` mode).
///
/// For each gene in `annot`, loads genotypes for its SNPs from `bed`,
/// computes the SNP correlation matrix, and derives the gene-level test
/// statistic using the SNP-wise mean model with Imhof's p-value.
pub fn analyze_pval(
    bed: &mut BedFile,
    annot: &GeneAnnot,
    pval_data: &SnpPvalData,
    config: &PvalAnalysisConfig,
) -> Result<Vec<GeneResult>> {
    // Clone snp_index to avoid holding an immutable borrow of bed while we
    // need mutable access for read_snp.
    let snp_index = bed.snp_index.clone();
    let n_indiv = bed.n_indiv();

    // Cache the entire .bed file in memory for fast SNP access.
    bed.cache_all()?;

    let mut results = Vec::with_capacity(annot.genes.len());

    for gene in &annot.genes {
        if gene.snps.is_empty() {
            continue;
        }

        // Resolve SNP indices in the PLINK data
        let mut snp_indices: Vec<usize> = Vec::new();
        let mut gene_pvals: Vec<f64> = Vec::new();

        for snp_id in &gene.snps {
            // Find SNP in PLINK data
            let Some(&idx) = snp_index.get(snp_id) else {
                continue; // SNP not in reference panel
            };
            snp_indices.push(idx);
            // Find p-value for this SNP
            let Some(&(p, _)) = pval_data.snps.get(snp_id) else {
                snp_indices.pop(); // no p-value, remove
                continue;
            };
            gene_pvals.push(p);
        }

        if snp_indices.is_empty() {
            continue;
        }

        // Determine sample size for this gene
        let n = config.fixed_n.unwrap_or_else(|| {
            // Use median N from matched SNPs if available
            let mut ns: Vec<i64> = pval_data.snps.values().filter_map(|(_, n)| *n).collect();
            if ns.is_empty() {
                n_indiv as i64
            } else {
                ns.sort_unstable();
                ns[ns.len() / 2]
            }
        });

        let n_snps = snp_indices.len();

        // Load genotypes: matrix of n_indiv × n_snps
        let genotypes = load_gene_genotypes(bed, &snp_indices, n_indiv)?;

        // Compute allele frequencies and filter rare variants
        let freqs = compute_freqs(&genotypes, n_indiv);

        // Compute SNP-SNP correlation matrix
        let corrs = compute_correlation(&genotypes, &freqs, n_indiv);

        // Eigendecompose the correlation matrix
        let eigenvalues = eigenvalues_symmetric(&corrs);

        // Filter eigenvalues like MAGMA's set_corrs:
        // cutoff = eigen_thresh * total / n (default eigen_thresh = 1e-4)
        let total: f64 = eigenvalues.iter().filter(|&&e| e > 0.0).sum();
        let eigen_thresh = 1e-4;
        let cutoff = eigen_thresh * total / eigenvalues.len() as f64;
        let lambda: Vec<f64> = eigenvalues
            .iter()
            .filter(|&&e| e > cutoff)
            .copied()
            .collect();

        if lambda.is_empty() {
            continue;
        }

        // Truncate p-values and convert to χ²(1) statistics
        let chisq_stats: Vec<f64> = gene_pvals
            .iter()
            .map(|&p| {
                let tp = truncate_pval(p, config.truncate_low, config.truncate_high);
                pval_to_chisq1(tp)
            })
            .collect();

        // Gene-level statistic: sum of χ² statistics
        let q: f64 = chisq_stats.iter().sum();

        // Compute gene p-value via Imhof's method
        let pval = imhof_pvalue(q, &lambda);

        // Compute Z-statistic
        let zstat = pval_to_zstat(pval);

        // Estimate effective number of parameters (NPARAM)
        let n_param = estimate_nparam(&eigenvalues);

        results.push(GeneResult {
            id: gene.id.clone(),
            chr: gene.chr,
            start: gene.start,
            end: gene.end,
            n_snps,
            n_param,
            n,
            zstat,
            pval,
        });
    }

    Ok(results)
}

/// Load genotypes for a gene's SNPs into an n_indiv × n_snps matrix.
/// Missing genotypes (dosage=3) are imputed to 2*freq (expected dosage).
///
/// SNP indices are sorted before reading to minimize disk seeks.
pub fn load_gene_genotypes(
    bed: &mut BedFile,
    snp_indices: &[usize],
    n_indiv: usize,
) -> Result<Mat<f64>> {
    let n_snp = snp_indices.len();

    // Sort SNP indices and remember original positions for column mapping
    let mut order: Vec<(usize, usize)> = snp_indices.iter().copied().enumerate().collect();
    order.sort_by_key(|&(_, idx)| idx);

    let mut geno = Mat::<f64>::zeros(n_indiv, n_snp);

    for &(orig_col, snp_idx) in &order {
        let dosages = bed.read_snp(snp_idx)?;
        for (i, &d) in dosages.iter().enumerate() {
            if i >= n_indiv {
                break;
            }
            geno[(i, orig_col)] = if d == MISS { f64::NAN } else { d };
        }
    }

    Ok(geno)
}

/// Compute per-SNP allele frequency (hom-alt allele frequency).
/// Frequency = mean(dosage) / 2, ignoring missing values.
pub fn compute_freqs(geno: &Mat<f64>, n_indiv: usize) -> Vec<f64> {
    let n_snp = geno.ncols();
    let mut freqs = vec![0.0f64; n_snp];
    for j in 0..n_snp {
        let mut sum = 0.0;
        let mut count = 0;
        for i in 0..n_indiv {
            let g = geno[(i, j)];
            if !g.is_nan() {
                sum += g;
                count += 1;
            }
        }
        freqs[j] = if count > 0 {
            sum / (2.0 * count as f64)
        } else {
            0.0
        };
    }
    freqs
}

/// Compute the n_snp × n_snp SNP-SNP correlation matrix.
///
/// Each SNP is mean-centered and divided by its standard deviation,
/// then R = (1/(n-1)) · X_c^T · X_c where X_c is the centered/scaled genotype matrix.
pub fn compute_correlation(geno: &Mat<f64>, freqs: &[f64], n_indiv: usize) -> Mat<f64> {
    let n_snp = geno.ncols();

    // Center and scale genotypes
    let mut x = Mat::<f64>::zeros(n_indiv, n_snp);
    for j in 0..n_snp {
        let mean = 2.0 * freqs[j];
        // Compute standard deviation
        let mut ss = 0.0;
        let mut count = 0;
        for i in 0..n_indiv {
            let g = geno[(i, j)];
            if !g.is_nan() {
                let d = g - mean;
                ss += d * d;
                count += 1;
            }
        }
        let var = if count > 1 {
            ss / (count as f64 - 1.0)
        } else {
            1.0
        };
        let sd = var.sqrt().max(1e-10);

        for i in 0..n_indiv {
            let g = geno[(i, j)];
            x[(i, j)] = if g.is_nan() { 0.0 } else { (g - mean) / sd };
        }
    }

    // R = (1/(n-1)) * X^T * X
    let xt = x.transpose();
    let r_raw = &xt * &x;
    let scale = 1.0 / (n_indiv as f64 - 1.0);
    let n_snp = r_raw.nrows();
    let mut r = Mat::from_fn(n_snp, n_snp, |i, j| r_raw[(i, j)] * scale);

    // Ensure diagonal is exactly 1.0 (numerical precision)
    for j in 0..n_snp {
        r[(j, j)] = 1.0;
    }

    r
}

/// Compute eigenvalues of a symmetric matrix using faer.
pub fn eigenvalues_symmetric(m: &Mat<f64>) -> Vec<f64> {
    let n = m.nrows();
    if n == 0 {
        return vec![];
    }
    if n == 1 {
        return vec![1.0];
    }

    let view = m.as_ref();
    let eig = view
        .self_adjoint_eigen(faer::Side::Lower)
        .expect("eigenvalue decomposition failed");
    let s = eig.S();

    let mut evals: Vec<f64> = (0..n).map(|i| s[i]).collect();
    // Sort descending
    evals.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    // Clamp tiny negative eigenvalues to 0
    evals = evals.into_iter().map(|e| e.max(0.0)).collect();
    evals
}

/// Estimate the effective number of parameters (NPARAM).
///
/// This follows MAGMA's approach: the eigenvalues of the correlation matrix
/// are used to estimate how many "independent" components exist. The formula
/// in the snpwise-mean model:
/// ```text
/// sdi_curr = 1/√(2·Σλ²)
/// sdi_mono = √0.5 / n_snps
/// sdi_indep = √(0.5/n_snps)
/// prop = clamp01((sdi_curr - sdi_mono) / (sdi_indep - sdi_mono))
/// n_param = round(1 + prop · (n_snps - 1))
/// ```
fn estimate_nparam(eigenvalues: &[f64]) -> usize {
    let n = eigenvalues.len();
    if n <= 1 {
        return 1;
    }
    let sum_sq: f64 = eigenvalues.iter().map(|e| e * e).sum();
    if sum_sq <= 0.0 {
        return 1;
    }
    let sdi_curr = 1.0 / (2.0 * sum_sq).sqrt();
    let sdi_mono = 0.5_f64.sqrt() / n as f64;
    let sdi_indep = (0.5 / n as f64).sqrt();
    let prop = ((sdi_curr - sdi_mono) / (sdi_indep - sdi_mono)).clamp(0.0, 1.0);
    let n_param = (1.0 + prop * (n as f64 - 1.0)).round() as usize;
    n_param.max(1)
}

// ─── Output ─────────────────────────────────────────────────────────────────

/// Write gene analysis results to a `.genes.out` file.
///
/// Format:
/// ```text
/// GENE  CHR  START  STOP  NSNPS  NPARAM  N  ZSTAT  P
/// ```
pub fn write_genes_out(results: &[GeneResult], path: &Path) -> Result<()> {
    use std::io::Write;
    let f = std::fs::File::create(path).map_err(MagmaError::Io)?;
    let mut w = std::io::BufWriter::new(f);

    writeln!(
        w,
        "GENE   CHR    START     STOP  NSNPS  NPARAM        N        ZSTAT            P"
    )
    .map_err(MagmaError::Io)?;

    for r in results {
        writeln!(
            w,
            "{:<8} {:>3} {:>9} {:>9}  {:>5}  {:>6}  {:>7}  {:>10.5}  {:>12.5e}",
            r.id, r.chr, r.start, r.end, r.n_snps, r.n_param, r.n, r.zstat, r.pval
        )
        .map_err(MagmaError::Io)?;
    }
    Ok(())
}

/// Write gene analysis results to a `.genes.raw` file (for gene-set analysis).
///
/// This is a simplified version of MAGMA's raw output format containing
/// gene-level sufficient statistics.
pub fn write_genes_raw(results: &[GeneResult], path: &Path) -> Result<()> {
    use std::io::Write;
    let f = std::fs::File::create(path).map_err(MagmaError::Io)?;
    let mut w = std::io::BufWriter::new(f);

    writeln!(w, "# model = snpwise_mean\n# version = 1.10-rust").map_err(MagmaError::Io)?;

    writeln!(
        w,
        "GENE   CHR    START     STOP  NSNPS  NPARAM    N        ZSTAT            P"
    )
    .map_err(MagmaError::Io)?;

    for r in results {
        writeln!(
            w,
            "{:<8} {:>3} {:>9} {:>9}  {:>5}  {:>6}  {:>7}  {:>10.5}  {:>12.5e}",
            r.id, r.chr, r.start, r.end, r.n_snps, r.n_param, r.n, r.zstat, r.pval
        )
        .map_err(MagmaError::Io)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_dir() -> PathBuf {
        PathBuf::from("tests/data")
    }

    #[test]
    fn test_pval_analysis_matches_golden() {
        let dir = test_dir();
        let mut bed = BedFile::open(&dir.join("sim_geno")).unwrap();
        let annot = GeneAnnot::read(&dir.join("annot.genes.annot")).unwrap();
        let pval_data =
            SnpPvalData::read(&dir.join("gwas_pval.txt"), "SNP", "P", None, Some(50000)).unwrap();

        let config = PvalAnalysisConfig {
            fixed_n: Some(50000),
            ..Default::default()
        };
        let results = analyze_pval(&mut bed, &annot, &pval_data, &config).unwrap();

        // Read golden output for comparison
        let golden = std::fs::read_to_string(dir.join("gene_pval.genes.out")).unwrap();
        let golden_lines: Vec<&str> = golden.lines().skip(1).collect(); // skip header

        assert_eq!(
            results.len(),
            golden_lines.len(),
            "gene count mismatch: {} vs {}",
            results.len(),
            golden_lines.len()
        );

        // Compare ZSTAT for first few genes
        for (i, r) in results.iter().take(5).enumerate() {
            let fields: Vec<&str> = golden_lines[i].split_whitespace().collect();
            let golden_z: f64 = fields[7].parse().unwrap();
            let golden_p: f64 = fields[8].parse().unwrap();

            // Allow some numerical tolerance due to integration differences
            let z_diff = (r.zstat - golden_z).abs();
            assert!(
                z_diff < 0.15,
                "gene {}: ZSTAT diff too large: {} vs {} (diff={})",
                r.id,
                r.zstat,
                golden_z,
                z_diff
            );

            let p_ratio = (r.pval.log10() - golden_p.log10()).abs();
            assert!(
                p_ratio < 0.5,
                "gene {}: log10(P) diff too large: {} vs {}",
                r.id,
                r.pval,
                golden_p
            );
        }
    }
}
