#!/usr/bin/env Rscript
# Cross-validation reference for Phase 2: IPTW + CLPM.
# Usage: Rscript gen_phase2_reference.R

library(jsonlite)
set.seed(42)

# ── IPTW data ──────────────────────────────────────────────────────────────
n <- 500
c1 <- rnorm(n, 0, 1)
c2 <- rnorm(n, 0, 1)
true_te <- 1.5
ps_true <- 1 / (1 + exp(-(-0.5 + 0.8 * c1 - 0.3 * c2)))
treatment <- rbinom(n, 1, ps_true)
outcome <- true_te * treatment + 2 * c1 + 1 * c2 + rnorm(n, 0, 1)

iptw_df <- data.frame(treatment = treatment, outcome = outcome, c1 = c1, c2 = c2)

# IPTW: logistic PS model + stabilized weights + WLS.
ps_model <- glm(treatment ~ c1 + c2, data = iptw_df, family = binomial())
ps <- fitted(ps_model)
p_bar <- mean(treatment)
sw <- treatment * p_bar / ps + (1 - treatment) * (1 - p_bar) / (1 - ps)
iptw_fit <- lm(outcome ~ treatment, data = iptw_df, weights = sw)
ate_r <- coef(iptw_fit)["treatment"]

# ── CLPM data ──────────────────────────────────────────────────────────────
x1 <- rnorm(n, 0, 1)
y1 <- rnorm(n, 0, 1)
x2 <- 0.4 * x1 + 0.2 * y1 + rnorm(n, 0, 0.5)
y2 <- 0.5 * y1 + 0.3 * x1 + rnorm(n, 0, 0.5)

clpm_df <- data.frame(x1 = x1, y1 = y1, x2 = x2, y2 = y2)

# CLPM: two OLS regressions.
fit_y2 <- lm(y2 ~ y1 + x1, data = clpm_df)
fit_x2 <- lm(x2 ~ x1 + y1, data = clpm_df)

results <- list(
  iptw = list(
    ate = as.numeric(ate_r),
    n = n
  ),
  clpm = list(
    ar_y = as.numeric(coef(fit_y2)["y1"]),
    cross_xy = as.numeric(coef(fit_y2)["x1"]),
    ar_x = as.numeric(coef(fit_x2)["x1"]),
    cross_yx = as.numeric(coef(fit_x2)["y1"]),
    n = n
  )
)

write.csv(iptw_df, "stat_crates/epi/tests/xval/iptw_data.csv", row.names = FALSE)
write.csv(clpm_df, "stat_crates/epi/tests/xval/clpm_data.csv", row.names = FALSE)
write_json(results, "stat_crates/epi/tests/xval/phase2_reference.json",
           auto_unbox = TRUE, digits = 12, pretty = TRUE)

cat("Phase 2 reference written.\n")
cat("  IPTW ATE =", round(ate_r, 4), "\n")
cat("  CLPM AR(Y) =", round(coef(fit_y2)["y1"], 4), ", cross(X→Y) =", round(coef(fit_y2)["x1"], 4), "\n")
cat("  CLPM AR(X) =", round(coef(fit_x2)["x1"], 4), ", cross(Y→X) =", round(coef(fit_x2)["y1"], 4), "\n")
