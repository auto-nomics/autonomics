#!/usr/bin/env Rscript
# Generate golden references for rdmulti, rddensity, rdlocrand
# using reproducible synthetic data.

suppressWarnings({
  library(rdrobust)
  library(rdmulti)
  # Source rddensity and rdlocrand directly (packages not installed)
  # rddensity needs lpdensity — try sourcing just the core test functions
  # rdlocrand needs AER/sandwich — source directly
})

set.seed(42)

# =====================================================================
# Shared synthetic data
# =====================================================================
n <- 1000
X <- runif(n, 0, 100)

# --- rdmulti: rdmc with 2 cutoffs ---
cat("=== rdmc ===\n")
C_mc <- c(rep(33, 500), rep(66, 500))
Y_mc <- (1 + X + (X >= C_mc)) * (C_mc == 33) +
        (0.5 + 0.5 * X + 0.8 * (X >= C_mc)) * (C_mc == 66) +
        rnorm(n)

result_mc <- rdmc(Y_mc, X, C_mc, level = 95)

cat("pooled_tau_cl:", sprintf("%.15e", as.numeric(result_mc$tau)), "\n")
cat("pooled_se_rb:", sprintf("%.15e", as.numeric(result_mc$se.rb)), "\n")
cat("weighted_tau_bc:", sprintf("%.15e", as.numeric(result_mc$B["weighted"])), "\n")
cat("weighted_se_rb_sq:", sprintf("%.15e", as.numeric(result_mc$V["weighted"])), "\n")
for (k in 1:2) {
  cat("cutoff", k, "_tau_cl:", sprintf("%.15e", as.numeric(result_mc$Coefs[k])), "\n")
  cat("cutoff", k, "_tau_bc:", sprintf("%.15e", as.numeric(result_mc$B[k])), "\n")
  cat("cutoff", k, "_se_rb_sq:", sprintf("%.15e", as.numeric(result_mc$V[k])), "\n")
  cat("cutoff", k, "_n_h:", as.numeric(result_mc$Nh[1,k] + result_mc$Nh[2,k]), "\n")
}

# =====================================================================
# rddensity: source functions directly
# =====================================================================
cat("\n=== rddensity ===\n")
# Source the rddensity internal functions
source_files <- c(
  "reference/rddensity/R/rddensity/R/rddensity_fun.R",
  "reference/rddensity/R/rddensity/R/rdbwdensity.R",
  "reference/rddensity/R/rddensity/R/rddensity.R"
)
for (f in source_files) {
  suppressPackageStartupMessages(source(f))
}

# Generate data with density discontinuity
set.seed(42)
x_density <- rnorm(2000, mean = -0.5)
# Add discontinuity
x_density[x_density > 0] <- x_density[x_density > 0] * 2

rdd <- rddensity(X = x_density, vce = "jackknife")

cat("hat_left:", sprintf("%.15e", rdd$hat$left), "\n")
cat("hat_right:", sprintf("%.15e", rdd$hat$right), "\n")
cat("hat_diff:", sprintf("%.15e", rdd$hat$diff), "\n")
cat("sd_jk_left:", sprintf("%.15e", rdd$sd_jk$left), "\n")
cat("sd_jk_right:", sprintf("%.15e", rdd$sd_jk$right), "\n")
cat("sd_jk_diff:", sprintf("%.15e", rdd$sd_jk$diff), "\n")
cat("t_jk:", sprintf("%.15e", rdd$test$t_jk), "\n")
cat("p_jk:", sprintf("%.15e", rdd$test$p_jk), "\n")
cat("h_left:", sprintf("%.15e", rdd$h$left), "\n")
cat("h_right:", sprintf("%.15e", rdd$h$right), "\n")
cat("N:", rdd$N$full, "\n")
cat("N_left:", rdd$N$left, "\n")
cat("N_right:", rdd$N$right, "\n")

# Continuous density (no discontinuity)
cat("\n--- continuous density ---\n")
set.seed(42)
x_cont <- rnorm(2000, mean = -0.5)
rdd2 <- rddensity(X = x_cont, vce = "jackknife")
cat("cont_t_jk:", sprintf("%.15e", rdd2$test$t_jk), "\n")
cat("cont_p_jk:", sprintf("%.15e", rdd2$test$p_jk), "\n")

# =====================================================================
# rdlocrand: source functions directly
# =====================================================================
cat("\n=== rdlocrand ===\n")
# Source rdlocrand internal functions
source_files_lr <- c(
  "reference/rdlocrand/R/rdlocrand/R/rdlocrand_fun.R",
  "reference/rdlocrand/R/rdlocrand/R/rdrandinf.R"
)
for (f in source_files_lr) {
  source(f)
}

# Use Senate data for randomization inference
data_senate <- read.csv("reference/rdpower/R/rdpower_senate.csv")
ok <- complete.cases(data_senate$demvoteshfor2, data_senate$demmv)
Y_senate <- data_senate$demvoteshfor2[ok]
R_senate <- data_senate$demmv[ok]

# rdrandinf with window [-1, 1]
set.seed(42)
rdr <- rdrandinf(Y = Y_senate, R = R_senate, cutoff = 0, wl = -1, wr = 1,
                 statistic = "diffmeans", reps = 500, seed = 42)

cat("obs_stat:", sprintf("%.15e", rdr$obs.stat), "\n")
cat("p_value:", sprintf("%.15e", rdr$p.value), "\n")
cat("asy_pvalue:", sprintf("%.15e", rdr$asy.pvalue), "\n")
cat("n_window:", rdr$window[3], "\n")
cat("n_treat:", rdr$window[4], "\n")
cat("n_ctrl:", rdr$window[5], "\n")
