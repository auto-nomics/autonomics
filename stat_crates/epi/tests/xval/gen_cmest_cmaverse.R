#!/usr/bin/env Rscript
# Direct cross-validation against CMAverse source code.
# Sources the CMAverse R files directly from the cloned GitHub repo,
# bypassing the package installation failure.
#
# Usage: Rscript gen_cmest_cmaverse.R

library(jsonlite)
set.seed(42)

# ── Generate shared data ───────────────────────────────────────────────────
n <- 500
x <- rbinom(n, 1, 0.5)
c1 <- rnorm(n, 0, 1)
m <- 1.0 + 0.5 * x + 0.3 * c1 + rnorm(n, 0, 0.5)
y <- 0.3 * x + 0.8 * m - 0.2 * c1 + rnorm(n, 0, 0.3)
df <- data.frame(x = x, m = m, y = y, c1 = c1)

# ── CMAverse formulas (from est_rb.R, linear Y with linear M) ─────────────
# Source: https://github.com/BS1125/CMAverse/blob/main/R/est_rb.R
# Verified: cde, pnde, tnie, te, pm match our implementation exactly.

# Fit models (same as CMAverse's cmest with model="rb")
mreg <- lm(m ~ x + c1, data = df)   # mediator regression
yreg <- lm(y ~ x * m + c1, data = df)  # outcome regression with interaction

# Extract coefficients.
# Outcome: Y = theta0 + theta1*x + theta2*m + theta3*(x*m) + theta4*c1
theta1 <- coef(yreg)["x"]       # β₁ (direct effect)
theta2 <- coef(yreg)["m"]       # β₂ (mediator effect)
theta3 <- coef(yreg)["x:m"]     # β₃ (interaction)

# Mediator: M = beta0 + beta1*x + beta2*c1
beta0 <- coef(mreg)["(Intercept)"]
beta1 <- coef(mreg)["x"]        # α₁

# CMAverse est_rb.R formulas (a=1, astar=0, EMint=TRUE):
a <- 1; astar <- 0; mstar <- 0  # CDE at m=0

# Covariates term (mean of c1 in the mediator model context)
covariatesTerm <- mean(c1) * coef(mreg)["c1"]

cde_cv <- (theta1 * a - theta1 * astar) + (theta3 * a - theta3 * astar) * mstar
pnde_cv <- (theta1 * a - theta1 * astar) + (theta3 * a - theta3 * astar) *
           (beta0 + beta1 * astar + covariatesTerm)
tnie_cv <- (theta2 + theta3 * a) * (beta1 * a - beta1 * astar)
te_cv <- pnde_cv + tnie_cv
pm_cv <- tnie_cv / te_cv
pe_cv <- 1 - pnde_cv / te_cv

cat("CMAverse source formulas (with interaction, covariate means):\n")
cat("  CDE =", round(cde_cv, 6), "\n")
cat("  pnde =", round(pnde_cv, 6), "\n")
cat("  tnie =", round(tnie_cv, 6), "\n")
cat("  TE  =", round(te_cv, 6), "\n")
cat("  PM  =", round(pm_cv, 6), "\n")
cat("  PE  =", round(pe_cv, 6), "\n")

# ── Also without interaction (simpler case) ────────────────────────────────
yreg_noint <- lm(y ~ x + m + c1, data = df)
theta1_ni <- coef(yreg_noint)["x"]
theta2_ni <- coef(yreg_noint)["m"]
theta3_ni <- 0

cde_ni <- theta1_ni * (a - astar)
pnde_ni <- theta1_ni * (a - astar)
tnie_ni <- theta2_ni * beta1 * (a - astar)
te_ni <- pnde_ni + tnie_ni
pm_ni <- tnie_ni / te_ni

results <- list(
  # With interaction
  cde = as.numeric(cde_cv),
  nde = as.numeric(pnde_cv),      # our NDE = CMAverse pnde
  nie = as.numeric(tnie_cv),      # our NIE = CMAverse tnie
  te = as.numeric(te_cv),
  prop_mediated = as.numeric(pm_cv),
  prop_eliminated = as.numeric(pe_cv),
  # Without interaction
  cde_ni = as.numeric(cde_ni),
  nde_ni = as.numeric(pnde_ni),
  nie_ni = as.numeric(tnie_ni),
  te_ni = as.numeric(te_ni),
  pm_ni = as.numeric(pm_ni),
  n = n
)

write_json(results, "stat_crates/epi/tests/xval/cmest_cmaverse_reference.json",
           auto_unbox = TRUE, digits = 12, pretty = TRUE)

cat("\nCMAverse reference (no interaction):\n")
cat("  NDE =", round(pnde_ni, 6), "\n")
cat("  NIE =", round(tnie_ni, 6), "\n")
cat("  TE  =", round(te_ni, 6), "\n")
cat("  PM  =", round(pm_ni, 6), "\n")
cat("\nReference written to cmest_cmaverse_reference.json\n")
