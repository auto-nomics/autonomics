//! Loss functions — cross-entropy, MSE, Huber, Cox partial likelihood.
//!
//! Each loss computes the scalar loss value and the gradient w.r.t. the
//! model's raw output (logits or predictions).

use crate::tensor::Tensor;

// ═══════════════════════════════════════════════════════════════════════
// Binary cross-entropy (from logits)
// ═══════════════════════════════════════════════════════════════════════

/// Binary cross-entropy loss from logits.
///
/// - `logits`: shape `(batch, 1)` raw model output.
/// - `targets`: shape `(batch,)` binary labels in {0, 1}.
///
/// Returns `(loss, grad_logits)` where `grad_logits` has shape `(batch, 1)`.
pub fn binary_cross_entropy(logits: &Tensor, targets: &[f64]) -> (f64, Tensor) {
    let batch = logits.nrows();
    let mut loss = 0.0;
    let mut grad = Tensor::zeros(batch, 1);

    for i in 0..batch {
        let z = logits.at(i, 0);
        let t = targets[i];
        // numerically stable sigmoid.
        let p = sigmoid_stable(z);
        // BCE loss: -[t*log(p) + (1-t)*log(1-p)]
        let eps = 1e-12;
        loss -= t * (p + eps).ln() + (1.0 - t) * (1.0 - p + eps).ln();
        // Gradient w.r.t. logit: p - t
        grad.set(i, 0, (p - t) / batch as f64);
    }

    loss /= batch as f64;
    (loss, grad)
}

// ═══════════════════════════════════════════════════════════════════════
// Mean squared error
// ═══════════════════════════════════════════════════════════════════════

/// MSE loss.
/// - `preds`: shape `(batch, n)`.
/// - `targets`: shape `(batch, n)`.
pub fn mse(preds: &Tensor, targets: &Tensor) -> (f64, Tensor) {
    let (batch, n) = preds.shape();
    let mut loss = 0.0;
    let mut grad = Tensor::zeros(batch, n);

    for i in 0..batch {
        for j in 0..n {
            let d = preds.at(i, j) - targets.at(i, j);
            loss += d * d;
            grad.set(i, j, 2.0 * d / (batch * n) as f64);
        }
    }

    loss /= (batch * n) as f64;
    (loss, grad)
}

// ═══════════════════════════════════════════════════════════════════════
// Huber loss
// ═══════════════════════════════════════════════════════════════════════

pub fn huber(preds: &Tensor, targets: &Tensor, delta: f64) -> (f64, Tensor) {
    let (batch, n) = preds.shape();
    let mut loss = 0.0;
    let mut grad = Tensor::zeros(batch, n);

    for i in 0..batch {
        for j in 0..n {
            let d = preds.at(i, j) - targets.at(i, j);
            if d.abs() <= delta {
                loss += 0.5 * d * d;
                grad.set(i, j, d / (batch * n) as f64);
            } else {
                loss += delta * (d.abs() - 0.5 * delta);
                grad.set(i, j, delta * d.signum() / (batch * n) as f64);
            }
        }
    }

    loss /= (batch * n) as f64;
    (loss, grad)
}

// ═══════════════════════════════════════════════════════════════════════
// Cox partial likelihood loss (negative log)
// ═══════════════════════════════════════════════════════════════════════

