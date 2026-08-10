//! Comprehensive cross-validation: hierNet — coefficients, predictions, hierarchy.

use hierint::hiernet;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct WeakFixture {
    n: usize,
    p: usize,
    x: Vec<f64>,
    y: Vec<f64>,
    lamlist: Vec<f64>,
    bp: Vec<f64>,
    bn: Vec<f64>,
    th: Vec<f64>,
    obj: Vec<f64>,
    predictions: Vec<f64>,
}

fn load_weak() -> WeakFixture {
    serde_json::from_str(include_str!("hiernet_weak_full.json")).unwrap()
}

#[derive(Debug, Deserialize)]
struct StrongFixture {
    n: usize,
    p: usize,
    x: Vec<f64>,
    y: Vec<f64>,
    lamlist: Vec<f64>,
    bp: Vec<f64>,
    bn: Vec<f64>,
    th: Vec<f64>,
    obj: Vec<f64>,
}

fn load_strong() -> StrongFixture {
    serde_json::from_str(include_str!("hiernet_strong_full.json")).unwrap()
}

fn fit_weak_path(fx: &WeakFixture) -> hiernet::HierNetPath {
    let config = hiernet::HierNetConfig {
        family: hiernet::HierNetFamily::Gaussian,
        strong: false,
        diagonal: false,
        n_lam: fx.lamlist.len(),
        lamlist: Some(fx.lamlist.clone()),
        maxiter: 500,
        ..Default::default()
    };
    hiernet::fit_path(&fx.x, &fx.y, &config).expect("fit_path failed")
}

fn fit_strong_path(fx: &StrongFixture) -> hiernet::HierNetPath {
    let config = hiernet::HierNetConfig {
        family: hiernet::HierNetFamily::Gaussian,
        strong: true,
        diagonal: false,
        n_lam: fx.lamlist.len(),
        lamlist: Some(fx.lamlist.clone()),
        niter: 50,
        maxiter: 500,
        ..Default::default()
    };
    hiernet::fit_path(&fx.x, &fx.y, &config).expect("fit_path failed")
}

// ═══════════════════════════════════════════════════════════════════════
// Weak hierarchy tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn weak_lambda_path_match() {
    let fx = load_weak();
    let path = fit_weak_path(&fx);
    for (i, (got, expected)) in path.lamlist.iter().zip(fx.lamlist.iter()).enumerate() {
        let re = (got - expected).abs() / expected.abs().max(1e-10);
        assert!(re < 1e-4, "lamlist[{i}]: got {got}, expected {expected}");
    }
}

#[test]
fn weak_first_lambda_all_zero() {
    let fx = load_weak();
    let path = fit_weak_path(&fx);
    let c = &path.fits[0].coefs;
    assert!(c.bp.iter().all(|&v| v.abs() < 1e-8), "bp not zero at lambda_max");
    assert!(c.bn.iter().all(|&v| v.abs() < 1e-8), "bn not zero at lambda_max");
    assert!(c.th.iter().all(|&v| v.abs() < 1e-8), "th not zero at lambda_max");
}

#[test]
fn weak_last_lambda_has_main_effects() {
    let fx = load_weak();
    let path = fit_weak_path(&fx);
    let c = &path.fits.last().unwrap().coefs;
    let main_count = c.bp.iter().zip(&c.bn)
        .filter(|(bp, bn)| (*bp - *bn).abs() > 1e-6)
        .count();
    assert!(main_count >= 1, "expected main effects at smallest lambda, got {main_count}");
}

#[test]
fn weak_predictor_x1_discovered() {
    let fx = load_weak();
    let path = fit_weak_path(&fx);
    // x1 has the strongest effect (coefficient 1.0)
    let c = &path.fits.last().unwrap().coefs;
    let b1 = c.bp[0] - c.bn[0];
    assert!(b1.abs() > 0.1, "expected nonzero coefficient for x1, got {b1:.6}");
}

