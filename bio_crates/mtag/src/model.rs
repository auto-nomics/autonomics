//! Top-level MTAG pipeline driver.
//!
//! Port of the `mtag()` function in `mtag.py`.

use faer::Mat;

use crate::data::{self, DataConfig, MergedData, TraitData};
use crate::error::{MtagError, Result};
use crate::linalg::{cov2corr, pos_def_adjustment};
use crate::mtag::{MtagResult, mtag_analysis};
use crate::omega::{OmegaConfig, estimate_omega};

/// Full configuration for an MTAG analysis run.
#[derive(Clone, Debug)]
#[derive(Default)]
pub struct MtagConfig {
    /// Data loading / harmonisation options.
    pub data: DataConfig,
    /// Omega estimation options.
    pub omega: OmegaConfig,
    /// Pre-computed residual covariance matrix (skips LDSC estimation).
    pub residcov_path: Option<std::path::PathBuf>,
    /// Pre-computed genetic covariance matrix (skips estimation).
    pub gencov_path: Option<std::path::PathBuf>,
    /// Whether to output standardised betas.
    pub std_betas: bool,
    /// Whether to assume no sample overlap (Sigma is diagonal).
    pub no_overlap: bool,
}


/// Complete MTAG analysis result.
pub struct MtagAnalysis {
    /// Estimated residual covariance (Σ).
    pub sigma_hat: Mat<f64>,
    /// Estimated genetic covariance (Ω).
    pub omega_hat: Mat<f64>,
    /// MTAG-adjusted betas, SEs, and factors.
    pub result: MtagResult,
    /// The merged input data.
    pub data: MergedData,
}

/// Run the full MTAG pipeline.
///
/// Port of `mtag()` in `mtag.py`:
/// 1. Load and merge GWAS sumstats.
/// 2. Estimate Sigma (Σ) — provided externally or estimated via LDSC.
/// 3. Estimate Omega (Ω) — GMM (default) or numerical MLE.
/// 4. Perform MTAG analysis.
pub fn run_mtag(traits: &[TraitData], cfg: &MtagConfig) -> Result<MtagAnalysis> {
    if cfg.omega.equal_h2 && !cfg.omega.perfect_gencov {
        return Err(MtagError::InvalidInput(
            "equal_h2 requires perfect_gencov".into(),
        ));
    }

    // 1. Load and merge data.
    let merged = data::load_and_merge(traits, &cfg.data)?;

    let p = merged.zs.ncols();

    // 2. Estimate Sigma.
    let sigma_hat = if let Some(path) = &cfg.residcov_path {
        read_matrix(path)?
    } else {
        // Sigma must be provided externally for the library-only port.
        // The LDSC bivariate estimation is handled by the `ldsc` crate.
        return Err(MtagError::InvalidInput(
            "Sigma estimation via LDSC must be pre-computed; provide residcov_path".into(),
        ));
    };

    let sigma_hat = pos_def_adjustment(sigma_hat, 0.99, 1000)?;

    // Check chi2.
    let _mean_c2: Vec<f64> = (0..p)
        .map(|j| {
            let col_sq: f64 = (0..merged.zs.nrows())
                .map(|i| merged.zs[(i, j)] * merged.zs[(i, j)])
                .sum::<f64>()
                / merged.zs.nrows() as f64;
            col_sq / sigma_hat[(j, j)]
        })
        .collect();

    // 3. Estimate Omega.
    let omega_hat = if let Some(path) = &cfg.gencov_path {
        read_matrix(path)?
    } else {
        // Extract non-strand-ambiguous subset for Omega estimation.
        let (zs_sa, ns_sa) = data::filter_strand_ambig(&merged);
        estimate_omega(&zs_sa, &ns_sa, &sigma_hat, &cfg.omega)?
    };

    // 4. MTAG analysis.
    let result = mtag_analysis(&merged.zs, &merged.ns, &omega_hat, &sigma_hat);

    Ok(MtagAnalysis {
        sigma_hat,
        omega_hat,
        result,
        data: merged,
    })
}

