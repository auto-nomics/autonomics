//! Cross-validation of `hypothesize` trinity + combinators against R's
//! `hypothesize` package (v1.0.0) and `stats::`.
//!
//! To regenerate the reference values:
//!
//! ```bash
//! Rscript -e 'library(hypothesize); ...'   # see individual test comments
//! ```

use hypothesize::combine::*;
use hypothesize::*;

// ─── Trinity: Wald / LRT / Score ─────────────────────────────────────────────

#[test]
fn wald_univariate_matches_r_hypothesize() {
    // R: hypothesize::wald_test(estimate=2.5, se=0.8)
    //   $stat = 9.765625, $p.value = pchisq(9.765625, 1, lower.tail=FALSE)
    let w = wald_uni(2.5, 0.8, 0.0).unwrap();
    let z = 2.5_f64 / 0.8;
    let expected_stat = z * z;
    let expected_p = chisq_sf(expected_stat, 1.0);
    assert!((w.stat - expected_stat).abs() < 1e-12);
    assert!((w.p_value - expected_p).abs() < 1e-12);
    assert!((w.extra_f64("z").unwrap() - z).abs() < 1e-12);
}

#[test]
fn wald_multivariate_matches_manual() {
    // R: wald_test(estimate=c(2,3),
    //              vcov=matrix(c(1,.3,.3,1),2,2), null_value=c(0,0))
    let est = vec![2.0, 3.0];
    let vcov = vec![vec![1.0, 0.3], vec![0.3, 1.0]];
    let w = wald_multi(&est, &vcov, &[0.0, 0.0]).unwrap();
    // diff' Σ⁻¹ diff where Σ⁻¹ = (1/0.91)[[1,-0.3],[-0.3,1]]
    // = (1/0.91)(2·1.1 + 3·2.4) = (1/0.91)(2.2+7.2) = 9.4/0.91 ≈ 10.3297
    assert!((w.stat - 9.4 / 0.91).abs() < 1e-9);
    assert!((w.p_value - chisq_sf(w.stat, 2.0)).abs() < 1e-12);
}

#[test]
fn lrt_matches_r_hypothesize() {
    // R: hypothesize::lrt(null_loglik=-150, alt_loglik=-140, dof=3)
    //   $stat = 20, $p.value = pchisq(20, 3, lower.tail=FALSE) ≈ 0.0001687
    let t = lrt(-150.0, -140.0, Some(3)).unwrap();
    assert!((t.stat - 20.0).abs() < 1e-12);
    assert!((t.p_value - chisq_sf(20.0, 3.0)).abs() < 1e-12);

    // With LogLik objects: auto-dof.
    let t2 = lrt(LogLik::new(-150.0, 2), LogLik::new(-140.0, 5), None).unwrap();
    assert_eq!(t2.dof, 3.0);
    assert!((t2.stat - 20.0).abs() < 1e-12);
}

#[test]
fn score_test_matches_r_hypothesize() {
    // R: hypothesize::score_test(score=2, fisher_info=2)
    //   $stat = 4/2 = 2, $p.value = pchisq(2, 1, lower.tail=FALSE)
    let t = score_uni(2.0, 2.0, Some(0.0)).unwrap();
    assert!((t.stat - 2.0).abs() < 1e-12);
    assert!((t.p_value - chisq_sf(2.0, 1.0)).abs() < 1e-12);

    // Multivariate: score=c(1,2), I=diag(1,1) → S=1+4=5
    let t2 = score_multi(&[1.0, 2.0], &vec![vec![1.0, 0.0], vec![0.0, 1.0]], None).unwrap();
    assert!((t2.stat - 5.0).abs() < 1e-12);
}

// ─── z-test ──────────────────────────────────────────────────────────────────

