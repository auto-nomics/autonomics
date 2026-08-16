//! Cross-validation for glinternet.
//!
//! Port of `glinternet.cv()` in R.

use super::{Family, GlinternetConfig, GlinternetData, GlinternetFit, fit, predict};

/// CV result.
#[derive(Debug, Clone)]
pub struct CvResult {
    pub lambda: Vec<f64>,
    pub lambda_hat: f64,
    pub lambda_hat_1se: f64,
    pub cv_err: Vec<f64>,
    pub cv_err_std: Vec<f64>,
    pub fit: GlinternetFit, // fit on full data
}

/// Perform k-fold cross-validation.
///
/// Returns CV errors per lambda, lambdaHat (min CV error),
/// and lambdaHat1Std (1-SE rule).
pub fn glinternet_cv(
    x_cat_raw: &[usize],
    z_raw: &[f64],
    y: &[f64],
    num_levels: &[usize],
    config: &GlinternetConfig,
    n_folds: usize,
) -> crate::Result<CvResult> {
    let n = y.len();

    // Fit on full data to get lambda path
    let full_fit = fit(x_cat_raw, z_raw, y, num_levels, config)?;
    let n_lambda = full_fit.lambda.len();

    // Create folds (simple round-robin assignment)
    let fold_ids: Vec<usize> = (0..n).map(|i| i % n_folds).collect();

    // Accumulate errors per lambda per fold
    let mut fold_errors: Vec<Vec<f64>> = vec![Vec::new(); n_lambda];

    for fold in 0..n_folds {
        // Split data
        let train_idx: Vec<usize> = (0..n).filter(|&i| fold_ids[i] != fold).collect();
        let test_idx: Vec<usize> = (0..n).filter(|&i| fold_ids[i] == fold).collect();

        if train_idx.is_empty() || test_idx.is_empty() {
            continue;
        }

        // Build train data
        let p_cat = num_levels.iter().filter(|&&l| l > 1).count();
        let p_cont = num_levels.iter().filter(|&&l| l == 1).count();
        let _n_train = train_idx.len();

        let x_cat_train: Vec<usize> = if p_cat > 0 {
            (0..p_cat)
                .flat_map(|ci| train_idx.iter().map(move |&i| x_cat_raw[ci * n + i]))
                .collect()
        } else {
            Vec::new()
        };

        let z_train: Vec<f64> = if p_cont > 0 {
            (0..p_cont)
                .flat_map(|ci| train_idx.iter().map(move |&i| z_raw[ci * n + i]))
                .collect()
        } else {
            Vec::new()
        };

        let y_train: Vec<f64> = train_idx.iter().map(|&i| y[i]).collect();

        // Use the same lambda path from full fit
        let mut fold_config = config.clone();
        fold_config.lambda = Some(full_fit.lambda.clone());

        let fold_fit = match fit(&x_cat_train, &z_train, &y_train, num_levels, &fold_config) {
            Ok(f) => f,
            Err(_) => continue,
        };

        // Predict on test set
        let n_test = test_idx.len();
        let x_cat_test: Vec<usize> = if p_cat > 0 {
            (0..p_cat)
                .flat_map(|ci| test_idx.iter().map(move |&i| x_cat_raw[ci * n + i]))
                .collect()
        } else {
            Vec::new()
        };

        let z_test: Vec<f64> = if p_cont > 0 {
            (0..p_cont)
                .flat_map(|ci| test_idx.iter().map(move |&i| z_raw[ci * n + i]))
                .collect()
        } else {
            Vec::new()
        };

        let predictions = predict(&fold_fit, &x_cat_test, &z_test, num_levels, n_test);

        // Compute errors per lambda
        let n_fold_lambda = fold_fit.lambda.len().min(n_lambda);
        for j in 0..n_fold_lambda {
            let mut err_sum = 0.0;
            for (k, &i) in test_idx.iter().enumerate() {
                if k < predictions[j].len() {
                    if config.family == Family::Binomial {
                        // 0-1 loss
                        let pred_class = if predictions[j][k] > 0.5 { 1.0 } else { 0.0 };
                        err_sum += (y[i] - pred_class).abs();
                    } else {
                        // Squared error
                        let d = y[i] - predictions[j][k];
                        err_sum += d * d;
                    }
                }
            }
            let err = err_sum / n_test as f64;
            fold_errors[j].push(err);
        }
    }

    // Compute mean and std of CV errors
    let mut cv_err = vec![f64::MAX; n_lambda];
    let mut cv_err_std = vec![0.0; n_lambda];

    for j in 0..n_lambda {
        if !fold_errors[j].is_empty() {
            let m = fold_errors[j].iter().sum::<f64>() / fold_errors[j].len() as f64;
            cv_err[j] = m;
            let var = fold_errors[j].iter().map(|&e| (e - m).powi(2)).sum::<f64>()
                / fold_errors[j].len() as f64;
            cv_err_std[j] = var.sqrt();
        }
    }

    // Find lambdaHat (min CV error)
    let (best_idx, _) = cv_err
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
        .unwrap();
    let lambda_hat = full_fit.lambda[best_idx];

    // 1-SE rule: largest lambda within 1 SE of min
    let threshold = cv_err[best_idx] + cv_err_std[best_idx];
    let mut lambda_hat_1se = lambda_hat;
    for j in 0..n_lambda {
        if cv_err[j] <= threshold && full_fit.lambda[j] >= lambda_hat {
            lambda_hat_1se = full_fit.lambda[j];
            break;
        }
    }

    Ok(CvResult {
        lambda: full_fit.lambda.clone(),
        lambda_hat,
        lambda_hat_1se,
        cv_err,
        cv_err_std,
        fit: full_fit,
    })
}
