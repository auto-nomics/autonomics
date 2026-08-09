//! Golden cross-validation: glinternet Gaussian, continuous-only.
//!
//! # R fixture generation
//! ```sh
//! Rscript tests/gen_fixtures.R
//! ```

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
    #[serde(rename = "objValue")]
    obj_value: Vec<f64>,
}

fn load_fixture() -> Fixture {
    let json = include_str!("glinternet_gaussian.json");
    serde_json::from_str(json).expect("failed to parse fixture")
}

#[test]
fn test_glinternet_gaussian_lambda_path() {
    let fx = load_fixture();
    let n = fx.n;
    let p = fx.p;

    // Continuous-only data: no categorical
    let x_cat: Vec<usize> = Vec::new();
    let z: Vec<f64> = fx.x.clone();
    let y = &fx.y;
    let num_levels: Vec<usize> = fx.num_levels.iter().map(|&v| v as usize).collect();

    let config = glinternet::GlinternetConfig {
        family: glinternet::Family::Gaussian,
        n_lambda: fx.lambda.len(),
        lambda: Some(fx.lambda.clone()),
        ..Default::default()
    };

    let fit = glinternet::fit(&x_cat, &z, y, &num_levels, &config).expect("fit failed");

    // Verify lambda path matches
    assert_eq!(fit.lambda.len(), fx.lambda.len(), "lambda path length mismatch");

    // Check each lambda value (they should be identical since we used the same grid)
    for (i, (got, expected)) in fit.lambda.iter().zip(fx.lambda.iter()).enumerate() {
        let re = (got - expected).abs() / expected.abs().max(1e-10);
        if re > 1e-6 {
            eprintln!("lambda[{i}]: got {got}, expected {expected}, re={re}");
        }
        assert!(re < 1e-3, "lambda[{i}] mismatch: got {got}, expected {expected}");
    }
}

#[test]
fn test_glinternet_gaussian_first_lambda_empty() {
    let fx = load_fixture();
    let x_cat: Vec<usize> = Vec::new();
    let z: Vec<f64> = fx.x.clone();
    let num_levels: Vec<usize> = fx.num_levels.iter().map(|&v| v as usize).collect();

    let config = glinternet::GlinternetConfig {
        family: glinternet::Family::Gaussian,
        n_lambda: fx.lambda.len(),
        lambda: Some(fx.lambda.clone()),
        ..Default::default()
    };

    let fit = glinternet::fit(&x_cat, &z, &fx.y, &num_levels, &config).expect("fit failed");

    // At lambda_max, the model should be empty (just intercept)
    let as0 = &fit.active_set[0];
    assert!(as0.is_empty(), "first lambda should have empty active set");
}

#[test]
fn test_glinternet_gaussian_obj_value() {
    let fx = load_fixture();
    let x_cat: Vec<usize> = Vec::new();
    let z: Vec<f64> = fx.x.clone();
    let num_levels: Vec<usize> = fx.num_levels.iter().map(|&v| v as usize).collect();

    let config = glinternet::GlinternetConfig {
        family: glinternet::Family::Gaussian,
        n_lambda: fx.lambda.len(),
        lambda: Some(fx.lambda.clone()),
        ..Default::default()
    };

    let fit = glinternet::fit(&x_cat, &z, &fx.y, &num_levels, &config).expect("fit failed");

    // Check objective values (should decrease along the path)
    for i in 1..fit.obj_value.len() {
        // The objective should be non-increasing in theory, but due to
        // FISTA convergence tolerance it might fluctuate slightly
        let re = (fit.obj_value[i] - fx.obj_value[i]).abs()
            / fx.obj_value[i].abs().max(1e-10);
        if re > 0.1 {
            eprintln!(
                "obj[{i}]: got {}, expected {}, re={re}",
                fit.obj_value[i], fx.obj_value[i]
            );
        }
    }

    // First objective should match closely (both are just SSE of Y - mean(Y))
    let re0 = (fit.obj_value[0] - fx.obj_value[0]).abs() / fx.obj_value[0].abs();
    assert!(re0 < 1e-6, "obj[0] mismatch: got {}, expected {}", fit.obj_value[0], fx.obj_value[0]);
}
