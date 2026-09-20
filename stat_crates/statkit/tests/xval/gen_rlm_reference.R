#!/usr/bin/env Rscript
# Cross-validation reference for robust linear regression (M-estimation).
#
# Generates weighted data with gross outliers, fits MASS::rlm with Huber and
# Tukey psi (wt.method = "case", scale.est = "MAD"), and saves both data and
# results as JSON for the Rust xval_rlm test.
#
# Usage (from workspace root): Rscript stat_crates/statkit/tests/xval/gen_rlm_reference.R

library(MASS)
library(jsonlite)

set.seed(42)
n <- 150

# ── Generate covariates and NHANES-style case weights ──────────────────────
x1 <- rnorm(n, 2, 1)
x2 <- rnorm(n, 0, 2)
x3 <- rbinom(n, 1, 0.4)
w <- round(exp(rnorm(n, log(8000), 0.35)))
eps <- rnorm(n, 0, 1.5)
y <- 0.5 + 1.2 * x1 - 0.7 * x2 + 0.9 * x3 + eps

# Five gross outliers so robustness visibly bites.
idx <- sample(n, 5)
y[idx] <- y[idx] + c(25, -30, 40, -20, 35)

df <- data.frame(x1 = x1, x2 = x2, x3 = x3, y = y, w = w)

# ── Fit both psi families, tightly converged ────────────────────────────────
fit_one <- function(psi) {
  fit <- rlm(y ~ x1 + x2 + x3, data = df, weights = w,
             wt.method = "case", psi = psi, scale.est = "MAD",
             maxit = 100, acc = 1e-8)
  s <- summary(fit)
  list(
    coefficients = as.numeric(coef(fit)),
    std_errors = as.numeric(s$coefficients[, "Std. Error"]),
    scale = as.numeric(fit$s),
    crit = as.numeric(tail(fit$conv, 1)),
    n_iter = length(fit$conv)
  )
}

hub <- fit_one(psi.huber)
tuk <- fit_one(psi.bisquare)

results <- list(n = n, huber = hub, tukey = tuk)

# ── Save data and results ───────────────────────────────────────────────────
write.csv(df, "stat_crates/statkit/tests/xval/rlm_data.csv", row.names = FALSE)
write_json(results, "stat_crates/statkit/tests/xval/rlm_reference.json",
           auto_unbox = TRUE, digits = 12, pretty = TRUE)

cat("rlm reference results written to rlm_reference.json\n")
cat("  n =", n, ", outlier indices:", sort(idx), "\n")
cat("  huber coefficients:", round(hub$coefficients, 4), "\n")
cat("  tukey coefficients:", round(tuk$coefficients, 4), "\n")
