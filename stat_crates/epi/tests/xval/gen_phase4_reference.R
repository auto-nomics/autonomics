#!/usr/bin/env Rscript
# Cross-validation reference for Phase 4: Competing Risk (CIF).
# Uses R cmprsk::cuminc for CIF estimation.
#
# Usage: Rscript gen_phase4_reference.R

library(cmprsk)
library(jsonlite)
set.seed(42)

n <- 300

# ── Generate competing-risks data ──────────────────────────────────────────
# Two causes: 1 (event of interest) and 2 (competing).
x1 <- rnorm(n, 0, 1)
x2 <- rnorm(n, 0, 1)

# Cause 1 hazard influenced by x1; cause 2 by x2.
eta1 <- 0.5 * x1
eta2 <- 0.3 * x2

# Exponential times for each cause.
u <- runif(n)
hazard_sum <- 0.1 * exp(eta1) + 0.1 * exp(eta2)
time_true <- -log(u) / hazard_sum

# Determine which cause fires first.
p1 <- 0.1 * exp(eta1) / hazard_sum
cause <- ifelse(runif(n) < p1, 1, 2)

# Censoring.
censor_time <- runif(n, 0, 40)
obs_time <- pmin(time_true, censor_time)
obs_event <- ifelse(time_true <= censor_time, cause, 0)

df <- data.frame(time = obs_time, event = obs_event, x1 = x1, x2 = x2)

# ── CIF via cmprsk::cuminc ─────────────────────────────────────────────────
ci_fit <- cuminc(df$time, df$event, cencode = 0)

# cuminc returns a list indexed by cause: [[1]] = cause 1, [[2]] = cause 2.
# Each element has $time, $est (CIF estimate), $var.
cif1_data <- ci_fit[[1]]
cif1_times <- cif1_data$time
cif1 <- cif1_data$est

cif2_data <- ci_fit[[2]]
cif2_times <- if (!is.null(cif2_data)) cif2_data$time else numeric(0)
cif2 <- if (!is.null(cif2_data)) cif2_data$est else numeric(0)

last_cif1 <- if (length(cif1) > 0) tail(cif1, 1) else NA
last_cif2 <- if (length(cif2) > 0) tail(cif2, 1) else 0

results <- list(
  cif1_times = as.numeric(cif1_times),
  cif1 = as.numeric(cif1),
  cif1_last = as.numeric(last_cif1),
  cif2_last = as.numeric(last_cif2),
  n = n,
  n_cause1 = sum(df$event == 1),
  n_cause2 = sum(df$event == 2),
  n_censored = sum(df$event == 0)
)

write.csv(df, "stat_crates/epi/tests/xval/competing_risk_data.csv", row.names = FALSE)
write_json(results, "stat_crates/epi/tests/xval/phase4_reference.json",
           auto_unbox = TRUE, digits = 12, pretty = TRUE)

cat("Phase 4 CIF reference written.\n")
cat("  n =", n, ", cause 1 =", sum(df$event == 1),
    ", cause 2 =", sum(df$event == 2),
    ", censored =", sum(df$event == 0), "\n")
cat("  CIF1(last) =", round(as.numeric(last_cif1), 4), "\n")
cat("  CIF2(last) =", round(as.numeric(last_cif2), 4), "\n")