/// Run MTAG with pre-computed Sigma and Omega.
///
/// This is the common library entry point for the core MTAG calculation,
/// given already-estimated Σ and Ω matrices.
pub fn run_mtag_with_matrices(
    traits: &[TraitData],
    cfg: &MtagConfig,
    sigma_hat: Mat<f64>,
    omega_hat: Mat<f64>,
) -> Result<MtagAnalysis> {
    // 1. Load and merge data.
    let merged = data::load_and_merge(traits, &cfg.data)?;

    // 2. Posdef-adjust Sigma.
    let sigma_hat = pos_def_adjustment(sigma_hat, 0.99, 1000)?;

    // 3. MTAG analysis.
    let result = mtag_analysis(&merged.zs, &merged.ns, &omega_hat, &sigma_hat);

    Ok(MtagAnalysis {
        sigma_hat,
        omega_hat,
        result,
        data: merged,
    })
}

/// Read a P×P matrix from a whitespace-delimited text file.
///
/// Port of `_read_matrix` in `mtag.py` (the .txt path only).
fn read_matrix(path: &std::path::Path) -> Result<Mat<f64>> {
    let content = std::fs::read_to_string(path)?;
    let rows: Vec<Vec<f64>> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            l.split_whitespace()
                .map(|v| v.parse::<f64>().unwrap_or(0.0))
                .collect()
        })
        .collect();

    if rows.is_empty() {
        return Err(MtagError::InvalidInput("empty matrix file".into()));
    }

    let n = rows.len();
    let mut mat = Mat::zeros(n, n);
    for (i, row) in rows.iter().enumerate() {
        for (j, &val) in row.iter().enumerate() {
            mat[(i, j)] = val;
        }
    }
    Ok(mat)
}

/// Format the Omega and Sigma summary for logging.
pub fn format_summary(analysis: &MtagAnalysis) -> String {
    let p = analysis.omega_hat.nrows();

    let mut out = String::new();
    out.push_str("\nEstimated Omega:\n");
    for i in 0..p {
        let row: Vec<String> = (0..p)
            .map(|j| format!("{:.6}", analysis.omega_hat[(i, j)]))
            .collect();
        out.push_str(&format!("{}\n", row.join("  ")));
    }

    let omega_corr = cov2corr(&analysis.omega_hat);
    out.push_str("\nOmega (Correlation):\n");
    for i in 0..p {
        let row: Vec<String> = (0..p)
            .map(|j| format!("{:.6}", omega_corr[(i, j)]))
            .collect();
        out.push_str(&format!("{}\n", row.join("  ")));
    }

    out.push_str("\nEstimated Sigma:\n");
    for i in 0..p {
        let row: Vec<String> = (0..p)
            .map(|j| format!("{:.6}", analysis.sigma_hat[(i, j)]))
            .collect();
        out.push_str(&format!("{}\n", row.join("  ")));
    }

    let sigma_corr = cov2corr(&analysis.sigma_hat);
    out.push_str("\nSigma (Correlation):\n");
    for i in 0..p {
        let row: Vec<String> = (0..p)
            .map(|j| format!("{:.6}", sigma_corr[(i, j)]))
            .collect();
        out.push_str(&format!("{}\n", row.join("  ")));
    }

    // Average MTAG weight factors.
    let m = analysis.result.mtag_factor.nrows();
    let avg_factors: Vec<f64> = (0..p)
        .map(|j| {
            (0..m)
                .map(|i| analysis.result.mtag_factor[(i, j)])
                .sum::<f64>()
                / m as f64
        })
        .collect();
    out.push_str("\nMTAG weight factors (average across SNPs):\n");
    let row: Vec<String> = avg_factors.iter().map(|v| format!("{:.6}", v)).collect();
    out.push_str(&format!("{}\n", row.join("  ")));

    out
}
