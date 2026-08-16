//! Cross-validation for hierNet — port of `hierNet.cv()` in R.

use super::{HierNetConfig, HierNetFamily, HierNetPath, fit_path, predict};

/// CV result.
#[derive(Debug, Clone)]
pub struct HierNetCvResult {
    pub lamlist: Vec<f64>,
    pub lam_hat: f64,
    pub lam_hat_1se: f64,
    pub cv_err: Vec<f64>,
    pub cv_se: Vec<f64>,
    pub nonzero: Vec<usize>,
    pub path: HierNetPath,
}

/// Perform k-fold cross-validation.
pub fn hiernet_cv(
    x: &[f64],
    y: &[f64],
    config: &HierNetConfig,
    n_folds: usize,
) -> crate::Result<HierNetCvResult> {
    let n = y.len();
    let p = x.len() / n;

    // Fit full path
    let path = fit_path(x, y, config)?;
    let n_lam = path.lamlist.len();

    // Folds
    let fold_ids: Vec<usize> = (0..n).map(|i| i % n_folds).collect();

    let mut err2 = vec![vec![f64::MAX; n_lam]; n_folds];

    for fold in 0..n_folds {
        let train_idx: Vec<usize> = (0..n).filter(|&i| fold_ids[i] != fold).collect();
        let test_idx: Vec<usize> = (0..n).filter(|&i| fold_ids[i] == fold).collect();

        if train_idx.is_empty() || test_idx.is_empty() {
            continue;
        }

        // Build train data
        let _n_train = train_idx.len();
        let x_train: Vec<f64> = (0..p)
            .flat_map(|j| train_idx.iter().map(move |&i| x[j * n + i]))
            .collect();
        let y_train: Vec<f64> = train_idx.iter().map(|&i| y[i]).collect();

        // Fit path on training data using the same lamlist
        let mut fold_config = config.clone();
        fold_config.lamlist = Some(path.lamlist.clone());

        let fold_path = match fit_path(&x_train, &y_train, &fold_config) {
            Ok(fp) => fp,
            Err(_) => continue,
        };

        // Predict on test set
        let n_test = test_idx.len();
        let x_test: Vec<f64> = (0..p)
            .flat_map(|j| test_idx.iter().map(move |&i| x[j * n + i]))
            .collect();

        let n_fold_lam = fold_path.fits.len().min(n_lam);
        for j in 0..n_fold_lam {
            let yhat = predict(&fold_path.fits[j], &x_test, n_test);
            let mut err_sum = 0.0;
            for (k, &i) in test_idx.iter().enumerate() {
                if k < yhat.len() {
                    if config.family == HierNetFamily::Logistic {
                        err_sum += if (yhat[k] > 0.5) != (y[i] > 0.5) {
                            1.0
                        } else {
                            0.0
                        };
                    } else {
                        let d = y[i] - yhat[k];
                        err_sum += d * d;
                    }
                }
            }
            err2[fold][j] = err_sum / n_test as f64;
        }
    }

    // Aggregate
    let mut cv_err = vec![0.0; n_lam];
    let mut cv_se = vec![0.0; n_lam];
    for j in 0..n_lam {
        let valid: Vec<f64> = err2
            .iter()
            .map(|f| f[j])
            .filter(|&e| e < f64::MAX)
            .collect();
        if !valid.is_empty() {
            let m = valid.iter().sum::<f64>() / valid.len() as f64;
            cv_err[j] = m;
            let var = valid.iter().map(|&e| (e - m).powi(2)).sum::<f64>() / valid.len() as f64;
            cv_se[j] = (var / n_folds as f64).sqrt();
        }
    }

    // nonzero per lambda
    let nonzero: Vec<usize> = path
        .fits
        .iter()
        .map(|f| {
            let main = f
                .coefs
                .bp
                .iter()
                .zip(&f.coefs.bn)
                .filter(|(bp, bn)| (*bp - *bn).abs() > 1e-6)
                .count();
            let mut inter = 0usize;
            for j in 0..p {
                for k in (j + 1)..p {
                    if (f.coefs.th[j + p * k] + f.coefs.th[k + p * j]).abs() > 1e-6 {
                        inter += 1;
                    }
                }
            }
            main + inter
        })
        .collect();

    // lamHat and lamHat1se
    let (best_idx, _) = cv_err
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
        .unwrap();
    let lam_hat = path.lamlist[best_idx];
    let threshold = cv_err[best_idx] + cv_se[best_idx];
    let mut lam_hat_1se = lam_hat;
    for j in 0..n_lam {
        if cv_err[j] <= threshold && path.lamlist[j] >= lam_hat {
            lam_hat_1se = path.lamlist[j];
            break;
        }
    }

    Ok(HierNetCvResult {
        lamlist: path.lamlist.clone(),
        lam_hat,
        lam_hat_1se,
        cv_err,
        cv_se,
        nonzero,
        path,
    })
}