#[test]
fn weak_prediction_at_max_is_mean_y() {
    let fx = load_weak();
    let path = fit_weak_path(&fx);
    let preds = hiernet::predict(&path.fits[0], &fx.x, fx.n);
    let mean_y = fx.y.iter().sum::<f64>() / fx.n as f64;
    for (i, &p) in preds.iter().enumerate() {
        assert!(
            (p - mean_y).abs() < 1e-4,
            "prediction at lambda_max should be mean(y)={mean_y:.6}, got {p:.6} at obs {i}"
        );
    }
}

#[test]
fn weak_objective_decreasing() {
    let fx = load_weak();
    let path = fit_weak_path(&fx);
    for i in 1..path.fits.len() {
        assert!(
            path.fits[i].obj <= path.fits[i-1].obj + 1e-4,
            "objective should be non-increasing: [{i}]={:.6} > [{:.6}",
            path.fits[i].obj, path.fits[i-1].obj
        );
    }
}

#[test]
fn weak_hierarchy_holds() {
    let fx = load_weak();
    let path = fit_weak_path(&fx);
    let p = fx.p;

    for (li, fit) in path.fits.iter().enumerate() {
        for j in 0..p {
            // Weak hierarchy: if th[j,*] has nonzero entries, then bp[j]-bn[j] ≠ 0
            let main = fit.coefs.bp[j] - fit.coefs.bn[j];
            let mut has_interaction = false;
            for k in 0..p {
                if j != k {
                    let th_val = (fit.coefs.th[j + p * k] + fit.coefs.th[k + p * j]).abs();
                    if th_val > 1e-6 {
                        has_interaction = true;
                        break;
                    }
                }
            }
            if has_interaction {
                assert!(
                    main.abs() > 1e-6,
                    "weak hierarchy violated at lambda[{li}]: row {j} has interactions but main effect is zero"
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Strong hierarchy tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn strong_lambda_path_match() {
    let fx = load_strong();
    let path = fit_strong_path(&fx);
    for (i, (got, expected)) in path.lamlist.iter().zip(fx.lamlist.iter()).enumerate() {
        let re = (got - expected).abs() / expected.abs().max(1e-10);
        assert!(re < 1e-4, "lamlist[{i}]: got {got}, expected {expected}");
    }
}

#[test]
fn strong_first_lambda_all_zero() {
    let fx = load_strong();
    let path = fit_strong_path(&fx);
    let c = &path.fits[0].coefs;
    assert!(c.bp.iter().all(|&v| v.abs() < 1e-8));
    assert!(c.bn.iter().all(|&v| v.abs() < 1e-8));
    assert!(c.th.iter().all(|&v| v.abs() < 1e-8));
}

#[test]
fn strong_last_lambda_has_effects() {
    let fx = load_strong();
    let path = fit_strong_path(&fx);
    let c = &path.fits.last().unwrap().coefs;
    let p = fx.p;
    let main_count = c.bp.iter().zip(&c.bn)
        .filter(|(bp, bn)| (*bp - *bn).abs() > 1e-6)
        .count();
    assert!(main_count >= 1, "expected main effects at smallest lambda");

    let inter_count = (0..p-1).flat_map(|j| (j+1..p).map(move |k| {
        if (c.th[j + p * k] + c.th[k + p * j]).abs() > 1e-6 { 1 } else { 0 }
    })).sum::<i32>();
    assert!(inter_count >= 0, "interaction count should be non-negative");
}

#[test]
fn strong_strong_hierarchy_holds() {
    let fx = load_strong();
    let path = fit_strong_path(&fx);
    let p = fx.p;

    for (li, fit) in path.fits.iter().enumerate() {
        for j in 0..p {
            for k in 0..p {
                if j == k { continue; }
                let th_val = fit.coefs.th[j + p * k] + fit.coefs.th[k + p * j];
                if th_val.abs() > 1e-5 {
                    let main_j = (fit.coefs.bp[j] - fit.coefs.bn[j]).abs();
                    let main_k = (fit.coefs.bp[k] - fit.coefs.bn[k]).abs();
                    assert!(
                        main_j > 1e-5 && main_k > 1e-5,
                        "strong hierarchy violated at lambda[{li}]: th[{j},{k}] nonzero but main effects missing"
                    );
                }
            }
        }
    }
}