/// Negative Cox partial log-likelihood loss.
///
/// Given risk scores (logits) `h(x)` for each sample, times `t`, and event
/// indicators `e` (1 = event, 0 = censored), the partial likelihood is:
///
/// ```text
/// L = ∏_{i: e_i=1} exp(h_i) / Σ_{j: t_j ≥ t_i} exp(h_j)
/// ```
///
/// The negative log-likelihood is minimised.  Samples should be sorted by
/// descending time before calling (risk set = all with time ≥ current).
///
/// - `risk_scores`: shape `(batch, 1)` model output.
/// - `times`: survival times.
/// - `events`: event indicators (1 = event, 0 = censored).
///
/// Returns `(loss, grad)` where `grad` has shape `(batch, 1)`.
pub fn cox_partial_likelihood_loss(
    risk_scores: &Tensor,
    times: &[f64],
    events: &[usize],
) -> (f64, Tensor) {
    let batch = risk_scores.nrows();
    let h: Vec<f64> = (0..batch).map(|i| risk_scores.at(i, 0)).collect();

    // Sort indices by descending time (largest time first → smallest risk set first).
    let mut order: Vec<usize> = (0..batch).collect();
    order.sort_by(|&a, &b| times[b].partial_cmp(&times[a]).unwrap_or(std::cmp::Ordering::Equal));

    // Compute cumulative risk set sums.
    // For numerical stability, subtract the max hazard.
    let h_max = h.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let exp_h: Vec<f64> = h.iter().map(|&v| (v - h_max).exp()).collect();

    let mut loss = 0.0f64;
    let mut n_events = 0usize;

    // For each unique event time, we compute the Breslow partial likelihood.
    // Risk set at time t_i = {j : t_j >= t_i}.
    // Since sorted descending, risk set accumulates as we go.
    let mut cum_exp_h = 0.0; // Σ exp(h_j) for j in risk set

    // Process in order of descending time.
    for &idx in &order {
        // Add this sample to the risk set.
        cum_exp_h += exp_h[idx];

        if events[idx] == 1 {
            // This sample had an event.
            n_events += 1;
            // Negative log partial likelihood: -(h_i - log(Σ_exp))
            loss -= h[idx] - h_max - cum_exp_h.ln();
        }
    }

    // Compute risk-set sums for each event sample by accumulating in
    // descending-time order.  Since the risk set for an event at time t
    // is { j : t_j >= t }, and we process samples from largest to smallest
    // time, the cumulative sum `cum_exp_h` at the point we encounter an
    // event sample equals Σ_{j: t_j >= t_event} exp(h_j).
    let mut cum_exp_h_2 = 0.0;
    let mut event_contribs: Vec<(usize, f64)> = Vec::new();

    for &idx in &order {
        cum_exp_h_2 += exp_h[idx];
        if events[idx] == 1 {
            event_contribs.push((idx, cum_exp_h_2));
        }
    }

    // O(n × n_events) gradient computation for NEGATIVE log-likelihood.
    // ∂NLL/∂h_i (event sample) = -(1 - exp(h_i)/S) = -1 + exp(h_i)/S
    // ∂NLL/∂h_j (risk set member) = exp(h_j)/S
    let mut grad_correct = vec![0.0; batch];
    for &idx in &order {
        if events[idx] == 1 {
            grad_correct[idx] -= 1.0;
        }
    }

    for &(event_sample, s_at_event) in &event_contribs {
        for &j in &order {
            if times[j] >= times[event_sample] - 1e-12 {
                grad_correct[j] += exp_h[j] / s_at_event;
            }
        }
    }

    if n_events > 0 {
        loss /= n_events as f64;
        for g in &mut grad_correct {
            *g /= n_events as f64;
        }
    }

    let mut grad_tensor = Tensor::zeros(batch, 1);
    for i in 0..batch {
        grad_tensor.set(i, 0, grad_correct[i]);
    }

    (loss, grad_tensor)
}

// ═══════════════════════════════════════════════════════════════════════
// VAE KL divergence
// ═══════════════════════════════════════════════════════════════════════

/// KL divergence for a diagonal Gaussian: -0.5 * Σ(1 + log_var - mu^2 - exp(log_var)).
/// Returns `(kl, grad_mu, grad_log_var)`.
pub fn kl_divergence(mu: &[f64], log_var: &[f64]) -> (f64, Vec<f64>, Vec<f64>) {
    let n = mu.len();
    let mut kl = 0.0;
    let mut grad_mu = vec![0.0; n];
    let mut grad_lv = vec![0.0; n];

    for i in 0..n {
        kl += -0.5 * (1.0 + log_var[i] - mu[i].powi(2) - log_var[i].exp());
        grad_mu[i] = mu[i];
        grad_lv[i] = -0.5 * (1.0 - log_var[i].exp());
    }

    kl /= n as f64;
    for g in &mut grad_mu {
        *g /= n as f64;
    }
    for g in &mut grad_lv {
        *g /= n as f64;
    }

    (kl, grad_mu, grad_lv)
}

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

/// Numerically stable sigmoid.
pub fn sigmoid_stable(x: f64) -> f64 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let e = x.exp();
        e / (1.0 + e)
    }
}

