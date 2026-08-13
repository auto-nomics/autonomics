//! Parallel analysis based on multivariate LDSC — port of `R/paLDSC.R`.
//!
//! Horn's parallel analysis adapted for LDSC-derived genetic covariance matrices.

use faer::Mat;
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use crate::error::Result;
use crate::linalg;

/// Configuration for `paLDSC`.
#[derive(Clone, Debug)]
pub struct PaLdscConfig {
    pub n_replications: usize,
    pub percentile: f64,
    pub diagonal: bool,
}

impl Default for PaLdscConfig {
    fn default() -> Self {
        Self {
            n_replications: 500,
            percentile: 0.95,
            diagonal: false,
        }
    }
}

/// Result of parallel analysis.
#[derive(Clone, Debug)]
pub struct PaLdscResult {
    /// Observed eigenvalues (descending).
    pub observed_eigenvalues: Vec<f64>,
    /// Percentile thresholds from simulated null.
    pub threshold_eigenvalues: Vec<f64>,
    /// Suggested number of factors.
    pub n_factors: usize,
}

/// Run parallel analysis.
///
/// Compares eigenvalues of the observed S matrix against those from
/// random matrices simulated under the LDSC sampling distribution.
pub fn pa_ldsc(s: &Mat<f64>, v: &Mat<f64>, config: &PaLdscConfig) -> Result<PaLdscResult> {
    let k = s.nrows();
    let mut rng = ChaCha8Rng::seed_from_u64(1234);

    // Observed eigenvalues
    let (obs_eigvals, _) = linalg::eigen_sym(s);

    // Simulate null eigenvalues
    let mut sim_eigvals = vec![vec![0.0f64; k]; config.n_replications];

    for rep in 0..config.n_replications {
        // Sample a random matrix from the LDSC sampling distribution
        // Under null: S_null ~ N(S, V) approximately
        let s_vec = linalg::vech(s);
        let z = s_vec.len();

        // Generate random vech from N(S_vec, V)
        let mut sim_vec = vec![0.0f64; z];
        for i in 0..z {
            let noise = rng.sample(rand_distr::Normal::new(0.0, 1.0).unwrap());
            sim_vec[i] = s_vec[i];
            // Add correlated noise using V
            let sd = v[(i, i)].max(0.0).sqrt();
            sim_vec[i] += noise * sd;
        }

        // Reconstruct symmetric matrix from vech
        let sim_mat = linalg::vech_inv(&sim_vec);

        // Eigenvalues
        let (sim_eig, _) = linalg::eigen_sym(&sim_mat);
        sim_eigvals[rep] = sim_eig;
    }

    // Compute percentile thresholds
    let mut thresholds = vec![0.0f64; k];
    for j in 0..k {
        let mut col: Vec<f64> = sim_eigvals.iter().map(|r| r[j]).collect();
        col.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((config.percentile * col.len() as f64) as usize).min(col.len() - 1);
        thresholds[j] = col[idx];
    }

    // Count how many observed eigenvalues exceed thresholds
    let n_factors = obs_eigvals
        .iter()
        .zip(thresholds.iter())
        .take_while(|(obs, thresh)| obs > thresh)
        .count();

    Ok(PaLdscResult {
        observed_eigenvalues: obs_eigvals,
        threshold_eigenvalues: thresholds,
        n_factors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pa_ldsc_basic() {
        let mut s = Mat::zeros(3, 3);
        s[(0, 0)] = 0.5;
        s[(1, 1)] = 0.3;
        s[(2, 2)] = 0.1;
        s[(0, 1)] = 0.2;
        s[(0, 2)] = 0.05;
        s[(1, 2)] = 0.02;
        s[(1, 0)] = 0.2;
        s[(2, 0)] = 0.05;
        s[(2, 1)] = 0.02;

        let z = 6;
        let v = Mat::<f64>::identity(z, z) * 0.001;

        let config = PaLdscConfig {
            n_replications: 50,
            ..Default::default()
        };
        let result = pa_ldsc(&s, &v, &config).unwrap();
        assert_eq!(result.observed_eigenvalues.len(), 3);
        assert_eq!(result.threshold_eigenvalues.len(), 3);
    }
}
