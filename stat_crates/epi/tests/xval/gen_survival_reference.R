#!/usr/bin/env Rscript
# Cross-validation reference for Kaplan-Meier + Log-rank test.
#
# Usage: Rscript gen_survival_reference.R

library(survival)
library(jsonlite)

set.seed(42)
n <- 200

# ── Generate two-group survival data ───────────────────────────────────────
group <- rep(c(0, 1), each = n / 2)
x1 <- rnorm(n, 0, 1)

# Group 1 has higher hazard (treatment effect).
eta <- 0.3 * x1 + 0.5 * group
lambda_base <- 0.1
u <- runif(n)
time_true <- -log(u) / (lambda_base * exp(eta))
censor_time <- runif(n, 0, 60)
obs_time <- pmin(time_true, censor_time)
event <- as.numeric(time_true <= censor_time)

df <- data.frame(time = obs_time, event = event, group = group, x1 = x1)

# ── 1. Kaplan-Meier (overall) ──────────────────────────────────────────────
km_fit <- survfit(Surv(time, event) ~ 1, data = df)
# Filter to event times only (n.event > 0) for comparison with our implementation.
has_event <- km_fit$n.event > 0
km_times <- km_fit$time[has_event]
km_surv <- km_fit$surv[has_event]
km_se <- km_fit$std.err[has_event]
km_se[is.infinite(km_se)] <- NA  # Inf at last point → null in JSON
km_n_risk <- km_fit$n.risk[has_event]
km_n_event <- km_fit$n.event[has_event]

# ── 2. Log-rank test ───────────────────────────────────────────────────────
lr_fit <- survdiff(Surv(time, event) ~ group, data = df)

results <- list(
  km = list(
    times = as.numeric(km_times),
    survival = as.numeric(km_surv),
    std_error = as.numeric(km_se),
    n_at_risk = as.numeric(km_n_risk),
    n_events = as.numeric(km_n_event),
    n_obs = as.numeric(km_fit$n),
    n_censored = as.numeric(km_fit$n - km_fit$n.event)
  ),
  logrank = list(
    chi_squared = as.numeric(lr_fit$chisq),
    df = as.numeric(length(lr_fit$n) - 1),
    p_value = as.numeric(1 - pchisq(lr_fit$chisq, length(lr_fit$n) - 1)),
    n_groups = as.numeric(length(lr_fit$n)),
    observed = as.numeric(lr_fit$obs),
    expected = as.numeric(lr_fit$exp),
    n = as.numeric(lr_fit$n)
  )
)

write.csv(df, "stat_crates/epi/tests/xval/survival_data.csv", row.names = FALSE)
write_json(results, "stat_crates/epi/tests/xval/survival_reference.json",
           auto_unbox = TRUE, digits = 12, pretty = TRUE)

cat("Survival reference written.\n")
cat("  KM: ", length(km_times), " event times\n", sep = "")
cat("  Log-rank: χ²=", round(lr_fit$chisq, 4),
    ", df=", length(lr_fit$n) - 1,
    ", p=", round(1 - pchisq(lr_fit$chisq, length(lr_fit$n) - 1), 4), "\n", sep = "")
