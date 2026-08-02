#!/usr/bin/env Rscript
# Cross-validation reference for SEM (CFA) vs R lavaan.
#
# Usage: Rscript gen_sem_reference.R

library(lavaan)
library(jsonlite)
set.seed(42)

# ── Generate 2-factor CFA data (6 indicators, 3 per factor) ────────────────
n <- 500
f1 <- rnorm(n, 0, 1)
f2 <- 0.5 * f1 + rnorm(n, 0, 0.87)  # factor correlation ≈ 0.5

x1 <- 1.0 * f1 + rnorm(n, 0, 0.6)   # marker variable (loading = 1)
x2 <- 0.8 * f1 + rnorm(n, 0, 0.6)
x3 <- 0.8 * f1 + rnorm(n, 0, 0.6)
x4 <- 1.0 * f2 + rnorm(n, 0, 0.7)
x5 <- 0.7 * f2 + rnorm(n, 0, 0.7)
x6 <- 0.7 * f2 + rnorm(n, 0, 0.7)

df <- data.frame(x1 = x1, x2 = x2, x3 = x3, x4 = x4, x5 = x5, x6 = x6)

# ── Fit CFA with lavaan ────────────────────────────────────────────────────
model <- '
  f1 =~ x1 + x2 + x3
  f2 =~ x4 + x5 + x6
  f1 ~~ f2
'

fit <- cfa(model, data = df, estimator = "ML")
fit_summary <- summary(fit, standardized = TRUE)

# Extract parameter estimates.
params <- parameterEstimates(fit, standardized = TRUE)

# Loadings.
loadings <- params[params$op == "=~", ]
# Factor covariance.
factor_cov <- params[params$op == "~~" & params$lhs == "f1" & params$rhs == "f2", ]
# Error variances.
error_vars <- params[params$op == "~~" & params$lhs == params$rhs, ]

# Fit indices.
fit_measures <- fitMeasures(fit, c("chisq", "df", "pvalue", "cfi", "rmsea", "srmr", "aic", "bic"))

results <- list(
  loadings = lapply(1:nrow(loadings), function(i) {
    list(
      lhs = loadings$lhs[i], rhs = loadings$rhs[i],
      est = as.numeric(loadings$est[i]),
      se = as.numeric(loadings$se[i]),
      p = as.numeric(loadings$pvalue[i]),
      std = as.numeric(loadings$std.all[i])
    )
  }),
  factor_cov = list(
    est = as.numeric(factor_cov$est),
    se = as.numeric(factor_cov$se),
    p = as.numeric(factor_cov$pvalue),
    std = as.numeric(factor_cov$std.all)
  ),
  error_vars = lapply(1:nrow(error_vars), function(i) {
    list(
      var = error_vars$lhs[i],
      est = as.numeric(error_vars$est[i]),
      se = as.numeric(error_vars$se[i])
    )
  }),
  fit = list(
    chisq = as.numeric(fit_measures["chisq"]),
    df = as.numeric(fit_measures["df"]),
    pvalue = as.numeric(fit_measures["pvalue"]),
    cfi = as.numeric(fit_measures["cfi"]),
    rmsea = as.numeric(fit_measures["rmsea"]),
    srmr = as.numeric(fit_measures["srmr"])
  ),
  n = n
)

write.csv(df, "stat_crates/epi/tests/xval/sem_data.csv", row.names = FALSE)
write_json(results, "stat_crates/epi/tests/xval/sem_reference.json",
           auto_unbox = TRUE, digits = 12, pretty = TRUE)

cat("SEM (CFA) reference written.\n")
cat("  Loadings:\n")
for (i in 1:nrow(loadings)) {
  cat("    ", loadings$lhs[i], "=~", loadings$rhs[i],
      "est=", round(loadings$est[i], 4),
      "std=", round(loadings$std.all[i], 4), "\n")
}
cat("  Factor cov:", round(factor_cov$est, 4),
    "(std:", round(factor_cov$std.all, 4), ")\n")
cat("  Fit: χ²=", round(fit_measures["chisq"], 2),
    "df=", as.integer(fit_measures["df"]),
    "CFI=", round(fit_measures["cfi"], 4),
    "RMSEA=", round(fit_measures["rmsea"], 4),
    "SRMR=", round(fit_measures["srmr"], 4), "\n")
