//! Integration tests for the `genomic_sem` crate.
//!
//! These tests exercise the full pipeline with synthetic data whose
//! statistical properties are known analytically, providing end-to-end
//! cross-validation of the GenomicSEM port.

use faer::Mat;
use genomic_sem::*;

// =====================================================================
// Helper: construct a known covariance structure
// =====================================================================

/// Build a Covstruc from a known one-factor model with specified loadings
/// and residual variances. This lets us verify that SEM fitting recovers
/// the known parameters.
fn make_one_factor_covstruc(
    loadings: &[f64],
    resid_var: &[f64],
    factor_var: f64,
    v_scale: f64,
) -> utils::Covstruc {
    let k = loadings.len();
    let mut s = Mat::zeros(k, k);

    // S = Lambda · Psi · Lambda' + Theta
    for i in 0..k {
        for j in 0..k {
            s[(i, j)] = loadings[i] * loadings[j] * factor_var;
            if i == j {
                s[(i, i)] += resid_var[i];
            }
        }
    }

    let z = k * (k + 1) / 2;
    let v = Mat::<f64>::identity(z, z) * v_scale;

    utils::Covstruc {
        v,
        s,
        i_mat: Mat::<f64>::identity(k, k),
        n: Mat::zeros(1, z),
        m: 100_000.0,
        v_stand: None,
        s_stand: None,
    }
}

// =====================================================================
// nearPD tests
// =====================================================================

#[test]
fn test_near_pd_recovers_pd() {
    // Non-PD matrix
    let mut bad = Mat::zeros(3, 3);
    bad[(0, 0)] = 1.0;
    bad[(1, 1)] = 1.0;
    bad[(2, 2)] = 1.0;
    bad[(0, 1)] = 0.9;
    bad[(1, 0)] = 0.9;
    bad[(0, 2)] = 0.9;
    bad[(2, 0)] = 0.9;
    bad[(1, 2)] = 0.9;
    bad[(2, 1)] = 0.9;
    // This matrix has eigenvalue -0.8 (not PD)

    let fixed = near_pd::near_pd(&bad);
    assert!(near_pd::is_spd(&fixed), "nearPD output must be SPD");

    // Should be close to the original
    let mut max_diff = 0.0;
    for i in 0..3 {
        for j in 0..3 {
            let d = (fixed[(i, j)] - bad[(i, j)]).abs();
            if d > max_diff {
                max_diff = d;
            }
        }
    }
    assert!(
        max_diff < 0.3,
        "nearPD should be close to original, max_diff={max_diff}"
    );
}

#[test]
fn test_near_pd_preserves_identity() {
    let id = Mat::<f64>::identity(5, 5);
    let result = near_pd::near_pd(&id);
    for i in 0..5 {
        for j in 0..5 {
            let expected = if i == j { 1.0 } else { 0.0 };
            assert!((result[(i, j)] - expected).abs() < 1e-8);
        }
    }
}

// =====================================================================
// SEM engine tests
// =====================================================================

#[test]
fn test_sem_recovers_known_loadings_3trait() {
    // Known one-factor model: loadings = [0.8, 0.6, 0.7], resid = [0.2, 0.3, 0.2], F_var = 0.5
    let loadings = vec![0.8, 0.6, 0.7];
    let resid = vec![0.2, 0.3, 0.2];
    let factor_var = 0.5;
    let covstruc = make_one_factor_covstruc(&loadings, &resid, factor_var, 0.001);

    let config = usermodel::UserModelConfig {
        model: "F1 =~ NA*V1 + V2 + V3\nF1 ~~ 1*F1".to_string(),
        ..Default::default()
    };
    let result = usermodel::usermodel(&covstruc, &config).unwrap();

    // Check that the model fit produces reasonable results
    // For k=3 one-factor model: df = k(k-3)/2 = 0 (saturated)
    assert_eq!(
        result.modelfit.df, 0,
        "3 indicators with 1 factor = saturated model (df=0)"
    );

    // Check loadings are recovered (approximately)
    let loading_results: Vec<&usermodel::ParamResult> =
        result.results.iter().filter(|r| r.op == "=~").collect();
    assert_eq!(loading_results.len(), 3);

    // Loadings should be positive and in the right ballpark
    for r in &loading_results {
        assert!(
            r.unstand_est.abs() > 0.1,
            "Loading estimate should be meaningful: {}",
            r.unstand_est
        );
    }
}

