#!/usr/bin/env Rscript
# Cross-validation reference for survey-weighted moderated mediation.
#
# Generates weighted data with a continuous moderator W, then hand-
# decomposes the conditional indirect effects and the Hayes (2015) index
# of moderated mediation for both the first-stage (PROCESS model 7) and
# second-stage (model 14) parameterizations, using lm(weights=).
#
# Usage (from workspace root):
#   Rscript stat_crates/epi/tests/xval/gen_moderated_mediation_reference.R

library(jsonlite)

set.seed(42)
n <- 250

# ── Generate a weighted moderated system ────────────────────────────────────
x <- rbinom(n, 1, 0.4)
wm <- rnorm(n, 50, 15)      # moderator (e.g. age)
c1 <- rnorm(n, 60, 12)
wt <- exp(rnorm(n, log(5000), 0.4))
m <- 1 + 0.5 * x + 0.06 * wm + 0.012 * x * wm + 0.03 * c1 + rnorm(n, 0, 1.5)
y <- 4 + 0.3 * x + 0.8 * m + 0.02 * wm + 0.006 * m * wm + 0.01 * c1 + rnorm(n, 0, 2)

df <- data.frame(x = x, m = m, wm = wm, y = y, c1 = c1, w = wt)

# ── Moderator grid: weighted mean ± weighted population SD ─────────────────
mu <- weighted.mean(wm, wt)
sd_w <- sqrt(sum(wt * (wm - mu)^2) / sum(wt))
grid <- c(mu - sd_w, mu, mu + sd_w)

# ── First stage: X:W in the mediator model (PROCESS model 7) ───────────────
decompose_first <- function() {
  mm <- lm(m ~ x + wm + x:wm + c1, data = df, weights = w)
  ca <- coef(mm)
  ym <- lm(y ~ x + m + wm + c1, data = df, weights = w)
  cb <- coef(ym)

  a_x <- ca[["x"]]
  a_xw <- ca[["x:wm"]]
  b_m <- cb[["m"]]
  c_x <- cb[["x"]]

  a_path <- a_x + a_xw * grid
  b_path <- rep(b_m, length(grid))

  list(
    a_x = as.numeric(a_x),
    a_xw = as.numeric(a_xw),
    b_m = as.numeric(b_m),
    c_x = as.numeric(c_x),
    index_first_stage = as.numeric(a_xw * b_m),
    w = as.numeric(grid),
    a_path = as.numeric(a_path),
    b_path = as.numeric(b_path),
    indirect = as.numeric(a_path * b_path),
    direct = as.numeric(rep(c_x, length(grid)))
  )
}

# ── Second stage: M:W in the outcome model (PROCESS model 14) ──────────────
decompose_second <- function() {
  mm <- lm(m ~ x + wm + c1, data = df, weights = w)
  ca <- coef(mm)
  ym <- lm(y ~ x + m + wm + m:wm + c1, data = df, weights = w)
  cb <- coef(ym)

  a_x <- ca[["x"]]
  b_m <- cb[["m"]]
  b_mw <- cb[["m:wm"]]
  c_x <- cb[["x"]]

  a_path <- rep(a_x, length(grid))
  b_path <- b_m + b_mw * grid

  list(
    a_x = as.numeric(a_x),
    b_m = as.numeric(b_m),
    b_mw = as.numeric(b_mw),
    c_x = as.numeric(c_x),
    index_second_stage = as.numeric(a_x * b_mw),
    w = as.numeric(grid),
    a_path = as.numeric(a_path),
    b_path = as.numeric(b_path),
    indirect = as.numeric(a_path * b_path),
    direct = as.numeric(rep(c_x, length(grid)))
  )
}

results <- list(
  n = n,
  w_grid = as.numeric(grid),
  first = decompose_first(),
  second = decompose_second()
)

write.csv(df, "stat_crates/epi/tests/xval/moderated_mediation_data.csv", row.names = FALSE)
write_json(results, "stat_crates/epi/tests/xval/moderated_mediation_reference.json",
           auto_unbox = TRUE, digits = 12, pretty = TRUE)

cat("moderated mediation reference written to moderated_mediation_reference.json\n")
cat("  grid:", round(grid, 3), "\n")
cat("  first:  index =", round(results$first$index_first_stage, 5), "\n")
cat("  second: index =", round(results$second$index_second_stage, 5), "\n")
