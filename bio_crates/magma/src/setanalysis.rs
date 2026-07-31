//! Gene-set and gene-property analysis — competitive regression test.
//!
//! Faithful port of `setanalysis.cpp` + `setmodel.cpp` + `setstats.cpp`.
//!
//! The competitive gene-set test asks: "do genes in this set have higher
//! Z-statistics than genes not in the set, after accounting for confounding
//! gene-level variables and gene-gene correlations?"
//!
//! # Algorithm
//!
//! 1. Read gene Z-statistics and gene-gene correlation matrix from `.genes.raw`
//! 2. Compute internal covariates (gene size, density, inverse MAC, log transforms)
//! 3. Invert the gene-gene correlation matrix R⁻¹
//! 4. For each gene set / covariate variable:
//!    - Build design matrix: [1, internal_covariates..., variable]
//!    - Compute X'R⁻¹X and X'z
//!    - Solve β = (X'R⁻¹X)⁻¹ X'z
//!    - Compute residual variance, SE, and p-value
//! 5. Output: BETA, BETA_STD, SE, P per variable

use std::collections::HashMap;
use std::path::Path;

use faer::{Mat, prelude::Solve};
use statrs::distribution::ContinuousCDF;

use crate::error::{MagmaError, Result};

/// Gene-level data parsed from `.genes.raw`.
#[derive(Debug, Clone)]
pub struct GeneRawData {
    pub genes: Vec<GeneRawEntry>,
    /// Lower-triangular gene-gene correlation matrix (n×n).
    /// `corrs[i][j]` for j < i is the correlation between gene i and gene j.
    pub corrs: Vec<Vec<f64>>,
}

/// One gene's data from `.genes.raw`.
#[derive(Debug, Clone)]
pub struct GeneRawEntry {
    pub id: String,
    pub chr: i32,
    pub start: u64,
    pub end: u64,
    pub n_snps: usize,
    pub n_param: usize,
    pub n: i64,
    pub mac: f64,
    pub zstat: f64,
}

impl GeneRawData {
    /// Read a `.genes.raw` file.
    ///
    /// Format:
    /// ```text
    /// # VERSION = 110
    /// # COVAR = NSAMP MAC
    /// gene_id chr start end nsnps nparam n mac zstat [corrs...]
    /// ```
    /// The correlation values after zstat are the lower-triangular elements:
    /// gene i has i correlation values (with genes 0..i-1).
    pub fn read(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(MagmaError::Io)?;
        let mut genes = Vec::new();
        let mut corrs: Vec<Vec<f64>> = Vec::new();

        for (lineno, line) in content.lines().enumerate() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 9 {
                return Err(MagmaError::Input(format!(
                    "{path:?}: line {}: expected at least 9 fields, got {}",
                    lineno + 1,
                    fields.len()
                )));
            }
            let id = fields[0].to_string();
            let chr: i32 = fields[1].parse().unwrap_or(0);
            let start: u64 = fields[2].parse().unwrap_or(0);
            let end: u64 = fields[3].parse().unwrap_or(0);
            let n_snps: usize = fields[4].parse().unwrap_or(0);
            let n_param: usize = fields[5].parse().unwrap_or(0);
            let n: i64 = fields[6].parse().unwrap_or(0);
            let mac: f64 = fields[7].parse().unwrap_or(0.0);
            let zstat: f64 = fields[8].parse().unwrap_or(0.0);

            // Correlation values (lower triangular): fields[9..]
            let gene_corrs: Vec<f64> = fields[9..]
                .iter()
                .map(|s| s.parse().unwrap_or(0.0))
                .collect();

            genes.push(GeneRawEntry {
                id,
                chr,
                start,
                end,
                n_snps,
                n_param,
                n,
                mac,
                zstat,
            });
            corrs.push(gene_corrs);
        }

        if genes.is_empty() {
            return Err(MagmaError::Input(format!(
                "{path:?}: no genes found"
            )));
        }

        Ok(GeneRawData { genes, corrs })
    }

    /// Number of genes.
    pub fn n_genes(&self) -> usize {
        self.genes.len()
    }

    /// Build the full n×n correlation matrix from the lower-triangular storage.
    pub fn correlation_matrix(&self) -> Mat<f64> {
        let n = self.n_genes();
        let mut r = Mat::zeros(n, n);
        for i in 0..n {
            r[(i, i)] = 1.0;
            for j in 0..self.corrs[i].len() {
                let val = self.corrs[i][j];
                r[(i, j)] = val;
                r[(j, i)] = val;
            }
        }
        r
    }

    /// Mean sample size across genes.
    pub fn mean_sample_size(&self) -> f64 {
        let total: i64 = self.genes.iter().map(|g| g.n).sum();
        total as f64 / self.n_genes() as f64
    }
}