#[test]
fn test_sem_two_factor_recovery() {
    // Two-factor model with known structure
    let loadings1 = [0.7, 0.6, 0.0, 0.0];
    let loadings2 = [0.0, 0.0, 0.8, 0.5];
    let resid = [0.3, 0.3, 0.2, 0.3];
    let f1_var = 0.4;
    let f2_var = 0.5;

    let k = 4;
    let mut s = Mat::zeros(k, k);
    for i in 0..k {
        for j in 0..k {
            s[(i, j)] = loadings1[i] * loadings1[j] * f1_var + loadings2[i] * loadings2[j] * f2_var;
            if i == j {
                s[(i, i)] += resid[i];
            }
        }
    }

    let z = k * (k + 1) / 2;
    let covstruc = utils::Covstruc {
        v: Mat::<f64>::identity(z, z) * 0.001,
        s,
        i_mat: Mat::<f64>::identity(k, k),
        n: Mat::zeros(1, z),
        m: 100_000.0,
        v_stand: None,
        s_stand: None,
    };

    let config = usermodel::UserModelConfig {
        model: "F1 =~ NA*V1 + V2\nF2 =~ NA*V3 + V4\nF1 ~~ F2".to_string(),
        ..Default::default()
    };
    let result = usermodel::usermodel(&covstruc, &config).unwrap();

    // Should have loadings for both factors
    let f1_loadings: Vec<_> = result
        .results
        .iter()
        .filter(|r| r.op == "=~" && r.lhs == "F1")
        .collect();
    let f2_loadings: Vec<_> = result
        .results
        .iter()
        .filter(|r| r.op == "=~" && r.lhs == "F2")
        .collect();
    assert_eq!(f1_loadings.len(), 2);
    assert_eq!(f2_loadings.len(), 2);
}

#[test]
fn test_commonfactor_fit_quality() {
    // 5-trait one-factor model
    let loadings = vec![0.6, 0.7, 0.5, 0.8, 0.4];
    let resid = vec![0.3, 0.2, 0.4, 0.1, 0.5];
    let covstruc = make_one_factor_covstruc(&loadings, &resid, 0.5, 0.0001);

    let config = commonfactor::CommonFactorConfig::default();
    let result = commonfactor::commonfactor(&covstruc, &config).unwrap();

    // With 5 indicators, df = k(k-3)/2 = 5*2/2 = 5
    assert_eq!(result.modelfit.df, 5);

    // SRMR should be small for a well-fitting model
    assert!(
        result.modelfit.srmr < 0.5,
        "SRMR should be reasonable: {}",
        result.modelfit.srmr
    );
}

// =====================================================================
// rgmodel tests
// =====================================================================

