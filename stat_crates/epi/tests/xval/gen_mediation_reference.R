#!/usr/bin/env Rscript
# Cross-validation reference for causal mediation analysis.
# Uses R lm() for both models and computes NDE/NIE/TE manually
# (matching the VanderWeele two-model decomposition).
#
# Usage: Rscript gen_mediation_reference.R

library(jsonlite)

set.seed(42)
n <- 500

# ── Generate data with known mediation structure ───────────────────────────
x <- rbinom(n, 1, 0.5)                           # binary exposure
c1 <- rnorm(n, 0, 1)                              # confounder
m <- 1.0 + 0.5 * x + 0.3 * c1 + rnorm(n, 0, 0.5) # mediator: α₁=0.5
y <- 0.3 * x + 0.8 * m - 0.2 * c1 + rnorm(n, 0, 0.3)  # outcome: β₁=0.3, β₂=0.8

df <- data.frame(x = x, m = m, y = y, c1 = c1)

# ── Fit models ─────────────────────────────────────────────────────────────
# Mediator model: M ~ X + C
fit_m <- lm(m ~ x + c1, data = df)
alpha_0 <- coef(fit_m)[1]  # intercept
alpha_1 <- coef(fit_m)[2]  # X coefficient

# Outcome model: Y ~ X + M + C (no interaction)
fit_y <- lm(y ~ x + m + c1, data = df)
beta_1 <- coef(fit_y)[2]   # X direct effect
beta_2 <- coef(fit_y)[3]   # M effect
beta_3 <- 0                 # no interaction

# ── Decomposition (no interaction) ─────────────────────────────────────────
nde <- beta_1
nie <- beta_2 * alpha_1
te  <- nde + nie
prop_mediated <- nie / te

results <- list(
  alpha_0 = as.numeric(alpha_0),
  alpha_1 = as.numeric(alpha_1),
  beta_1 = as.numeric(beta_1),
  beta_2 = as.numeric(beta_2),
  nde = as.numeric(nde),
  nie = as.numeric(nie),
  te = as.numeric(te),
  prop_mediated = as.numeric(prop_mediated),
  n = n
)

write.csv(df, "stat_crates/epi/tests/xval/mediation_data.csv", row.names = FALSE)
write_json(results, "stat_crates/epi/tests/xval/mediation_reference.json",
           auto_unbox = TRUE, digits = 12, pretty = TRUE)

cat("Mediation reference written.\n")
cat("  α₁ =", round(alpha_1, 4), ", β₁ =", round(beta_1, 4), ", β₂ =", round(beta_2, 4), "\n")
cat("  NDE =", round(nde, 4), ", NIE =", round(nie, 4), ", TE =", round(te, 4), "\n")
cat("  Prop mediated =", round(prop_mediated, 4), "\n")
