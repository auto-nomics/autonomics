#!/usr/bin/env Rscript
# Generate golden fixtures for grf crate cross-validation tests.
#
# Each fixture is a deterministic dataset + R grf output, dumped to CSV
# files in `bio_crates/grf/fixtures/`. Rust tests load these and compare
# bit-equal (or tolerance-close for stochastic algorithms) against the
# grf-sys FFI output.
#
# Run:
#   Rscript bio_crates/grf/fixtures/generate.R
#
# Test data archive convention: copy the generated files to
#   aliyun://autonomics-data/grf/test-data/fixtures/
# so other machines can reproduce. See MEMORY.md for the rclone command.

suppressPackageStartupMessages(library(grf))

set.seed(20240101)  # global seed for any stochastic helper
# Write CSV with full 17-digit precision so Rust's f64 round-trip matches
# R's RNG state when used to seed grf.
fmt <- formatC  # alias
write_csv <- function(mat, path) {
  cn <- sprintf("c%d", seq_len(ncol(mat)) - 1L)
  body <- apply(mat, 1, function(row) {
    paste(fmt(row, format = "f", digits = 17), collapse = ",")
  })
  writeLines(c(paste(cn, collapse = ","), body), path)
}
write_vec <- function(v, path) {
  body <- vapply(seq_along(v), function(i) fmt(v[i], format = "f", digits = 17), character(1))
  writeLines(c("value", body), path)
}

OUT <- commandArgs(trailingOnly = TRUE)
if (length(OUT) < 1L) OUT <- "."

# ───────────────────────── regression_forest ─────────────────────────
{
  set.seed(11)
  n <- 80; p <- 4
  X <- matrix(rnorm(n * p), n, p)
  Y <- X[, 1] + 0.5 * X[, 2] + rnorm(n, sd = 0.5)
  rf <- regression_forest(X, Y, num.trees = 50, seed = 1,
                          compute.oob.predictions = TRUE,
                          num.threads = 1)
  oob <- predict(rf)$predictions
  write_csv(X, file.path(OUT, "reg_X.csv"))
  write_vec(Y, file.path(OUT, "reg_Y.csv"))
  write_vec(oob, file.path(OUT, "reg_oob.csv"))
  cat(sprintf("[reg] oob[1:3]: %s\n",
              paste(formatC(oob[1:3], digits = 17), collapse = " ")))
}

# ───────────────────────── regression_forest: predict on new data ─────
{
  set.seed(12)
  n_test <- 20
  X_test <- matrix(rnorm(n_test * 4), n_test, 4)
  preds <- predict(rf, X_test)$predictions
  write_csv(X_test, file.path(OUT, "reg_X_test.csv"))
  write_vec(preds, file.path(OUT, "reg_test_pred.csv"))
  cat(sprintf("[reg-test] pred[1:3]: %s\n",
              paste(formatC(preds[1:3], digits = 17), collapse = " ")))
}

# ───────────────────────── causal_forest ─────────────────────────
{
  set.seed(21)
  n <- 150; p <- 3
  X <- matrix(rnorm(n * p), n, p)
  W <- rbinom(n, 1, 0.5)
  # Heterogeneous TE: τ(x) = 1 + x₁.
  tau <- 1 + X[, 1]
  Y <- tau * W + rnorm(n, sd = 0.5)

  cf <- causal_forest(X, Y, W, num.trees = 80, seed = 7,
                       compute.oob.predictions = TRUE,
                       num.threads = 1)
  tau_hat <- predict(cf)$predictions
  # ATE
  ate <- average_treatment_effect(cf, target.sample = "all")
  ate_treated <- average_treatment_effect(cf, target.sample = "treated")

  write_csv(X, file.path(OUT, "causal_X.csv"))
  write_vec(Y, file.path(OUT, "causal_Y.csv"))
  write_vec(W, file.path(OUT, "causal_W.csv"))
  write_vec(tau_hat, file.path(OUT, "causal_tau_oob.csv"))
  write_vec(c(ate["estimate"], ate["std.err"]), file.path(OUT, "causal_ate.csv"))
  write_vec(c(ate_treated["estimate"], ate_treated["std.err"]),
            file.path(OUT, "causal_ate_treated.csv"))
  # Save Y.hat / W.hat for downstream tests.
  write_vec(cf$Y.hat, file.path(OUT, "causal_Yhat.csv"))
  write_vec(cf$W.hat, file.path(OUT, "causal_What.csv"))
  cat(sprintf("[causal] tau[1:3]: %s  ate=%.6f ate_tr=%.6f\n",
              paste(formatC(tau_hat[1:3], digits = 10), collapse = " "),
              ate["estimate"], ate_treated["estimate"]))
}

# ───────────────────────── quantile_forest ─────────────────────────
{
  set.seed(31)
  n <- 60; p <- 3
  X <- matrix(rnorm(n * p), n, p)
  Y <- X[, 1] + 0.5 * X[, 2] + rnorm(n, sd = 0.5)
  qf <- quantile_forest(X, Y, quantiles = c(0.25, 0.5, 0.75),
                         num.trees = 50, seed = 1, num.threads = 1)
  qpreds <- predict(qf, X, quantiles = c(0.25, 0.5, 0.75))$predictions
  write_csv(X, file.path(OUT, "q_X.csv"))
  write_vec(Y, file.path(OUT, "q_Y.csv"))
  write_csv(qpreds, file.path(OUT, "q_oob.csv"))
  cat(sprintf("[quantile] oob[1:3,]: %.6f %.6f %.6f\n",
              qpreds[1,1], qpreds[1,2], qpreds[1,3]))
}

