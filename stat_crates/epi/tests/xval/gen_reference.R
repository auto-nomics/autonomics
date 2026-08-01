#!/usr/bin/env Rscript
# Cross-validation reference: generates synthetic data, runs R reference
# implementations, and writes results as JSON for Rust tests to compare.
#
# Usage: Rscript gen_reference.R

library(jsonlite)

set.seed(42)
n <- 200

# ── Generate synthetic data ────────────────────────────────────────────────
x1 <- rnorm(n, 0, 1)
x2 <- rnorm(n, 2, 1)
x3 <- rbinom(n, 1, 0.3)
eta <- -1 + 0.5 * x1 - 0.3 * x2 + 0.8 * x3
prob <- 1 / (1 + exp(-eta))
y_binary <- rbinom(n, 1, prob)
y_cont <- 2 + 3 * x1 - 1 * x2 + 0.5 * x3 + rnorm(n, 0, 0.5)

df <- data.frame(x1 = x1, x2 = x2, x3 = x3,
                 y_bin = y_binary, y_cont = y_cont)

results <- list()

# ── 1. OLS (lm) ────────────────────────────────────────────────────────────
fit_ols <- lm(y_cont ~ x1 + x2 + x3, data = df)
ols_summary <- summary(fit_ols)
results$ols <- list(
  coefficients = coef(fit_ols),
  std_errors = ols_summary$coefficients[, "Std. Error"],
  t_values = ols_summary$coefficients[, "t value"],
  p_values = ols_summary$coefficients[, "Pr(>|t|)"],
  r_squared = ols_summary$r.squared,
  n = nrow(df)
)

# ── 2. Logistic regression (glm) ──────────────────────────────────────────
fit_glm <- glm(y_bin ~ x1 + x2 + x3, data = df, family = binomial())
glm_summary <- summary(fit_glm)
results$logistic <- list(
  coefficients = coef(fit_glm),
  std_errors = glm_summary$coefficients[, "Std. Error"],
  z_values = glm_summary$coefficients[, "z value"],
  p_values = glm_summary$coefficients[, "Pr(>|z|)"],
  odds_ratios = exp(coef(fit_glm)),
  log_likelihood = logLik(fit_glm)[1],
  null_log_likelihood = logLik(glm(y_bin ~ 1, data = df, family = binomial()))[1],
  n = nobs(fit_glm)
)

# ── 3. Chi-squared test ───────────────────────────────────────────────────
# Create a 2×3 contingency table.
grp <- factor(sample(c("A", "B"), n, replace = TRUE))
cat_var <- factor(sample(c("X", "Y", "Z"), n, replace = TRUE,
                          prob = c(0.4, 0.35, 0.25)))
tab <- table(grp, cat_var)
chi_res <- chisq.test(tab)
results$chisq <- list(
  statistic = chi_res$statistic,
  df = chi_res$parameter,
  p_value = chi_res$p.value,
  counts = lapply(1:nrow(tab), function(i) as.numeric(tab[i,])),
  row_labels = rownames(tab),
  col_labels = colnames(tab)
)

# ── 4. RCS (natural splines via splines::ns + glm) ─────────────────────────
# Use ns() as proxy for rcs() — both are constrained cubic splines.
# We test the spline logistic fit and nonlinearity LR test.
library(splines)
fit_linear <- glm(y_bin ~ x1 + x2, data = df, family = binomial())
fit_ns <- glm(y_bin ~ ns(x1, df = 3) + x2, data = df, family = binomial())
lr_stat <- 2 * (as.numeric(logLik(fit_ns)) - as.numeric(logLik(fit_linear)))
lr_df <- fit_linear$df.residual - fit_ns$df.residual
lr_p <- 1 - pchisq(lr_stat, lr_df)
results$rcs <- list(
  linear_ll = as.numeric(logLik(fit_linear)),
  spline_ll = as.numeric(logLik(fit_ns)),
  lr_stat = lr_stat,
  lr_df = as.numeric(lr_df),
  lr_p_value = lr_p
)

# ── 5. LASSO (glmnet) ──────────────────────────────────────────────────────
# Try glmnet if available; otherwise skip.
has_glmnet <- requireNamespace("glmnet", quietly = TRUE)
if (has_glmnet) {
  library(glmnet)
  X <- as.matrix(df[, c("x1", "x2", "x3")])
  cv_fit <- cv.glmnet(X, df$y_bin, family = "binomial", alpha = 1,
                       nfolds = 10, seed = 42)
  # Get coefficients at lambda.min
  coef_min <- as.numeric(coef(cv_fit, s = "lambda.min"))
  coef_1se <- as.numeric(coef(cv_fit, s = "lambda.1se"))
  results$lasso <- list(
    lambda_min = cv_fit$lambda.min,
    lambda_1se = cv_fit$lambda.1se,
    coef_min = coef_min,
    coef_1se = coef_1se
  )
} else {
  results$lasso <- NULL
  cat("WARNING: glmnet not available, skipping LASSO reference\n")
}

# ── Save data and results ──────────────────────────────────────────────────
write.csv(df, "stat_crates/statkit/tests/xval/data.csv", row.names = FALSE)
write_json(results, "stat_crates/statkit/tests/xval/reference.json",
           auto_unbox = TRUE, digits = 12, pretty = TRUE)

cat("Reference results written to stat_crates/statkit/tests/xval/reference.json\n")
cat("Test data written to stat_crates/statkit/tests/xval/data.csv\n")
