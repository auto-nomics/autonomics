#!/usr/bin/env Rscript
# Cross-validation reference for survey-weighted serial mediation.
#
# Generates weighted data for an X -> M1 -> M2 -> Y chain, fits the three
# WLS models with lm(weights=), and hand-decomposes the paths:
#   ie_m1 = a1*b1, ie_m2 = a2*b2, ie_serial = a1*d21*b2
#   direct = c', te = direct + total_indirect
#
# Usage (from workspace root):
#   Rscript stat_crates/epi/tests/xval/gen_serial_mediation_reference.R

library(jsonlite)

set.seed(42)
n <- 250

# ── Generate a weighted serial system ───────────────────────────────────────
x <- rbinom(n, 1, 0.4)
c1 <- rnorm(n, 55, 10)
w <- exp(rnorm(n, log(6000), 0.35))
m1 <- 1 + 0.7 * x + 0.03 * c1 + rnorm(n, 0, 1.5)
m2 <- 3 + 0.2 * x + 0.5 * m1 + 0.01 * c1 + rnorm(n, 0, 1.2)
y <- 2 + 0.3 * x + 0.6 * m1 + 0.8 * m2 + 0.04 * c1 + rnorm(n, 0, 2)

df <- data.frame(x = x, m1 = m1, m2 = m2, y = y, c1 = c1, w = w)

# ── Three-model chain (all WLS, same weights) ───────────────────────────────
mm1 <- lm(m1 ~ x + c1, data = df, weights = w)
cm1 <- coef(mm1)

mm2 <- lm(m2 ~ x + m1 + c1, data = df, weights = w)
cm2 <- coef(mm2)

ym <- lm(y ~ x + m1 + m2 + c1, data = df, weights = w)
cy <- coef(ym)

a1 <- cm1[["x"]]
a2 <- cm2[["x"]]
d21 <- cm2[["m1"]]
c_prime <- cy[["x"]]
b1 <- cy[["m1"]]
b2 <- cy[["m2"]]

ie_m1 <- a1 * b1
ie_m2 <- a2 * b2
ie_serial <- a1 * d21 * b2
total_indirect <- ie_m1 + ie_m2 + ie_serial
direct <- c_prime
te <- direct + total_indirect

results <- list(
  n = n,
  a1 = as.numeric(a1),
  a2 = as.numeric(a2),
  d21 = as.numeric(d21),
  b1 = as.numeric(b1),
  b2 = as.numeric(b2),
  c_prime = as.numeric(c_prime),
  ie_m1 = as.numeric(ie_m1),
  ie_m2 = as.numeric(ie_m2),
  ie_serial = as.numeric(ie_serial),
  total_indirect = as.numeric(total_indirect),
  direct = as.numeric(direct),
  te = as.numeric(te),
  prop_mediated = as.numeric(total_indirect / te),
  prop_serial = as.numeric(ie_serial / te)
)

write.csv(df, "stat_crates/epi/tests/xval/serial_mediation_data.csv", row.names = FALSE)
write_json(results, "stat_crates/epi/tests/xval/serial_mediation_reference.json",
           auto_unbox = TRUE, digits = 12, pretty = TRUE)

cat("serial mediation reference written to serial_mediation_reference.json\n")
cat("  ie_serial =", round(ie_serial, 4),
    ", total_indirect =", round(total_indirect, 4),
    ", te =", round(te, 4), "\n")
