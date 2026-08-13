//! Simulate GWAS summary statistics — port of `R/simLDSC.R`.

use faer::Mat;
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use crate::error::Result;
use crate::linalg;

/// Configuration for `simLDSC`.
#[derive(Clone, Debug)]
pub struct SimLdscConfig {
    pub covmat: Mat<f64>,
    pub n: Mat<f64>,
    pub r_pheno: Mat<f64>,
    pub intercepts: Vec<f64>,
    pub ld_scores: Vec<f64>,
    pub n_replications: usize,
    pub seed: u64,
}

/// Simulated summary statistics for one trait.
#[derive(Clone, Debug)]
pub struct SimSumstats {
    pub snp: Vec<String>,
    pub n: Vec<f64>,
    pub z: Vec<f64>,
    pub a1: Vec<String>,
}

/// Simulate GWAS summary statistics for testing.
pub fn sim_ldsc(config: &SimLdscConfig) -> Result<Vec<SimSumstats>> {
    let k = config.covmat.nrows();
    let n_snps = config.ld_scores.len();
    let mut rng = ChaCha8Rng::seed_from_u64(config.seed);

    let mut all_traits = Vec::new();

    for _rep in 0..config.n_replications {
        let mut trait_sumstats = Vec::new();

        for t in 0..k {
            let n_val = config.n[(t, t)];
            let mut snp_list = Vec::new();
            let mut n_list = Vec::new();
            let mut z_list = Vec::new();
            let mut a1_list = Vec::new();

            for i in 0..n_snps {
                let ld = config.ld_scores[i];
                // Simulate chi-squared from genetic + noise components
                let h2 = config.covmat[(t, t)];
                let normal = rand_distr::Normal::<f64>::new(0.0, 1.0).unwrap();
                let noise: f64 = rng.sample(normal).abs();
                let chi2: f64 = n_val * h2 * ld / config.covmat.nrows() as f64
                    + config.intercepts.get(t).copied().unwrap_or(1.0)
                    + noise * 0.1;

                let z = if chi2 > 0.0 {
                    let sign: f64 = if rng.random_bool(0.5) { 1.0 } else { -1.0 };
                    sign * chi2.sqrt()
                } else {
                    0.0
                };

                snp_list.push(format!("rs{}", i + 1));
                n_list.push(n_val);
                z_list.push(z);
                a1_list.push(if rng.random_bool(0.5) {
                    "A".into()
                } else {
                    "G".into()
                });
            }

            trait_sumstats.push(SimSumstats {
                snp: snp_list,
                n: n_list,
                z: z_list,
                a1: a1_list,
            });
        }

        all_traits.extend(trait_sumstats);
    }

    Ok(all_traits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sim_ldsc_basic() {
        let k = 3;
        let config = SimLdscConfig {
            covmat: Mat::<f64>::identity(k, k) * 0.3,
            n: Mat::<f64>::identity(k, k) * 100_000.0,
            r_pheno: Mat::<f64>::identity(k, k),
            intercepts: vec![1.0; k],
            ld_scores: vec![1.0, 2.0, 3.0],
            n_replications: 1,
            seed: 42,
        };
        let results = sim_ldsc(&config).unwrap();
        assert_eq!(results.len(), k);
        assert_eq!(results[0].snp.len(), 3);
    }
}