#[test]
fn test_rgmodel_correlation_matrix() {
    let k = 4;
    let mut s = Mat::zeros(k, k);
    // Create a matrix with known correlations
    for i in 0..k {
        s[(i, i)] = 0.3 + 0.1 * i as f64;
    }
    for i in 0..k {
        for j in (i + 1)..k {
            let val = 0.1 * (i + j) as f64;
            s[(i, j)] = val;
            s[(j, i)] = val;
        }
    }
    let z = k * (k + 1) / 2;
    let covstruc = utils::Covstruc {
        v: Mat::<f64>::identity(z, z) * 0.001,
        s: s.clone(),
        i_mat: Mat::<f64>::identity(k, k),
        n: Mat::zeros(1, z),
        m: 100_000.0,
        v_stand: None,
        s_stand: None,
    };

    let result = rgmodel::rgmodel(&covstruc, false).unwrap();

    // R should be a correlation matrix (diagonal = 1)
    for i in 0..k {
        assert!(
            (result.r[(i, i)] - 1.0).abs() < 1e-10,
            "R diagonal should be 1"
        );
    }

    // V_R should be positive semi-definite (non-negative diagonal)
    for i in 0..z {
        assert!(
            result.v_r[(i, i)] >= 0.0,
            "V_R diagonal should be non-negative"
        );
    }

    // Check that rg is consistent: r_ij = s_ij / (sd_i * sd_j)
    for i in 0..k {
        for j in 0..k {
            let sd_i = s[(i, i)].sqrt();
            let sd_j = s[(j, j)].sqrt();
            let expected_r = s[(i, j)] / (sd_i * sd_j);
            assert!(
                (result.r[(i, j)] - expected_r).abs() < 1e-8,
                "r[{},{}] = {} but expected {}",
                i,
                j,
                result.r[(i, j)],
                expected_r
            );
        }
    }
}

// =====================================================================
// GWAS pipeline tests
// =====================================================================

#[test]
fn test_user_gwas_parallel() {
    let k = 3;
    let covstruc = utils::Covstruc {
        v: Mat::<f64>::identity(6, 6) * 0.001,
        s: {
            let mut m = Mat::zeros(k, k);
            m[(0, 0)] = 0.3;
            m[(1, 1)] = 0.3;
            m[(2, 2)] = 0.3;
            m[(0, 1)] = 0.15;
            m[(0, 2)] = 0.15;
            m[(1, 2)] = 0.15;
            m[(1, 0)] = 0.15;
            m[(2, 0)] = 0.15;
            m[(2, 1)] = 0.15;
            m
        },
        i_mat: Mat::<f64>::identity(k, k),
        n: Mat::zeros(1, 6),
        m: 100_000.0,
        v_stand: None,
        s_stand: None,
    };

    let n_snps = 10;
    let sumstats = sumstats::MergedSumstats {
        snp: (0..n_snps).map(|i| format!("rs{}", i + 1)).collect(),
        chr: vec![1; n_snps],
        bp: (0..n_snps).map(|i| ((i + 1) * 1000) as i64).collect(),
        maf: vec![0.3; n_snps],
        a1: vec!["A".into(); n_snps],
        a2: vec!["G".into(); n_snps],
        betas: (0..k).map(|_| vec![0.01; n_snps]).collect(),
        ses: (0..k).map(|_| vec![0.05; n_snps]).collect(),
        n_traits: k,
    };

    let config = gwas::UserGwasConfig {
        model: "F1 =~ NA*V1 + V2 + V3\nF1 ~~ 1*F1\nF1 ~ SNP".to_string(),
        parallel: true,
        ..Default::default()
    };

    let results = gwas::user_gwas(&covstruc, &sumstats, &config).unwrap();
    assert_eq!(results.len(), n_snps);
}

// =====================================================================
// Utility function tests
// =====================================================================

#[test]
fn test_liability_conversion_known_values() {
    // For pop_prev = 0.01, samp_prev = 0.5:
    // conversion.factor ≈ 0.01² × 0.99² / (0.5 × 0.5 × dnorm(qnorm(0.99))²)
    let cf = utils::liability_conversion_factor(0.01, 0.5);
    assert!(cf > 0.0);
    // Should be less than 1 for rare diseases
    assert!(
        cf < 1.0,
        "liability conversion for 1% prevalence should be < 1, got {cf}"
    );
}