# ───────────────────────── probability_forest ─────────────────────────
{
  set.seed(41)
  n <- 60; p <- 2
  X <- matrix(rnorm(n * p), n, p)
  Y <- factor((seq_len(n) %% 3L) + 1L)
  pf <- probability_forest(X, Y, num.trees = 50, seed = 1,
                            compute.oob.predictions = TRUE,
                            num.threads = 1)
  poob <- predict(pf)$predictions
  write_csv(X, file.path(OUT, "p_X.csv"))
  write_vec(as.integer(Y), file.path(OUT, "p_Y.csv"))
  write_csv(poob, file.path(OUT, "p_oob.csv"))
  cat(sprintf("[prob] row1 probs sum: %.10f\n", sum(poob[1, ])))
}

# ───────────────────────── survival_forest ─────────────────────────
{
  set.seed(51)
  n <- 60; p <- 2
  X <- matrix(rnorm(n * p), n, p)
  time <- pmax(0.1, abs(X[, 1]) + rnorm(n, sd = 0.5))
  censor <- rbinom(n, 1, 0.7)
  sf <- survival_forest(X, time, censor, num.trees = 50, seed = 1,
                         compute.oob.predictions = TRUE,
                         num.threads = 1)
  soob <- predict(sf)$predictions
  write_csv(X, file.path(OUT, "s_X.csv"))
  write_vec(time, file.path(OUT, "s_time.csv"))
  write_vec(censor, file.path(OUT, "s_censor.csv"))
  write_csv(soob, file.path(OUT, "s_oob.csv"))
  cat(sprintf("[surv] oob dim: %d x %d\n", nrow(soob), ncol(soob)))
}

# ───────────────────────── multi_regression_forest ─────────────────────────
{
  set.seed(61)
  n <- 50; p <- 2
  X <- matrix(rnorm(n * p), n, p)
  Y <- cbind(X[, 1] + X[, 2], X[, 1] - X[, 2]) + rnorm(n * 2, sd = 0.3)
  mrf <- multi_regression_forest(X, Y, num.trees = 50, seed = 1,
                                 compute.oob.predictions = TRUE,
                                 num.threads = 1)
  moob <- predict(mrf)$predictions
  write_csv(X, file.path(OUT, "mr_X.csv"))
  write_csv(Y, file.path(OUT, "mr_Y.csv"))
  write_csv(moob, file.path(OUT, "mr_oob.csv"))
  cat(sprintf("[multi-reg] oob[1,]: %.6f %.6f\n", moob[1, 1], moob[1, 2]))
}

# ───────────────────────── instrumental_forest ─────────────────────────
{
  set.seed(71)
  n <- 120; p <- 3
  X <- matrix(rnorm(n * p), n, p)
  Z <- sign(X[, 1])  # binary instrument
  W <- ifelse(Z > 0.3, 1, 0)
  Y <- 0.5 * W + rnorm(n, sd = 0.5)
  ivf <- instrumental_forest(X, Y, W, Z, num.trees = 80, seed = 13,
                              compute.oob.predictions = TRUE,
                              num.threads = 1)
  ioob <- predict(ivf)$predictions
  write_csv(X, file.path(OUT, "iv_X.csv"))
  write_vec(Y, file.path(OUT, "iv_Y.csv"))
  write_vec(W, file.path(OUT, "iv_W.csv"))
  write_vec(Z, file.path(OUT, "iv_Z.csv"))
  write_vec(ioob, file.path(OUT, "iv_oob.csv"))
  cat(sprintf("[iv] tau[1:3]: %s\n",
              paste(formatC(ioob[1:3], digits = 10), collapse = " ")))
}

# ───────────────────────── lm_forest ─────────────────────────
{
  set.seed(81)
  n <- 80; p <- 2
  X <- matrix(rnorm(n * p), n, p)
  Y <- matrix(1.5 * X[, 1] + 0.3 * X[, 2] + rnorm(n, sd = 0.3), n, 1)
  W <- cbind(X[, 1], X[, 2])
  lmf <- lm_forest(X, Y, W, num.trees = 50, seed = 1,
                     compute.oob.predictions = TRUE, num.threads = 1)
  loob <- predict(lmf)$predictions
  write_csv(X, file.path(OUT, "lm_X.csv"))
  write_csv(Y, file.path(OUT, "lm_Y.csv"))
  write_csv(W, file.path(OUT, "lm_W.csv"))
  write_csv(loob, file.path(OUT, "lm_oob.csv"))
  cat(sprintf("[lm] oob dim: %d x %d\n", nrow(loob), ncol(loob)))
}

# ───────────────────────── ll_regression_forest ─────────────────────────
{
  set.seed(91)
  n <- 100; p <- 3
  X <- matrix(rnorm(n * p), n, p)
  Y <- X[, 1] + 0.5 * X[, 2] + 0.1 * X[, 1]^2 + rnorm(n, sd = 0.5)
  llf <- ll_regression_forest(X, Y, enable.ll.split = FALSE,
                                ll.split.lambda = 0.1,
                                num.trees = 50, seed = 1,
                                num.threads = 1)
  llpred <- predict(llf, X)$predictions
  write_csv(X, file.path(OUT, "ll_X.csv"))
  write_vec(Y, file.path(OUT, "ll_Y.csv"))
  write_vec(llpred, file.path(OUT, "ll_pred.csv"))
  cat(sprintf("[ll] pred[1:3]: %s\n",
              paste(formatC(llpred[1:3], digits = 10), collapse = " ")))
}

cat("\nAll fixtures written to:", OUT, "\n")