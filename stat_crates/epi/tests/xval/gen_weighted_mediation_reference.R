#!/usr/bin/env Rscript
# Cross-validation reference for survey-weighted causal mediation.
#
# Generates weighted data, fits the two WLS models with lm(weights=), and
# hand-decomposes NDE/NIE/TE at the weighted covariate means for both the
# no-interaction and interaction parameterizations.
#
# Usage (from workspace root):
#   Rscript stat_crates/epi/tests/xval/gen_weighted_mediation_reference.R

library(jsonlite)

set.seed(42)
n <- 250

# ── Generate a weighted NHANES-shaped system ───────────────────────────────
x <- rbinom(n, 1, 0.4)
c1 <- rnorm(n, 60, 12)
c2 <- rbinom(n, 1, 0.3)
w <- exp(rnorm(n, log(5000), 0.4))
m <- 2 + 0.8 * x + 0.05 * c1 + 0.3 * c2 + rnorm(n, 0, 1.5)
y <- 5 + 0.4 * x + 0.9 * m + 0.01 * c1 + 0.5 * c2 + rnorm(n, 0, 2)

df <- data.frame(x = x, m = m, y = y, c1 = c1, c2 = c2, w = w)

decompose <- function(interaction) {
  mm <- lm(m ~ x + c1 + c2, data = df, weights = w)
  ca <- coef(mm)
  m_uc <- ca[["(Intercept)"]] +
    ca[["x"]] * 0 +
    ca[["c1"]] * weighted.mean(df$c1, w) +
    ca[["c2"]] * weighted.mean(df$c2, w)

  ym <- if (interaction) {
    lm(y ~ x + m + x:m + c1 + c2, data = df, weights = w)
  } else {
    lm(y ~ x + m + c1 + c2, data = df, weights = w)
  }
  cb <- coef(ym)
  b1 <- cb[["x"]]
  b2 <- cb[["m"]]
  b3 <- if (interaction) cb[["x:m"]] else 0

  cde <- (b1 + b3 * m_uc) * 1  # x_treated 1 -> x_control 0
  nie <- (b2 + b3 * 1) * ca[["x"]] * 1
  te <- cde + nie

  list(
    alpha_x = as.numeric(ca[["x"]]),
    beta_x = as.numeric(b1),
    beta_m = as.numeric(b2),
    beta_xm = as.numeric(b3),
    m_under_control = as.numeric(m_uc),
    cde = as.numeric(cde),
    nde = as.numeric(cde),
    nie = as.numeric(nie),
    te = as.numeric(te),
    prop_mediated = as.numeric(nie / te)
  )
}

results <- list(
  n = n,
  no_interaction = decompose(FALSE),
  interaction = decompose(TRUE)
)

write.csv(df, "stat_crates/epi/tests/xval/weighted_mediation_data.csv", row.names = FALSE)
write_json(results, "stat_crates/epi/tests/xval/weighted_mediation_reference.json",
           auto_unbox = TRUE, digits = 12, pretty = TRUE)

cat("weighted mediation reference written to weighted_mediation_reference.json\n")
cat("  no_interaction: nie =", round(results$no_interaction$nie, 4),
    ", te =", round(results$no_interaction$te, 4), "\n")
cat("  interaction:    nie =", round(results$interaction$nie, 4),
    ", te =", round(results$interaction$te, 4), "\n")