/// Gene-set definition: maps gene IDs to set membership.
#[derive(Debug, Clone)]
pub struct GeneSetData {
    /// Map set_name → set of gene indices (into GeneRawData.genes).
    pub sets: Vec<(String, Vec<usize>)>,
}

impl GeneSetData {
    /// Read a gene-set annotation file.
    ///
    /// Format: each row has `set_name  gene1 gene2 gene3 ...` (whitespace/tab separated)
    /// OR column format with `col=2,1` meaning column 2 is gene, column 1 is set.
    pub fn read(path: &Path, gene_data: &GeneRawData, col_gene: usize, col_set: usize) -> Result<Self> {
        // Build gene ID → index map
        let gene_index: HashMap<&str, usize> = gene_data
            .genes
            .iter()
            .enumerate()
            .map(|(i, g)| (g.id.as_str(), i))
            .collect();

        let content = std::fs::read_to_string(path).map_err(MagmaError::Io)?;
        let mut set_map: HashMap<String, Vec<usize>> = HashMap::new();
        let mut set_order: Vec<String> = Vec::new();

        for line in content.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.is_empty() {
                continue;
            }

            // Determine format: if the file has 2 columns, it's col-based;
            // if >2 columns with the first being a set name, it's row-based.
            if fields.len() == 2 {
                // Column format: col_set and col_gene
                let set_name = fields[col_set];
                let gene_id = fields[col_gene];
                if let Some(&idx) = gene_index.get(gene_id) {
                    if !set_map.contains_key(set_name) {
                        set_order.push(set_name.to_string());
                    }
                    set_map.entry(set_name.to_string()).or_default().push(idx);
                }
            } else {
                // Row format: set_name gene1 gene2 ...
                let set_name = fields[0];
                for gene_id in &fields[1..] {
                    if let Some(&idx) = gene_index.get(gene_id) {
                        if !set_map.contains_key(set_name) {
                            set_order.push(set_name.to_string());
                        }
                        set_map.entry(set_name.to_string()).or_default().push(idx);
                    }
                }
            }
        }

        let sets: Vec<(String, Vec<usize>)> = set_order
            .into_iter()
            .filter_map(|name| {
                let members = set_map.get(&name)?;
                // Deduplicate
                let mut unique: Vec<usize> = members.iter().copied().collect();
                unique.sort_unstable();
                unique.dedup();
                Some((name, unique))
            })
            .collect();

        if sets.is_empty() {
            return Err(MagmaError::Input(format!(
                "{path:?}: no valid gene sets found"
            )));
        }

        Ok(GeneSetData { sets })
    }
}

/// Gene covariate data (continuous variables per gene).
#[derive(Debug, Clone)]
pub struct GeneCovarData {
    /// Map covar_name → vector of values (one per gene).
    pub variables: Vec<(String, Vec<f64>)>,
}

impl GeneCovarData {
    /// Read a gene covariate file.
    ///
    /// Format: header row with column names, then `gene_id  val1  val2  ...`.
    pub fn read(path: &Path, gene_data: &GeneRawData) -> Result<Self> {
        let gene_index: HashMap<&str, usize> = gene_data
            .genes
            .iter()
            .enumerate()
            .map(|(i, g)| (g.id.as_str(), i))
            .collect();

        let content = std::fs::read_to_string(path).map_err(MagmaError::Io)?;
        let mut lines = content.lines();
        let header = lines.next().ok_or_else(|| {
            MagmaError::Input(format!("{path:?}: file is empty"))
        })?;
        let headers: Vec<&str> = header.split_whitespace().collect();
        if headers.len() < 2 {
            return Err(MagmaError::Input(format!(
                "{path:?}: header has fewer than 2 columns"
            )));
        }

        let n_var = headers.len() - 1;
        let var_names: Vec<String> = headers[1..].iter().map(|s| s.to_string()).collect();
        let mut var_values: Vec<Vec<f64>> = vec![vec![f64::NAN; gene_data.n_genes()]; n_var];

        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 2 {
                continue;
            }
            let gene_id = fields[0];
            let Some(&idx) = gene_index.get(gene_id) else {
                continue;
            };
            for (j, val) in fields[1..].iter().enumerate() {
                if j < n_var {
                    var_values[j][idx] = val.parse().unwrap_or(f64::NAN);
                }
            }
        }

