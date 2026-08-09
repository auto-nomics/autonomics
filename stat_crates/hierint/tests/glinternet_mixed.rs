//! Golden cross-validation: glinternet mixed categorical + continuous.

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
    let json = include_str!("glinternet_mixed.json");
    serde_json::from_str(json).expect("failed to parse fixture")
}

#[test]
fn test_glinternet_mixed_fit() {
    let fx = load_fixture();
    let n = fx.n;
    let p = fx.p;
    let num_levels: Vec<usize> = fx.num_levels.iter().map(|&v| v as usize).collect();

    // Split into cat and cont column-major arrays
    let cat_indices: Vec<usize> = (0..p).filter(|&i| num_levels[i] > 1).collect();
    let cont_indices: Vec<usize> = (0..p).filter(|&i| num_levels[i] == 1).collect();

    let x_cat: Vec<usize> = if !cat_indices.is_empty() {
        let mut v = Vec::with_capacity(cat_indices.len() * n);
        for &ci in &cat_indices {
            for i in 0..n {
                v.push(fx.x[ci * n + i] as usize);
            }
        }
        v
    } else {
        Vec::new()
    };

    let z: Vec<f64> = if !cont_indices.is_empty() {
        let mut v = Vec::with_capacity(cont_indices.len() * n);
        for &ci in &cont_indices {
            for i in 0..n {
                v.push(fx.x[ci * n + i]);
            }
        }
        v
    } else {
        Vec::new()
    };

    let config = glinternet::GlinternetConfig {
        family: glinternet::Family::Gaussian,
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
}