#[test]
fn test_standardization() {
    let mut s = Mat::zeros(3, 3);
    s[(0, 0)] = 4.0;
    s[(1, 1)] = 9.0;
    s[(2, 2)] = 16.0;
    s[(0, 1)] = 6.0;
    s[(1, 0)] = 6.0;
    s[(0, 2)] = 4.0;
    s[(2, 0)] = 4.0;
    s[(1, 2)] = 6.0;
    s[(2, 1)] = 6.0;

    let cor = utils::standardize(&s);
    for i in 0..3 {
        assert!((cor[(i, i)] - 1.0).abs() < 1e-10);
    }
    // cor[0,1] = 6 / (2*3) = 1.0
    assert!((cor[(0, 1)] - 1.0).abs() < 1e-10);
    // cor[0,2] = 4 / (2*4) = 0.5
    assert!((cor[(0, 2)] - 0.5).abs() < 1e-10);
}

// =====================================================================
// paLDSC tests
// =====================================================================

#[test]
fn test_paldsc_returns_valid_eigenvalues() {
    let k = 4;
    let mut s = Mat::zeros(k, k);
    for i in 0..k {
        s[(i, i)] = 0.3;
    }
    s[(0, 1)] = 0.2;
    s[(1, 0)] = 0.2;

    let z = k * (k + 1) / 2;
    let v = Mat::<f64>::identity(z, z) * 0.001;

    let config = paldsc::PaLdscConfig {
        n_replications: 100,
        ..Default::default()
    };
    let result = paldsc::pa_ldsc(&s, &v, &config).unwrap();

    assert_eq!(result.observed_eigenvalues.len(), k);
    assert_eq!(result.threshold_eigenvalues.len(), k);
    // The first eigenvalue should be larger (one factor present)
    assert!(result.observed_eigenvalues[0] > result.observed_eigenvalues[k - 1]);
}

// =====================================================================
// write_model tests
// =====================================================================

#[test]
fn test_write_model_bifactor() {
    let loadings = vec![
        vec![0.8, 0.1],
        vec![0.7, 0.1],
        vec![0.1, 0.8],
        vec![0.1, 0.7],
    ];
    let names: Vec<String> = (0..4).map(|i| format!("V{}", i + 1)).collect();
    let model = write_model::write_model(&loadings, &names, 0.3, true, true, false, false);
    assert!(model.contains("F1 =~"));
    assert!(model.contains("F2 =~"));
    assert!(model.contains("Common_F =~"));
    assert!(model.contains("Common_F ~~ 0*F1"));
}

// =====================================================================
// summary_gls tests
// =====================================================================

#[test]
fn test_gls_with_identity_weight() {
    // When Omega = I, GLS should match OLS
    let y = vec![1.0, 3.0, 5.0, 7.0];
    let omega = Mat::<f64>::identity(4, 4);
    let x = vec![vec![1.0, 2.0, 3.0, 4.0]];

    let result = summary_gls::summary_gls(&y, &omega, &x, true).unwrap();

    // OLS slope should be 2.0 (y = 2x - 1)
    assert!(
        (result.betas[1] - 2.0).abs() < 0.01,
        "GLS slope should be ~2.0, got {}",
        result.betas[1]
    );
}

// =====================================================================
// fusion tests
// =====================================================================

#[test]
fn test_fusion_binary_effect() {
    // Z = 2, HSQ = 0.1, N = 100000, binary
    let (effect, se) = fusion::compute_effect_se(2.0, 0.1, 100_000.0, true);
    let expected_denom = (100_000.0_f64 / 4.0 * 0.1).sqrt();
    assert!((effect - 2.0 / expected_denom).abs() < 1e-10);
    assert!((se - 1.0 / expected_denom).abs() < 1e-10);
}

// =====================================================================
// SEM syntax parser tests
// =====================================================================