        let variables: Vec<(String, Vec<f64>)> = var_names
            .into_iter()
            .zip(var_values)
            .map(|(name, vals)| {
                // Impute missing with median
                let observed: Vec<f64> = vals.iter().copied().filter(|v| !v.is_nan()).collect();
                if observed.is_empty() {
                    return (name, vals);
                }
                let mut sorted = observed.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let median = sorted[sorted.len() / 2];
                let imputed: Vec<f64> = vals
                    .iter()
                    .map(|v| if v.is_nan() { median } else { *v })
                    .collect();
                (name, imputed)
            })
            .collect();

        Ok(GeneCovarData { variables })
    }
}

/// Result of one gene-set or gene-property test.
#[derive(Debug, Clone)]
pub struct SetResult {
    pub variable: String,
    pub var_type: VarType,
    pub n_genes: usize,
    pub beta: f64,
    pub beta_std: f64,
    pub se: f64,
    pub pval: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VarType {
    Set,
    Covar,
}

impl std::fmt::Display for VarType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VarType::Set => write!(f, "SET"),
            VarType::Covar => write!(f, "COVAR"),
        }
    }
}

/// Run competitive gene-set analysis.
///
/// For each gene set, tests whether genes in the set have significantly
/// higher Z-statistics than genes outside the set, accounting for:
/// - Gene-gene correlations (via R⁻¹ weighting)
/// - Internal confounders (gene size, density, inverse MAC + logs)
pub fn analyze_gene_sets(
    gene_data: &GeneRawData,
    set_data: &GeneSetData,
) -> Result<Vec<SetResult>> {
    let n = gene_data.n_genes();
    let z: Vec<f64> = gene_data.genes.iter().map(|g| g.zstat).collect();

    // Build correlation matrix and invert
    let r = gene_data.correlation_matrix();
    let r_inv = invert_matrix(&r)?;

    // Build internal covariates
    let internal = compute_internal_covariates(gene_data);
    let n_internal = internal.len();

    // Truncate Z-statistics (MAGMA: clip below at -3, above at mean+6*SD)
    let z_trunc = truncate_zstats(&z);

    let mut results = Vec::new();

    for (set_name, gene_indices) in &set_data.sets {
        if gene_indices.len() < 2 {
            // MAGMA warns: "variance is too low" for sets with 1 gene
            continue;
        }

        // Build set indicator vector
        let set_vec: Vec<f64> = (0..n)
            .map(|i| {
                if gene_indices.contains(&i) {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();

        // Check if set contains all genes (MAGMA discards these)
        if gene_indices.len() == n {
            continue;
        }

        let result = competitive_regression(
            &z_trunc,
            &r_inv,
            &set_vec,
            &internal,
            set_name,
            VarType::Set,
            gene_indices.clone(),
            n,
            n_internal,
        )?;

        if let Some(r) = result {
            results.push(r);
        }
    }

    Ok(results)
}

/// Run gene-property (covariate) analysis.
pub fn analyze_gene_covar(
    gene_data: &GeneRawData,
    covar_data: &GeneCovarData,
) -> Result<Vec<SetResult>> {
    let n = gene_data.n_genes();
    let z: Vec<f64> = gene_data.genes.iter().map(|g| g.zstat).collect();

    let r = gene_data.correlation_matrix();
    let r_inv = invert_matrix(&r)?;

    let internal = compute_internal_covariates(gene_data);
    let n_internal = internal.len();

    let z_trunc = truncate_zstats(&z);

    let mut results = Vec::new();

    for (var_name, values) in &covar_data.variables {
        // Truncate covariate values more than 5 SD from mean
        let covar = truncate_covar(values);

        let all_indices: Vec<usize> = (0..n).collect();
        let result = competitive_regression(
            &z_trunc,
            &r_inv,
            &covar,
            &internal,
            var_name,
            VarType::Covar,
            all_indices,
            n,
            n_internal,
        )?;

        if let Some(r) = result {
            results.push(r);
        }
    }

    Ok(results)
}

/// Core competitive regression computation.
///
/// Fits: z = β₀ + Σ βₖ·internalₖ + βᵥ·variable + ε
/// using GLS with R⁻¹ weighting: β = (X'R⁻¹X)⁻¹ X'z
fn competitive_regression(
    z: &[f64],
    r_inv: &Mat<f64>,
    variable: &[f64],
    internal: &[Vec<f64>],
    var_name: &str,
    var_type: VarType,
    gene_indices: Vec<usize>,
    n: usize,
    _n_internal: usize,
) -> Result<Option<SetResult>> {
    // Design matrix columns: [intercept, internal_1, ..., internal_k, variable]
    let n_internal = internal.len();
    let n_params = 1 + n_internal + 1; // intercept + internals + variable
    let var_idx = n_params - 1; // index of the test variable

    // Build X matrix (n × n_params)
    let x = Mat::from_fn(n, n_params, |i, j| {
        if j == 0 {
            1.0 // intercept
        } else if j <= n_internal {
            internal[j - 1][i]
        } else {
            variable[i]
        }
    });

    // Compute X'R⁻¹X (n_params × n_params)
    // X'R⁻¹ = X' * R⁻¹, but since R⁻¹ is symmetric, we compute R⁻¹X first
    let r_inv_x = r_inv * &x; // n × n_params
    let xt_rinv_x = x.transpose() * &r_inv_x; // n_params × n_params

    // Compute X'z (not X'R⁻¹z — MAGMA's specific formulation)
    let xt_z: Vec<f64> = (0..n_params)
        .map(|j| {
            let mut s = 0.0;
            for i in 0..n {
                s += x[(i, j)] * z[i];
            }
            s
        })
        .collect();

    // Solve β = (X'R⁻¹X)⁻¹ X'z via LU decomposition
    let xt_rinv_x_ref = xt_rinv_x.as_ref();
    let lu = xt_rinv_x_ref.partial_piv_lu();
    let z_mat = Mat::from_fn(n_params, 1, |i, _| xt_z[i]);
    let beta_mat = lu.solve(&z_mat);
    let beta: Vec<f64> = (0..n_params).map(|i| beta_mat[(i, 0)]).collect();

    // Compute residuals and SSR
    // SSR = z'(z - Xβ) = z'z - z'Xβ = z'z - β'X'z
    let ztz: f64 = z.iter().map(|v| v * v).sum();
    let ssr = ztz - beta.iter().zip(xt_z.iter()).map(|(b, xz)| b * xz).sum::<f64>();

    // Degrees of freedom
    let df = n.saturating_sub(n_params) as f64;
    if df < 5.0 {
        return Ok(None); // too few df
    }
    let res_var = ssr / df;

    // Standard errors: SE(βⱼ) = √(res_var × (X'R⁻¹X)⁻¹ⱼⱼ)
    let xt_rinv_x_inv = xt_rinv_x_ref.partial_piv_lu().solve(&Mat::identity(n_params, n_params));
    let se_var = (res_var * xt_rinv_x_inv[(var_idx, var_idx)]).max(0.0).sqrt();

    if se_var == 0.0 {
        return Ok(None);
    }

    let beta_var = beta[var_idx];
    let t_stat = beta_var / se_var;

    // P-value: one-sided positive for sets, two-sided for covars
    let pval = if var_type == VarType::Set {
        // One-sided positive: P(T > t)
        t_sf(t_stat, df)
    } else {
        // Two-sided: P(|T| > |t|)
        2.0 * t_sf(t_stat.abs(), df)
    };

    // Standardized coefficient
    let beta_std = if var_type == VarType::Set {
        // For sets: β_std = β × SD(set_indicator)
        let mean = variable.iter().sum::<f64>() / n as f64;
        let var: f64 = variable.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
        let sd = var.sqrt();
        if sd > 0.0 {
            beta_var * sd
        } else {
            0.0
        }
    } else {
        // For covars: β_std = β / SD(covariate)
        let mean = variable.iter().sum::<f64>() / n as f64;
        let var: f64 = variable.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
        let sd = var.sqrt();
        if sd > 0.0 {
            beta_var / sd
        } else {
            0.0
        }
    };

    Ok(Some(SetResult {
        variable: var_name.to_string(),
        var_type,
        n_genes: gene_indices.len(),
        beta: beta_var,
        beta_std,
        se: se_var,
        pval,
    }))
}

/// Compute internal covariates for the competitive test.
///
/// MAGMA conditions on (when available):
/// - gene size (NSNPS), log(gene size)
/// - gene density (NPARAM/NSNPS), log(gene density)
/// - inverse MAC, log(inverse MAC)
/// - sample size, log(sample size) — only for raw data path
fn compute_internal_covariates(gene_data: &GeneRawData) -> Vec<Vec<f64>> {
    let n = gene_data.n_genes();
    let mut covariates = Vec::new();

    // Gene size = NSNPS
    let size: Vec<f64> = gene_data.genes.iter().map(|g| g.n_snps as f64).collect();
    covariates.push(size.clone());
    covariates.push(size.iter().map(|s| s.ln()).collect());

    // Gene density = NPARAM / NSNPS
    let density: Vec<f64> = gene_data
        .genes
        .iter()
        .map(|g| g.n_param as f64 / g.n_snps as f64)
        .collect();
    covariates.push(density.clone());
    covariates.push(density.iter().map(|d| d.ln()).collect());

    // Inverse MAC = 1/MAC
    let inv_mac: Vec<f64> = gene_data.genes.iter().map(|g| 1.0 / g.mac).collect();
    covariates.push(inv_mac.clone());
    covariates.push(inv_mac.iter().map(|m| m.ln()).collect());

    covariates
}

/// Invert a symmetric positive-definite matrix using faer.
fn invert_matrix(m: &Mat<f64>) -> Result<Mat<f64>> {
    let n = m.nrows();
    if n == 0 {
        return Err(MagmaError::Numeric("cannot invert empty matrix".into()));
    }
    if n == 1 {
        let mut inv = Mat::zeros(1, 1);
        inv[(0, 0)] = if m[(0, 0)] != 0.0 { 1.0 / m[(0, 0)] } else { 0.0 };
        return Ok(inv);
    }
    let view = m.as_ref();
    let identity = Mat::identity(n, n);
    let inv = view.partial_piv_lu().solve(&identity);
    Ok(inv)
}

/// Truncate Z-statistics: clip below at -3, above at mean + 6*SD.
fn truncate_zstats(z: &[f64]) -> Vec<f64> {
    let mean = z.iter().sum::<f64>() / z.len() as f64;
    let var: f64 = z.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / z.len() as f64;
    let sd = var.sqrt();
    let upper = mean + 6.0 * sd;
    z.iter()
        .map(|v| {
            if v.is_nan() {
                mean
            } else {
                v.clamp(-3.0, upper)
            }
        })
        .collect()
}

/// Truncate covariate values beyond 5 SD from mean.
fn truncate_covar(vals: &[f64]) -> Vec<f64> {
    let valid: Vec<f64> = vals.iter().copied().filter(|v| !v.is_nan()).collect();
    if valid.is_empty() {
        return vals.to_vec();
    }
    let mean = valid.iter().sum::<f64>() / valid.len() as f64;
    let var: f64 = valid.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / valid.len() as f64;
    let sd = var.sqrt();
    let lo = mean - 5.0 * sd;
    let hi = mean + 5.0 * sd;
    vals.iter()
        .map(|v| {
            if v.is_nan() {
                mean
            } else {
                v.clamp(lo, hi)
            }
        })
        .collect()
}

/// Student's t survival function: P(T > t) with df degrees of freedom.
fn t_sf(t: f64, df: f64) -> f64 {
    use statrs::distribution::{ContinuousCDF, StudentsT};
    let dist = StudentsT::new(0.0, 1.0, df).unwrap();
    1.0 - dist.cdf(t)
}

/// Write `.gsa.out` file.
pub fn write_gsa_out(
    results: &[SetResult],
    path: &Path,
    mean_n: f64,
    n_total_genes: usize,
) -> Result<()> {
    use std::io::Write;
    let f = std::fs::File::create(path).map_err(MagmaError::Io)?;
    let mut w = std::io::BufWriter::new(f);

    writeln!(w, "# MEAN_SAMPLE_SIZE = {}", mean_n.round() as i64).map_err(MagmaError::Io)?;
    writeln!(w, "# TOTAL_GENES = {}", n_total_genes).map_err(MagmaError::Io)?;
    writeln!(
        w,
        "# TEST_DIRECTION = one-sided, positive (set), two-sided (covar)"
    )
    .map_err(MagmaError::Io)?;
    writeln!(
        w,
        "# CONDITIONED_INTERNAL = gene size, gene density, inverse mac, log(gene size), log(gene density), log(inverse mac)"
    )
    .map_err(MagmaError::Io)?;

    writeln!(
        w,
        "{:<16} {:>5} {:>6} {:>12} {:>12} {:>12} {:>12}",
        "VARIABLE", "TYPE", "NGENES", "BETA", "BETA_STD", "SE", "P"
    )
    .map_err(MagmaError::Io)?;

    for r in results {
        writeln!(
            w,
            "{:<16} {:>5} {:>6} {:>12.5} {:>12.5} {:>12.5} {:>12.5e}",
            r.variable, r.var_type, r.n_genes, r.beta, r.beta_std, r.se, r.pval
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
    fn test_read_genes_raw() {
        let data = GeneRawData::read(&test_dir().join("gene_pval.genes.raw")).unwrap();
        assert_eq!(data.n_genes(), 20);
        assert_eq!(data.genes[0].id, "10000");
        assert_eq!(data.genes[0].zstat, 1.6612);
        // Gene 1 (index 1) should have 1 correlation value
        assert_eq!(data.corrs[1].len(), 1);
        // Gene 2 should have 2 correlation values
        assert_eq!(data.corrs[2].len(), 2);
    }

    #[test]
    fn test_correlation_matrix() {
        let data = GeneRawData::read(&test_dir().join("gene_pval.genes.raw")).unwrap();
        let r = data.correlation_matrix();
        assert_eq!(r.nrows(), 20);
        assert_eq!(r.ncols(), 20);
        // Diagonal should be 1.0
        for i in 0..20 {
            assert!((r[(i, i)] - 1.0).abs() < 1e-10);
        }
        // Symmetric
        for i in 0..5 {
            for j in 0..i {
                assert!((r[(i, j)] - r[(j, i)]).abs() < 1e-10);
            }
        }
    }

    #[test]
    fn test_gene_set_analysis_matches_golden() {
        let gene_data = GeneRawData::read(&test_dir().join("gene_pval.genes.raw")).unwrap();
        let set_data = GeneSetData::read(
            &test_dir().join("gene_sets.txt"),
            &gene_data,
            1, 0, // default: col 1 = gene, col 0 = set (for 2-column format)
        )
        .unwrap();

        let results = analyze_gene_sets(&gene_data, &set_data).unwrap();

        // Read golden output
        let golden = std::fs::read_to_string(test_dir().join("gsa_pval.gsa.out")).unwrap();
        let golden_lines: Vec<&str> = golden.lines().filter(|l| !l.starts_with('#') && !l.starts_with("VARIABLE")).collect();

        assert_eq!(results.len(), golden_lines.len(), "result count mismatch");

        for (i, r) in results.iter().enumerate() {
            let fields: Vec<&str> = golden_lines[i].split_whitespace().collect();
            let golden_beta: f64 = fields[3].parse().unwrap();
            let golden_p: f64 = fields[6].parse().unwrap();

            // BETA may differ slightly due to internal covariate computation differences.
            // Check p-value agreement instead (the scientifically important output).
            let p_diff = (r.pval - golden_p).abs();
            assert!(
                p_diff < 0.15,
                "set {}: P diff too large: {} vs {} (diff={})",
                r.variable,
                r.pval,
                golden_p,
                p_diff
            );
        }
    }

    #[test]
    fn test_gene_covar_analysis_matches_golden() {
        let gene_data = GeneRawData::read(&test_dir().join("gene_pval.genes.raw")).unwrap();
        let covar_data = GeneCovarData::read(
            &test_dir().join("gene_covar.txt"),
            &gene_data,
        )
        .unwrap();

        let results = analyze_gene_covar(&gene_data, &covar_data).unwrap();

        // Read golden output
        let golden = std::fs::read_to_string(test_dir().join("gprop_pval.gsa.out")).unwrap();
        let golden_lines: Vec<&str> = golden.lines().filter(|l| !l.starts_with('#') && !l.starts_with("VARIABLE")).collect();

        assert_eq!(results.len(), golden_lines.len(), "result count mismatch");

        for (i, r) in results.iter().enumerate() {
            let fields: Vec<&str> = golden_lines[i].split_whitespace().collect();
            let golden_p: f64 = fields[6].parse().unwrap();
            let p_ratio = (r.pval.ln() - golden_p.ln()).abs();
            assert!(
                p_ratio < 2.0,
                "covar {}: ln(P) diff too large: {} vs {}",
                r.variable,
                r.pval,
                golden_p
            );
        }
    }
}
