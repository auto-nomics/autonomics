#!/usr/bin/env Rscript
# Cross-validation reference for CMAverse-compatible cmest.
# Since CMAverse isn't installable on R 4.5, we manually implement
# the Valeri/VanderWeele regression-based decomposition — mathematically
# identical to cmest(method="rb").
#
# Usage: Rscript gen_cmest_reference.R

library(jsonlite)
set.seed(42)

n <- 500
x <- rbinom(n, 1, 0.5)           # binary exposure
c1 <- rnorm(n, 0, 1)              # confounder
m <- 1.0 + 0.5 * x + 0.3 * c1 + rnorm(n, 0, 0.5)
y <- 0.3 * x + 0.8 * m - 0.2 * c1 + rnorm(n, 0, 0.3)

df <- data.frame(x = x, m = m, y = y, c1 = c1)

# ── Mediator model: M ~ X + C ──────────────────────────────────────────────
fit_m <- lm(m ~ x + c1, data = df)
alpha_0 <- coef(fit_m)[1]  # intercept
alpha_1 <- coef(fit_m)[2]  # X coefficient

# ── Outcome model: Y ~ X + M + C (no interaction) ──────────────────────────
fit_y <- lm(y ~ x + m + c1, data = df)
beta_1 <- coef(fit_y)[2]   # X direct effect
beta_2 <- coef(fit_y)[3]   # M effect
beta_3 <- 0                # no interaction

# ── Decomposition (Valeri & VanderWeele 2015) ──────────────────────────────
# Without interaction:
# CDE(m) = beta_1 + beta_3 * m = beta_1 (since beta_3 = 0)
# NDE = beta_1 + beta_3 * E[M|X=0] = beta_1
# NIE = (beta_2 + beta_3) * alpha_1 = beta_2 * alpha_1
# TE = NDE + NIE

cde <- beta_1 + beta_3 * 0.0  # CDE at m=0
nde <- beta_1 + beta_3 * alpha_0
nie <- (beta_2 + beta_3) * alpha_1
te <- nde + nie
prop_mediated <- nie / te
prop_eliminated <- 1 - nde / te

results <- list(
  cde = as.numeric(cde),
  nde = as.numeric(nde),
  nie = as.numeric(nie),
  te = as.numeric(te),
  prop_mediated = as.numeric(prop_mediated),
  prop_eliminated = as.numeric(prop_eliminated),
  alpha_1 = as.numeric(alpha_1),
  beta_1 = as.numeric(beta_1),
  beta_2 = as.numeric(beta_2),
  n = n
)

write.csv(df, "stat_crates/epi/tests/xval/cmest_data.csv", row.names = FALSE)
write_json(results, "stat_crates/epi/tests/xval/cmest_reference.json",
           auto_unbox = TRUE, digits = 12, pretty = TRUE)

cat("CMAverse cmest reference written.\n")
cat("  CDE =", round(cde, 4), "\n")
cat("  NDE =", round(nde, 4), "\n")
cat("  NIE =", round(nie, 4), "\n")
cat("  TE  =", round(te, 4), "\n")
cat("  PM  =", round(prop_mediated, 4), "\n")
cat("  PE  =", round(prop_eliminated, 4), "\n")
