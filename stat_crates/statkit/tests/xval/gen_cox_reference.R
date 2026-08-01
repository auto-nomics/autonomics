#!/usr/bin/env Rscript
# Cross-validation reference for Cox PH regression.
# Generates survival data, fits coxph with Breslow ties, saves results as JSON.
#
# Usage: Rscript gen_cox_reference.R

library(survival)
library(jsonlite)

set.seed(42)
n <- 200

# ── Generate covariates ────────────────────────────────────────────────────
x1 <- rnorm(n, 0, 1)
x2 <- rnorm(n, 2, 1)
x3 <- rbinom(n, 1, 0.3)

# ── Generate survival times (exponential baseline) ────────────────────────
beta_true <- c(0.5, -0.3, 0.8)
eta <- 0.5 * x1 - 0.3 * x2 + 0.8 * x3
lambda_base <- 0.1
u <- runif(n)
time_true <- -log(u) / (lambda_base * exp(eta))

# ── Random censoring ──────────────────────────────────────────────────────
censor_time <- runif(n, 0, 60)
obs_time <- pmin(time_true, censor_time)
event <- as.numeric(time_true <= censor_time)

df <- data.frame(x1 = x1, x2 = x2, x3 = x3,
                 time = obs_time, event = event)

# ── Fit Cox model (Breslow ties to match our implementation) ───────────────
fit <- coxph(Surv(time, event) ~ x1 + x2 + x3, data = df, ties = "breslow")
fit_summary <- summary(fit)

results <- list(
  coefficients = as.numeric(coef(fit)),
  std_errors = as.numeric(fit_summary$coefficients[, "se(coef)"]),
  z_values = as.numeric(fit_summary$coefficients[, "z"]),
  p_values = as.numeric(fit_summary$coefficients[, "Pr(>|z|)"]),
  hazard_ratios = as.numeric(exp(coef(fit))),
  hr_ci_lower = as.numeric(exp(coef(fit) - qnorm(0.975) * fit_summary$coefficients[, "se(coef)"])),
  hr_ci_upper = as.numeric(exp(coef(fit) + qnorm(0.975) * fit_summary$coefficients[, "se(coef)"])),
  log_likelihood = as.numeric(fit$loglik[2]),     # full model
  null_log_likelihood = as.numeric(fit$loglik[1]), # null model
  concordance = as.numeric(fit_summary$concordance[1]),
  n = as.numeric(fit$n),
  n_events = as.numeric(fit$nevent)
)

# ── Save data and results ──────────────────────────────────────────────────
write.csv(df, "stat_crates/statkit/tests/xval/cox_data.csv", row.names = FALSE)
write_json(results, "stat_crates/statkit/tests/xval/cox_reference.json",
           auto_unbox = TRUE, digits = 12, pretty = TRUE)

cat("Cox reference results written to cox_reference.json\n")
cat("  n =", n, ", events =", sum(event), "\n")
cat("  coefficients:", round(coef(fit), 4), "\n")
cat("  HR:", round(exp(coef(fit)), 4), "\n")
cat("  C-index:", round(fit_summary$concordance[1], 4), "\n")