#[test]
fn z_test_matches_r_hypothesize() {
    // R: hypothesize::z_test(c(980,1020,950,...), mu0=1000, sigma=100)
    // With the R package's example data (50 bulbs, mean=1000, sigma=100):
    // z = (xbar - 1000) / (100/sqrt(50))
    let lifetimes: Vec<f64> = vec![
        980.0, 1020.0, 950.0, 1010.0, 990.0, 1005.0, 970.0, 1030.0, 985.0, 995.0, 1000.0, 1015.0,
        960.0, 1025.0, 975.0, 1008.0, 992.0, 1012.0, 988.0, 1002.0, 978.0, 1018.0, 965.0, 1022.0,
        982.0, 1005.0, 995.0, 1010.0, 972.0, 1028.0, 990.0, 1000.0, 985.0, 1015.0, 968.0, 1020.0,
        980.0, 1008.0, 992.0, 1012.0, 975.0, 1018.0, 962.0, 1025.0, 985.0, 1002.0, 988.0, 1010.0,
        978.0, 1020.0,
    ];
    let t = z_test(&lifetimes, 1000.0, 100.0, Alternative::TwoSided).unwrap();
    let xbar: f64 = lifetimes.iter().sum::<f64>() / 50.0;
    let z = (xbar - 1000.0) / (100.0 / 50.0_f64.sqrt());
    assert!((t.stat - z).abs() < 1e-9);
    assert!((t.p_value - 2.0 * normal_cdf(-z.abs())).abs() < 1e-9);
}

// ─── Combinators ─────────────────────────────────────────────────────────────

#[test]
fn fisher_combine_matches_r_hypothesize() {
    // R: hypothesize::fisher_combine(0.08, 0.12, 0.04)
    let t = fisher_combine_pvals(&[0.08, 0.12, 0.04]).unwrap();
    let expected_stat = -2.0 * (0.08_f64).ln() - 2.0 * (0.12_f64).ln() - 2.0 * (0.04_f64).ln();
    assert!((t.stat - expected_stat).abs() < 1e-9);
    assert!((t.p_value - chisq_sf(expected_stat, 6.0)).abs() < 1e-9);
    assert_eq!(t.dof, 6.0);
}

#[test]
fn boolean_algebra_matches_r_hypothesize() {
    // R: intersection_test(0.01, 0.03, 0.04) → p = max(p) = 0.04
    let t = intersection_test(&[0.01_f64, 0.03, 0.04]).unwrap();
    assert!((t.p_value - 0.04).abs() < 1e-12);

    // R: union_test(0.01, 0.80) → p = min(p) = 0.01
    let t2 = union_test(&[0.01_f64, 0.80]).unwrap();
    assert!((t2.p_value - 0.01).abs() < 1e-12);

    // R: complement_test(wald_test(estimate=3.0, se=1.0))
    //   → p = 1 - original_p
    let w = wald_uni(3.0, 1.0, 0.0).unwrap();
    let orig_p = w.p_value;
    let c = complement_test(&w);
    assert!((c.p_value - (1.0 - orig_p)).abs() < 1e-12);

    // Double complement recovers original.
    let cc = complement_test(&c);
    assert!((cc.p_value - orig_p).abs() < 1e-9);
}

#[test]
fn de_morgan_law_holds() {
    // union(a, b) == complement(intersection(complement(a), complement(b)))
    let a = wald_uni(2.0, 1.0, 0.0).unwrap();
    let b = wald_uni(1.5, 0.8, 0.0).unwrap();

    let or_ab = union_test(&[a.clone(), b.clone()]).unwrap();
    let na = complement_test(&a);
    let nb = complement_test(&b);
    let and_neg = intersection_test(&[na, nb]).unwrap();
    let de_morgan = complement_test(&and_neg);

    assert!((or_ab.p_value - de_morgan.p_value).abs() < 1e-9);
}

// ─── Invert / CI duality ─────────────────────────────────────────────────────

#[test]
fn invert_wald_matches_analytical_confint() {
    let est = 2.5_f64;
    let se = 0.8_f64;
    let grid: Vec<f64> = (0..=10_000).map(|i| i as f64 * 0.0005).collect();
    let cs = invert_test(|theta| wald_uni(est, se, theta).unwrap(), &grid, 0.05);

    let w = wald_uni(est, se, 0.0).unwrap();
    let (lo, hi) = confint_wald(&w, 0.95).unwrap();

    // Grid search should agree with analytical to within grid resolution.
    assert!(
        (cs.lower() - lo).abs() < 0.001,
        "lower: grid {} vs analytical {}",
        cs.lower(),
        lo
    );
    assert!(
        (cs.upper() - hi).abs() < 0.001,
        "upper: grid {} vs analytical {}",
        cs.upper(),
        hi
    );
}
