//! Binary-phenotype marginal correlation reconstruction — faithful port of
//! `R/binary_processing.R`.
//!
//! Reconstructs the marginal SNP Pearson correlation for a binary phenotype
//! from its test statistic, by searching for the logistic-regression
//! coefficients (b0, b1) whose Wald statistic matches the observed Z, then
//! deriving the dosage/case-status Pearson correlation under the fitted model.

/// Logistic sigmoid `1 / (1 + exp(-x))`.
#[inline]
fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// `sum.error`: relative difference between the model-implied case count and the
/// observed case count.
fn sum_error(b0: f64, b1: f64, n1: f64, count_x: &[f64; 3], val_x: &[f64; 3]) -> f64 {
    let mut s = 0.0;
    for k in 0..3 {
        s += sigmoid(b0 + b1 * val_x[k]) * count_x[k];
    }
    (s - n1) / n1
}

/// `stat.error`: relative difference between the implied Wald statistic and the
/// observed one.
fn stat_error(b0: f64, b1: f64, stat: f64, count_x: &[f64; 3]) -> f64 {
    // x = cbind(1, c(0,1,2)); xsx = t(x) %*% diag(mu*(1-mu)*count.x) %*% x
    let mu: [f64; 3] = [
        sigmoid(b0 + b1 * 0.0),
        sigmoid(b0 + b1 * 1.0),
        sigmoid(b0 + b1 * 2.0),
    ];
    let w: [f64; 3] = [mu[0] * (1.0 - mu[0]) * count_x[0], mu[1] * (1.0 - mu[1]) * count_x[1], mu[2] * (1.0 - mu[2]) * count_x[2]];
    // xsx = [[sum w, sum w*x],[sum w*x, sum w*x^2]]  (x col0=1, col1=0/1/2)
    let xsx00 = w[0] + w[1] + w[2];
    let xsx01 = w[1] * 1.0 + w[2] * 2.0;
    let xsx11 = w[1] * 1.0 + w[2] * 4.0;
    let det = xsx00 * xsx11 - xsx01 * xsx01;
    // var of b1 = (xsx^{-1})[1,1] = xsx[0,0]/det
    if det > 0.0 {
        let var = xsx00 / det;
        (b1 / var.sqrt() - stat).abs() / stat.abs()
    } else {
        f64::INFINITY
    }
}

/// `find.b0`: minimise `sum.error` by stepwise descent with reductions.
fn find_b0(mut b0: f64, b1: f64, n1: f64, count_x: &[f64; 3], val_x: &[f64; 3], mut step: f64, reduction: f64, tolerance: f64) -> f64 {
    let mut err = sum_error(b0, b1, n1, count_x, val_x);
    while step != 0.0 && err.abs() > tolerance {
        let prop_b0 = b0 + -step * err.signum();
        let prop_err = sum_error(prop_b0, b1, n1, count_x, val_x);
        if prop_err.abs() < err.abs() {
            b0 = prop_b0;
            err = prop_err;
        } else {
            step *= reduction;
        }
    }
    b0
}

/// `find.beta`: search (b0, b1) so the Wald statistic matches `stat`.
/// Returns `[b0, b1]`.
fn find_beta(count_x: &[f64; 3], n1: f64, n_orig: f64, stat: f64, tolerance: f64, reduction: f64) -> [f64; 2] {
    let val_x: [f64; 3] = [0.0, 1.0, 2.0];
    // initial b0 = log(N1/(N.orig-N1)); b1 = 0
    let mut b0 = (n1 / (n_orig - n1)).ln();
    let mut b1: f64 = 0.0;
    let mut step = 0.01 * stat.signum();
    let mut try_reverse = false;
    let mut err = stat_error(b0, b1, stat, count_x);
    while step.abs() > tolerance / 10.0 && err > tolerance {
        // propose b1 += step
        let prop_b1 = b1 + step;
        let prop_b0 = find_b0(b0, prop_b1, n1, count_x, &val_x, step.abs(), reduction, tolerance.max(step.abs()));
        let prop_err = stat_error(prop_b0, prop_b1, stat, count_x);
        if prop_err < err {
            b1 = prop_b1;
            b0 = prop_b0;
            err = prop_err;
            try_reverse = false;
        } else if try_reverse {
            step = -step;
            try_reverse = false;
        } else {
            step *= reduction;
            try_reverse = b1 != 0.0;
        }
    }
    [b0, b1]
}

/// `process.binary`: reconstruct marginal Pearson correlations for each SNP.
/// `stat`, `n`, `freq` are per-SNP vectors (aligned); `case_prop` is the
/// phenotype's case proportion. Returns correlations (NaN where reconstruction
/// fails — those SNPs must be dropped by the caller).
pub fn process_binary(stat: &[f64], n: &[f64], freq: &[f64], case_prop: f64) -> Vec<f64> {
    let no_snps = stat.len();
    let mut corrs = vec![f64::NAN; no_snps];
    let n_case_factor = case_prop; // N.case = N * case.prop
    for i in 0..no_snps {
        let mut f = freq[i];
        if f > 0.5 {
            f = 1.0 - f;
        }
        let ni = n[i];
        let n_case = ni * n_case_factor;
        let count_x: [f64; 3] = [
            ni * (1.0 - f).powi(2),
            ni * 2.0 * f * (1.0 - f),
            ni * f.powi(2),
        ];
        let x = [0.0f64, 1.0, 2.0];
        let beta = find_beta(&count_x, n_case, ni, stat[i], 1e-5, 0.25);
        let b0 = beta[0];
        let b1 = beta[1];
        let mu: [f64; 3] = [sigmoid(b0 + b1 * x[0]), sigmoid(b0 + b1 * x[1]), sigmoid(b0 + b1 * x[2])];
        let mut xty = 0.0;
        for k in 0..3 {
            xty += x[k] * count_x[k] * mu[k];
        }
        if xty.is_finite() {
            let sx = count_x[0] * x[0] + count_x[1] * x[1] + count_x[2] * x[2];
            let sx2 = count_x[0] * x[0].powi(2) + count_x[1] * x[1].powi(2) + count_x[2] * x[2].powi(2);
            let sy = n_case;
            let var_x = (sx2 - sx * sx / ni) / (ni - 1.0);
            let var_y = (sy - sy * sy / ni) / (ni - 1.0);
            let cov_xy = (xty - sx * sy / ni) / (ni - 1.0);
            if var_x > 0.0 && var_y > 0.0 {
                corrs[i] = cov_xy / (var_x * var_y).sqrt();
            }
        }
    }
    corrs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_corr_finite_and_bounded() {
        // plausible SNP: N=100000, maf 0.2, case proportion 0.3, z=4
        let n = vec![100000.0];
        let freq = vec![0.2];
        let stat = vec![4.0];
        let corr = process_binary(&stat, &n, &freq, 0.3);
        assert!(corr[0].is_finite());
        assert!(corr[0] > 0.0 && corr[0] < 1.0);
    }
}
