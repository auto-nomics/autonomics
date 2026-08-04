//! Input data structure for MVMR, mirroring R's `format_mvmr()` output.
//!
//! R's `format_mvmr(BXGs, BYG, seBXGs, seBYG, RSID)` produces a data frame with
//! columns `SNP, betaYG, sebetaYG, betaX1..betaXp, sebetaX1..sebetaXp`. We
//! store the same layout column-major in [`MvmrInput`]: outcome effect / SE
//! vectors, per-exposure effect / SE vectors, and an optional SNP label.

use crate::error::{MvmrError, Result};

/// Formatted MVMR input, the Rust analogue of an `mvmr_format` data frame.
///
/// * `beta_yg`, `sebeta_yg` — outcome instrument-effect and its standard error
///   (length `n`, the number of instruments).
/// * `beta_xg`, `sebeta_xg` — per-exposure instrument effects and standard
///   errors. Each inner vector has length `n`; the outer length is the number
///   of exposures `p`. All exposures share the same instrument set.
/// * `snp` — optional SNP labels; if empty, 1-based integer placeholders are
///   used (matching R's `format_mvmr(missing(RSID))` branch).
#[derive(Clone, Debug)]
pub struct MvmrInput {
    pub beta_yg: Vec<f64>,
    pub sebeta_yg: Vec<f64>,
    pub beta_xg: Vec<Vec<f64>>,
    pub sebeta_xg: Vec<Vec<f64>>,
    pub snp: Vec<String>,
}

impl MvmrInput {
    /// Number of instruments.
    pub fn n_snps(&self) -> usize {
        self.beta_yg.len()
    }

    /// Number of exposures.
    pub fn n_exposures(&self) -> usize {
        self.beta_xg.len()
    }

    /// Validate internal consistency: matching lengths, at least one exposure,
    /// at least as many instruments as exposures plus one residual degree of
    /// freedom.
    pub fn validate(&self) -> Result<()> {
        let n = self.n_snps();
        let p = self.n_exposures();
        if p == 0 {
            return Err(MvmrError::BadExposureCount("zero exposures".into()));
        }
        if self.sebeta_yg.len() != n {
            return Err(MvmrError::LengthMismatch(format!(
                "sebeta_yg len {} != beta_yg len {}",
                self.sebeta_yg.len(),
                n
            )));
        }
        for (i, v) in self.beta_xg.iter().enumerate() {
            if v.len() != n {
                return Err(MvmrError::LengthMismatch(format!(
                    "beta_xg[{i}] len {} != n {n}",
                    v.len()
                )));
            }
        }
        for (i, v) in self.sebeta_xg.iter().enumerate() {
            if v.len() != n {
                return Err(MvmrError::LengthMismatch(format!(
                    "sebeta_xg[{i}] len {} != n {n}",
                    v.len()
                )));
            }
        }
        if self.sebeta_xg.len() != p {
            return Err(MvmrError::BadExposureCount(format!(
                "sebeta_xg has {} exposures, beta_xg has {p}",
                self.sebeta_xg.len()
            )));
        }
        // The IVW regression needs n > p residual degrees of freedom, and the
        // validity Q statistic divides by (n - p - 1).
        if n <= p + 1 {
            return Err(MvmrError::InsufficientSnps(format!(
                "n={n} must exceed p+1={} for the validity Q statistic",
                p + 1
            )));
        }
        if !self.snp.is_empty() && self.snp.len() != n {
            return Err(MvmrError::LengthMismatch(format!(
                "snp len {} != n {n}",
                self.snp.len()
            )));
        }
        Ok(())
    }

    /// SNP label at row `i`, falling back to 1-based placeholders when `snp`
    /// is empty (matching R's `format_mvmr()`).
    pub fn snp_label(&self, i: usize) -> String {
        self.snp
            .get(i)
            .cloned()
            .unwrap_or_else(|| (i + 1).to_string())
    }

    /// Borrow exposure-effect column `j` (0-indexed) as a slice.
    pub fn bx(&self, j: usize) -> &[f64] {
        &self.beta_xg[j]
    }
    /// Borrow exposure-SE column `j` (0-indexed) as a slice.
    pub fn sex(&self, j: usize) -> &[f64] {
        &self.sebeta_xg[j]
    }

    /// Build the `n × p` exposure-effect matrix (column `j` = exposure `j+1`),
    /// row-major so that row `i` holds `[β_X1,…,β_Xp]` for instrument `i`.
    pub fn beta_xg_matrix(&self) -> Vec<Vec<f64>> {
        let n = self.n_snps();
        let p = self.n_exposures();
        let mut m = vec![vec![0.0; p]; n];
        for j in 0..p {
            for i in 0..n {
                m[i][j] = self.beta_xg[j][i];
            }
        }
        m
    }
    /// Build the `n × p` exposure-SE matrix.
    pub fn sebeta_xg_matrix(&self) -> Vec<Vec<f64>> {
        let n = self.n_snps();
        let p = self.n_exposures();
        let mut m = vec![vec![0.0; p]; n];
        for j in 0..p {
            for i in 0..n {
                m[i][j] = self.sebeta_xg[j][i];
            }
        }
        m
    }
}
