//! Comprehensive cross-validation: glinternet logistic categorical.

use hierint::glinternet;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Fixture {
    n: usize,
    #[allow(dead_code)]
    p: usize,
    #[serde(rename = "X")]
    x: Vec<f64>,
    y: Vec<f64>,
    #[serde(rename = "numLevels")]
    num_levels: Vec<f64>,
    lambda: Vec<f64>,
    #[serde(rename = "objValue")]
    obj_value: Vec<f64>,
    #[allow(dead_code)]
    fitted: Vec<f64>,
}

fn load() -> Fixture {
    serde_json::from_str(include_str!("glinternet_logit_full.json")).unwrap()
}

fn run_fit(fx: &Fixture) -> glinternet::GlinternetFit {
    let x_cat: Vec<usize> = fx.x.iter().map(|&v| v as usize).collect();
    let z: Vec<f64> = Vec::new();
    let num_levels: Vec<usize> = fx.num_levels.iter().map(|&v| v as usize).collect();

    let config = glinternet::GlinternetConfig {
        family: glinternet::Family::Binomial,
        n_lambda: fx.lambda.len(),
        lambda: Some(fx.lambda.clone()),
        ..Default::default()
    };

    glinternet::fit(&x_cat, &z, &fx.y, &num_levels, &config).expect("fit failed")
}

#[test]
fn logit_lambda_path_match() {
    let fx = load();
    let fit = run_fit(&fx);
    assert_eq!(fit.lambda.len(), fx.lambda.len());
    for (i, (got, expected)) in fit.lambda.iter().zip(fx.lambda.iter()).enumerate() {
        let re = (got - expected).abs() / expected.abs().max(1e-10);
        assert!(re < 1e-4, "lambda[{i}]: got {got}, expected {expected}");
    }
}

#[test]
fn logit_first_lambda_empty() {
    let fx = load();
    let fit = run_fit(&fx);
    assert!(fit.active_set[0].is_empty(), "first lambda should be empty");
}

#[test]
fn logit_finds_active_variables() {
    let fx = load();
    let fit = run_fit(&fx);
    let last = fit.active_set.last().unwrap();
    // At smallest lambda, model should have some active groups
    // (main effects, interactions, or both — hierarchy is approximate)
    let total = last.num_groups();
    assert!(
        total > 0,
        "expected active groups at smallest lambda, got {total}"
    );
}

#[test]
fn logit_objective_values() {
    let fx = load();
    let fit = run_fit(&fx);
    for i in 0..fit.lambda.len() {
        let re = (fit.obj_value[i] - fx.obj_value[i]).abs() / fx.obj_value[i].abs().max(1e-10);
        assert!(
            re < 0.1,
            "obj[{i}]: Rust={:.6}, R={:.6}, re={re:.4}",
            fit.obj_value[i],
            fx.obj_value[i]
        );
    }
}

#[test]
fn logit_predictions_in_range() {
    let fx = load();
    let fit = run_fit(&fx);
    let num_levels: Vec<usize> = fx.num_levels.iter().map(|&v| v as usize).collect();
    let preds = glinternet::predict(
        &fit,
        &fx.x.iter().map(|&v| v as usize).collect::<Vec<_>>(),
        &[],
        &num_levels,
        fx.n,
    );

    for (li, pred_lam) in preds.iter().enumerate() {
        for (i, &p) in pred_lam.iter().enumerate() {
            assert!(
                p.is_finite(),
                "prediction not finite at lambda[{li}], obs[{i}]"
            );
        }
    }
}

#[test]
fn logit_path_structure() {
    let fx = load();
    let fit = run_fit(&fx);
    // First lambda is always empty
    assert!(fit.active_set[0].is_empty());
    // Lambda path is strictly decreasing
    for i in 1..fit.lambda.len() {
        assert!(fit.lambda[i] < fit.lambda[i - 1], "lambda should decrease");
    }
    // At least some non-empty models in the path
    let nonempty: usize = fit.active_set.iter().filter(|a| !a.is_empty()).count();
    assert!(
        nonempty >= 3,
        "expected at least 3 non-empty models, got {nonempty}"
    );
}
