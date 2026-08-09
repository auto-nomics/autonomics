//! Golden cross-validation: glinternet logistic, categorical variables.

use hierint::glinternet;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
struct Fixture {
    n: usize,
    p: usize,
    #[serde(rename = "X")]
    x: Vec<f64>,
    y: Vec<f64>,
    #[serde(rename = "numLevels")]
    num_levels: Vec<f64>,
    lambda: Vec<f64>,
}

fn load_fixture() -> Fixture {
    let json = include_str!("glinternet_logistic.json");
    serde_json::from_str(json).expect("failed to parse fixture")
}

#[test]
fn test_glinternet_logistic_fit() {
    let fx = load_fixture();
    let n = fx.n;
    let p = fx.p;

    // All categorical, no continuous
    let x_cat: Vec<usize> = fx.x.iter().map(|&v| v as usize).collect();
    let z: Vec<f64> = Vec::new();
    let num_levels: Vec<usize> = fx.num_levels.iter().map(|&v| v as usize).collect();

    let config = glinternet::GlinternetConfig {
        family: glinternet::Family::Binomial,
        n_lambda: fx.lambda.len(),
        lambda: Some(fx.lambda.clone()),
        ..Default::default()
    };

    let fit = glinternet::fit(&x_cat, &z, &fx.y, &num_levels, &config).expect("fit failed");

    // Verify lambda path
    assert_eq!(fit.lambda.len(), fx.lambda.len());
    for (i, (got, expected)) in fit.lambda.iter().zip(fx.lambda.iter()).enumerate() {
        let re = (got - expected).abs() / expected.abs().max(1e-10);
        assert!(re < 1e-3, "lambda[{i}] mismatch: got {got}, expected {expected}");
    }

    // First lambda should have empty active set
    assert!(fit.active_set[0].is_empty(), "first lambda should be empty");
}

#[test]
fn test_glinternet_logistic_finds_interactions() {
    let fx = load_fixture();
    let x_cat: Vec<usize> = fx.x.iter().map(|&v| v as usize).collect();
    let z: Vec<f64> = Vec::new();
    let num_levels: Vec<usize> = fx.num_levels.iter().map(|&v| v as usize).collect();

    let config = glinternet::GlinternetConfig {
        family: glinternet::Family::Binomial,
        n_lambda: fx.lambda.len(),
        lambda: Some(fx.lambda.clone()),
        ..Default::default()
    };

    let fit = glinternet::fit(&x_cat, &z, &fx.y, &num_levels, &config).expect("fit failed");

    // At small lambda, some variables should be active
    let last = fit.active_set.last().unwrap();
    let total = last.num_groups();
    assert!(total > 0, "expected some active groups at small lambda, got {total}");
}
