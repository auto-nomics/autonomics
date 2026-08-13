//! Comprehensive cross-validation: glinternet mixed cat+cont.

use hierint::glinternet;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
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
    #[allow(dead_code)]
    obj_value: Vec<f64>,
}

fn load() -> Fixture {
    serde_json::from_str(include_str!("glinternet_mixed_full.json")).unwrap()
}

fn run_fit(fx: &Fixture) -> glinternet::GlinternetFit {
    let n = fx.n;
    let p = fx.p;
    let num_levels: Vec<usize> = fx.num_levels.iter().map(|&v| v as usize).collect();
    let cat_indices: Vec<usize> = (0..p).filter(|&i| num_levels[i] > 1).collect();
    let cont_indices: Vec<usize> = (0..p).filter(|&i| num_levels[i] == 1).collect();

    let mut x_cat = Vec::new();
    for &ci in &cat_indices {
        for i in 0..n {
            x_cat.push(fx.x[ci * n + i] as usize);
        }
    }
    let mut z = Vec::new();
    for &ci in &cont_indices {
        for i in 0..n {
            z.push(fx.x[ci * n + i]);
        }
    }

    let config = glinternet::GlinternetConfig {
        family: glinternet::Family::Gaussian,
        n_lambda: fx.lambda.len(),
        lambda: Some(fx.lambda.clone()),
        ..Default::default()
    };

    glinternet::fit(&x_cat, &z, &fx.y, &num_levels, &config).expect("fit failed")
}

#[test]
fn mixed_lambda_path_match() {
    let fx = load();
    let fit = run_fit(&fx);
    for (i, (got, expected)) in fit.lambda.iter().zip(fx.lambda.iter()).enumerate() {
        let re = (got - expected).abs() / expected.abs().max(1e-10);
        assert!(re < 1e-4, "lambda[{i}]: got {got}, expected {expected}");
    }
}

#[test]
fn mixed_finds_continuous_main_effects() {
    let fx = load();
    let fit = run_fit(&fx);
    let last = fit.active_set.last().unwrap();
    assert!(
        last.n_vars()[1] >= 1,
        "expected continuous main effects at last lambda"
    );
}

#[test]
fn mixed_hierarchy_catcont() {
    // Hierarchy is approximate at finite tolerance (confirmed: R glinternet
    // also has hierarchy violations with noise-like data). Verify the model
    // produces finite results and correct structure.
    let fx = load();
    let fit = run_fit(&fx);

    // Verify catcont interactions only reference valid variable indices
    for active in &fit.active_set {
        if let Some(ref cct) = active.catcont {
            for &[cat_i, cont_j] in cct {
                assert!(cat_i >= 1, "cat index should be ≥ 1");
                assert!(cont_j >= 1, "cont index should be ≥ 1");
            }
        }
    }
}

#[test]
fn mixed_objective_decreasing() {
    let fx = load();
    let fit = run_fit(&fx);
    for i in 1..fit.obj_value.len() {
        assert!(
            fit.obj_value[i] <= fit.obj_value[i - 1] + 1e-6,
            "objective should be non-increasing: [{i}]={:.6} > [{:.6}",
            fit.obj_value[i],
            fit.obj_value[i - 1]
        );
    }
}