/// Softmax along the last axis (columns).
pub fn softmax(logits: &Tensor) -> Tensor {
    let (batch, n) = logits.shape();
    let mut out = Tensor::zeros(batch, n);
    for i in 0..batch {
        let row: Vec<f64> = logits.row(i);
        let max_val = row.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let exps: Vec<f64> = row.iter().map(|&v| (v - max_val).exp()).collect();
        let sum: f64 = exps.iter().sum();
        for j in 0..n {
            out.set(i, j, exps[j] / sum);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bce_basic() {
        let logits = Tensor::from_rows(2, 1, &[0.0, 0.0]);
        let targets = vec![0.0, 1.0];
        let (loss, grad) = binary_cross_entropy(&logits, &targets);
        // At logit=0: p=0.5, loss for each = -[t*log(0.5)+(1-t)*log(0.5)] = log(2).
        assert!((loss - std::f64::consts::LN_2).abs() < 1e-10, "loss={loss}");
        // grad = p - t: [0.5-0, 0.5-1] = [0.5, -0.5], divided by batch=2.
        assert!((grad.at(0, 0) - 0.25).abs() < 1e-10);
        assert!((grad.at(1, 0) - (-0.25)).abs() < 1e-10);
    }

    #[test]
    fn test_bce_perfect_prediction() {
        // Very high logit with target=1 → near-zero loss.
        let logits = Tensor::from_rows(1, 1, &[20.0]);
        let (loss, _) = binary_cross_entropy(&logits, &[1.0]);
        assert!(loss < 1e-5, "loss should be near zero, got {loss}");
    }

    #[test]
    fn test_mse() {
        let preds = Tensor::from_rows(1, 2, &[1.0, 2.0]);
        let targets = Tensor::from_rows(1, 2, &[1.0, 3.0]);
        let (loss, grad) = mse(&preds, &targets);
        // MSE = (0^2 + 1^2) / 2 = 0.5.
        assert!((loss - 0.5).abs() < 1e-10);
        // grad = 2*(pred-target)/(batch*n) = 2*(0,-1)/2 = (0,-1).
        assert!((grad.at(0, 0)).abs() < 1e-10);
        assert!((grad.at(0, 1) - (-1.0)).abs() < 1e-10);
    }

    #[test]
    fn test_sigmoid_stable() {
        assert!((sigmoid_stable(0.0) - 0.5).abs() < 1e-10);
        assert!(sigmoid_stable(100.0) > 0.99);
        assert!(sigmoid_stable(-100.0) < 0.01);
        // Should not overflow.
        assert!(sigmoid_stable(1e10).is_finite());
        assert!(sigmoid_stable(-1e10).is_finite());
    }

    #[test]
    fn test_softmax_sums_to_one() {
        let logits = Tensor::from_rows(2, 3, &[1.0, 2.0, 3.0, 0.0, 0.0, 0.0]);
        let probs = softmax(&logits);
        for i in 0..2 {
            let sum: f64 = (0..3).map(|j| probs.at(i, j)).sum();
            assert!((sum - 1.0).abs() < 1e-10);
        }
    }

    #[test]
    fn test_cox_loss_basic() {
        // Simple case: 3 samples, 2 events.
        // h = [1, 2, 3], times = [3, 2, 1], events = [1, 1, 0]
        let risk = Tensor::from_rows(3, 1, &[1.0, 2.0, 3.0]);
        let times = vec![3.0, 2.0, 1.0];
        let events = vec![1, 1, 0];
        let (loss, grad) = cox_partial_likelihood_loss(&risk, &times, &events);
        assert!(loss.is_finite());
        assert_eq!(grad.shape(), (3, 1));
        // All grads should be finite.
        for i in 0..3 {
            assert!(grad.at(i, 0).is_finite());
        }
    }

    #[test]
    fn test_cox_gradient_check() {
        // Numerical gradient check.
        let h = vec![0.5, -0.3, 0.8, 1.2];
        let times = vec![5.0, 3.0, 7.0, 2.0];
        let events = vec![1, 0, 1, 1];

        let risk = Tensor::from_rows(4, 1, &h);
        let (_, grad_analytic) = cox_partial_likelihood_loss(&risk, &times, &events);

        let eps = 1e-6;
        for k in 0..4 {
            let mut h_plus = h.clone();
            h_plus[k] += eps;
            let risk_plus = Tensor::from_rows(4, 1, &h_plus);
            let (loss_plus, _) = cox_partial_likelihood_loss(&risk_plus, &times, &events);

            let mut h_minus = h.clone();
            h_minus[k] -= eps;
            let risk_minus = Tensor::from_rows(4, 1, &h_minus);
            let (loss_minus, _) = cox_partial_likelihood_loss(&risk_minus, &times, &events);

            let numeric = (loss_plus - loss_minus) / (2.0 * eps);
            let analytic = grad_analytic.at(k, 0);
            let rel_err = (numeric - analytic).abs() / (numeric.abs() + 1e-8);
            assert!(
                rel_err < 1e-4,
                "cox grad mismatch at {k}: numeric={numeric:.8}, analytic={analytic:.8}, rel_err={rel_err:.2e}"
            );
        }
    }

    #[test]
    fn test_kl_divergence() {
        let mu = vec![0.0, 0.0];
        let log_var = vec![0.0, 0.0]; // N(0,1)
        let (kl, grad_mu, grad_lv) = kl_divergence(&mu, &log_var);
        // KL(N(0,1) || N(0,1)) = 0.
        assert!(kl.abs() < 1e-10, "kl = {kl}");
        // grad_mu = mu/n = 0.
        assert!(grad_mu[0].abs() < 1e-10);
        // grad_lv = -0.5*(1-exp(0))/n = 0.
        assert!(grad_lv[0].abs() < 1e-10);
    }
}
