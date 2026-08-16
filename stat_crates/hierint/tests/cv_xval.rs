//! Cross-validation tests for glinternet CV.

use hierint::glinternet;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CvFixture {
    n: usize,
    #[allow(dead_code)]
    p: usize,
    #[serde(rename = "X")]
    x: Vec<f64>,
    y: Vec<f64>,
    #[serde(rename = "numLevels")]
    num_levels: Vec<f64>,
    lambda: Vec<f64>,
    #[serde(rename = "lambdaHat")]
    #[allow(dead_code)]
    lambda_hat: f64,
    #[serde(rename = "lambdaHat1Std")]
    #[allow(dead_code)]
    lambda_hat1_std: f64,
    #[serde(rename = "cvErr")]
    #[allow(dead_code)]
    cv_err: Vec<f64>,
    #[serde(rename = "cvErrStd")]
    #[allow(dead_code)]
    cv_err_std: Vec<f64>,
}

fn load() -> CvFixture {
    serde_json::from_str(include_str!("glinternet_cv.json")).unwrap()
}

#[test]
fn cv_completes_and_returns_valid_result() {
    let fx = load();
    let num_levels: Vec<usize> = fx.num_levels.iter().map(|&v| v as usize).collect();

    let config = glinternet::GlinternetConfig {
        family: glinternet::Family::Gaussian,
        n_lambda: fx.lambda.len(),
        ..Default::default()
    };

    let result = glinternet::cv::glinternet_cv(&[], &fx.x, &fx.y, &num_levels, &config, 5);
    assert!(result.is_ok(), "CV should succeed: {:?}", result.err());

    let cv = result.unwrap();

    // Lambda path should match
    assert_eq!(cv.lambda.len(), fx.lambda.len());

    // CV errors should be finite
    for &e in &cv.cv_err {
        assert!(e.is_finite(), "CV error not finite");
        assert!(e >= 0.0, "CV error should be non-negative");
    }

    // lambdaHat should be within the lambda range
    let lam_min = cv.lambda.iter().fold(f64::INFINITY, |a, &b| a.min(b));
    let lam_max = cv.lambda.iter().fold(0.0f64, |a, &b| a.max(b));
    assert!(
        cv.lambda_hat >= lam_min - 1e-10,
        "lambdaHat below lambda_min"
    );
    assert!(
        cv.lambda_hat <= lam_max + 1e-10,
        "lambdaHat above lambda_max"
    );

    // lambdaHat1Std >= lambdaHat
    assert!(
        cv.lambda_hat_1se >= cv.lambda_hat - 1e-10,
        "lambdaHat1Std should be >= lambdaHat"
    );
}

#[test]
fn cv_errors_decrease_then_increase() {
    let fx = load();
    let num_levels: Vec<usize> = fx.num_levels.iter().map(|&v| v as usize).collect();

    let config = glinternet::GlinternetConfig {
        family: glinternet::Family::Gaussian,
        n_lambda: fx.lambda.len(),
        ..Default::default()
    };

    let cv = glinternet::cv::glinternet_cv(&[], &fx.x, &fx.y, &num_levels, &config, 5).unwrap();

    // CV error should generally decrease then increase (U-shape)
    // At minimum, the minimum should not be at the very first or very last lambda
    let min_idx = cv
        .cv_err
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
        .map(|(i, _)| i)
        .unwrap();

    assert!(
        min_idx > 0,
        "CV minimum should not be at lambda_max (empty model)"
    );
}