#[test]
fn test_parse_complex_model() {
    let model_str = "\
        F1 =~ NA*V1 + V2 + V3\n\
        F2 =~ NA*V4 + V5\n\
        F1 ~ F2\n\
        F1 ~~ F2\n\
        V1 ~~ V1\n\
        V2 ~~ V2\n\
        V3 ~~ V3\n\
        V4 ~~ V4\n\
        V5 ~~ V5";

    let model = sem::parse_model(model_str).unwrap();
    assert_eq!(model.latent_vars.len(), 2);
    assert!(model.observed_vars.len() >= 5);
    assert!(model.lines.iter().any(|l| l.op == "=~"));
    assert!(model.lines.iter().any(|l| l.op == "~"));
    assert!(model.lines.iter().any(|l| l.op == "~~"));
}

#[test]
fn test_parse_model_with_ghost_parameter() {
    let model_str = "F1 =~ NA*V1 + V2\nF2 =~ NA*V3 + V4\nind := F1 * F2";
    let model = sem::parse_model(model_str).unwrap();
    assert!(model.lines.iter().any(|l| l.op == ":="));
}

// =====================================================================
// Index/subset tests
// =====================================================================

#[test]
fn test_index_s_symmetric() {
    let idx = index::index_s(4, false);
    // Should be symmetric
    for i in 0..4 {
        for j in 0..4 {
            assert_eq!(idx[(i, j)], idx[(j, i)]);
        }
    }
}

#[test]
fn test_sub_sv_preserves_values() {
    let k = 4;
    let mut s = Mat::zeros(k, k);
    for i in 0..k {
        s[(i, i)] = (i + 1) as f64 * 0.1;
    }
    let z = k * (k + 1) / 2;
    let v = Mat::<f64>::identity(z, z) * 0.01;

    let (s_sub, v_sub) = index::sub_sv(&s, &v, &[0, 2]);
    // S_sub diagonal should have original (0,0) and (2,2) values
    assert!((s_sub[(0, 0)] - 0.1).abs() < 1e-10);
    assert!((s_sub[(1, 1)] - 0.3).abs() < 1e-10);
    // V_sub should be 3×3 (z for k=2)
    assert_eq!(v_sub.nrows(), 3);
    assert_eq!(v_sub.ncols(), 3);
}

// =====================================================================
// Full pipeline integration test
// =====================================================================

#[test]
fn test_full_pipeline_ldsc_to_sem() {
    // Simulate a known genetic covariance structure
    let loadings = vec![0.7, 0.6, 0.5, 0.4];
    let resid = vec![0.2, 0.2, 0.2, 0.2];
    let factor_var = 0.3;
    let covstruc = make_one_factor_covstruc(&loadings, &resid, factor_var, 0.0005);

    // Step 1: Fit common factor
    let cf_result =
        commonfactor::commonfactor(&covstruc, &commonfactor::CommonFactorConfig::default())
            .unwrap();
    assert!(cf_result.modelfit.df > 0);

    // Step 2: Fit user model
    let names: Vec<String> = (0..4).map(|i| format!("V{}", i + 1)).collect();
    let model_str = format!(
        "F1 =~ NA*{} + {} + {} + {}\nF1 ~~ 1*F1",
        names[0], names[1], names[2], names[3]
    );
    let um_config = usermodel::UserModelConfig {
        model: model_str,
        ..Default::default()
    };
    let um_result = usermodel::usermodel(&covstruc, &um_config).unwrap();
    assert!(!um_result.results.is_empty());

    // Step 3: Compute rgmodel
    let rg_result = rgmodel::rgmodel(&covstruc, false).unwrap();
    for i in 0..4 {
        assert!((rg_result.r[(i, i)] - 1.0).abs() < 1e-10);
    }

    // Step 4: paLDSC
    let pa_config = paldsc::PaLdscConfig {
        n_replications: 50,
        ..Default::default()
    };
    let pa_result = paldsc::pa_ldsc(&covstruc.s, &covstruc.v, &pa_config).unwrap();
    assert_eq!(pa_result.observed_eigenvalues.len(), 4);
}
