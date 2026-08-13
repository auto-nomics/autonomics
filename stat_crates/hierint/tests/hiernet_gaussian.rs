//! Golden cross-validation: hierNet Gaussian, weak hierarchy.

use hierint::hiernet;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
struct Fixture {
    n: usize,
    p: usize,
    x: Vec<f64>,
    y: Vec<f64>,
    strong: bool,
    diagonal: bool,
    lamlist: Vec<f64>,
    bp: Vec<f64>,
    bn: Vec<f64>,
    th: Vec<f64>,
    obj: Vec<f64>,
}

fn load_fixture(name: &str) -> Fixture {
    let json = match name {
        "weak" => include_str!("hiernet_gaussian_weak.json"),
        "strong" => include_str!("hiernet_strong.json"),
        _ => panic!("unknown fixture: {name}"),
    };
    serde_json::from_str(json).expect("failed to parse fixture")
}

#[test]
fn test_hiernet_weak_lambda_path() {
    let fx = load_fixture("weak");
    let p = fx.p;
    let n = fx.n;

    let config = hiernet::HierNetConfig {
        family: hiernet::HierNetFamily::Gaussian,
        strong: false,
        diagonal: false,
        n_lam: fx.lamlist.len(),
        lamlist: Some(fx.lamlist.clone()),
        maxiter: 500,
        ..Default::default()
    };

    let path = hiernet::fit_path(&fx.x, &fx.y, &config).expect("fit_path failed");

    // Verify lambda path
    assert_eq!(path.lamlist.len(), fx.lamlist.len());
    for (i, (got, expected)) in path.lamlist.iter().zip(fx.lamlist.iter()).enumerate() {
        let re = (got - expected).abs() / expected.abs().max(1e-10);
        assert!(
            re < 1e-4,
            "lamlist[{i}] mismatch: got {got}, expected {expected}"
        );
    }
}

#[test]
fn test_hiernet_weak_first_lambda_empty() {
    let fx = load_fixture("weak");

    let config = hiernet::HierNetConfig {
        family: hiernet::HierNetFamily::Gaussian,
        strong: false,
        diagonal: false,
        n_lam: fx.lamlist.len(),
        lamlist: Some(fx.lamlist.clone()),
        maxiter: 500,
        ..Default::default()
    };

    let path = hiernet::fit_path(&fx.x, &fx.y, &config).expect("fit_path failed");

    // At lambda_max, all coefficients should be zero
    let coefs = &path.fits[0].coefs;
    let all_zero = coefs.bp.iter().all(|&v| v.abs() < 1e-10)
        && coefs.bn.iter().all(|&v| v.abs() < 1e-10)
        && coefs.th.iter().all(|&v| v.abs() < 1e-10);
    assert!(all_zero, "first lambda should have all-zero coefficients");
}

#[test]
fn test_hiernet_weak_coefficients_nonzero_at_small_lambda() {
    let fx = load_fixture("weak");

    let config = hiernet::HierNetConfig {
        family: hiernet::HierNetFamily::Gaussian,
        strong: false,
        diagonal: false,
        n_lam: fx.lamlist.len(),
        lamlist: Some(fx.lamlist.clone()),
        maxiter: 500,
        ..Default::default()
    };

    let path = hiernet::fit_path(&fx.x, &fx.y, &config).expect("fit_path failed");

    // At the smallest lambda, we should find some nonzero main effects
    let last = &path.fits.last().unwrap().coefs;
    let n_main = last
        .bp
        .iter()
        .zip(&last.bn)
        .filter(|(bp, bn)| (*bp - *bn).abs() > 1e-6)
        .count();
    assert!(
        n_main > 0,
        "expected nonzero main effects at smallest lambda"
    );
}

#[test]
fn test_hiernet_strong_fit() {
    let fx = load_fixture("strong");

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

    let path = hiernet::fit_path(&fx.x, &fx.y, &config).expect("fit_path failed");

    // Verify lambda path
    assert_eq!(path.lamlist.len(), fx.lamlist.len());

    // First lambda: all zeros
    let coefs = &path.fits[0].coefs;
    let all_zero =
        coefs.bp.iter().all(|&v| v.abs() < 1e-10) && coefs.bn.iter().all(|&v| v.abs() < 1e-10);
    assert!(all_zero, "first lambda should be empty");
}
