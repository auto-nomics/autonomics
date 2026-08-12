#!/usr/bin/env Rscript
# Generate golden reference for rdmulti::rdmc using the Senate data
# with artificially created multi-cutoff structure.

library(rdrobust)
library(rdmulti)

# Load Senate data
data_senate <- read.csv("reference/rdpower/R/rdpower_senate.csv")
ok <- complete.cases(data_senate$demvoteshfor2, data_senate$demmv)
Y <- data_senate$demvoteshfor2[ok]
R <- data_senate$demmv[ok]

# Create multi-cutoff structure: split at 0 (original) and add a second
# cutoff at 5 for observations with R >= 5
C <- ifelse(R >= 5, 5, 0)

cat("n_obs:", length(Y), "\n")
cat("cutoffs:", paste(sort(unique(C)), collapse=", "), "\n")

# Run rdmc
result <- rdmc(Y, R, C, level=95)

cat("\n--- rdmc results ---\n")
cat("pooled_tau_cl:", sprintf("%.15e", as.numeric(result$tau)), "\n")
cat("pooled_se_rb:", sprintf("%.15e", as.numeric(result$se.rb)), "\n")
cat("weighted_tau_bc:", sprintf("%.15e", as.numeric(result$B["weighted"])), "\n")
cat("weighted_se_rb:", sprintf("%.15e", sqrt(as.numeric(result$V["weighted"]))), "\n")

# Per-cutoff
for (k in seq_along(sort(unique(C)))) {
  cat("cutoff", k, "_tau_cl:", sprintf("%.15e", as.numeric(result$Coefs[k])), "\n")
  cat("cutoff", k, "_tau_bc:", sprintf("%.15e", as.numeric(result$B[k])), "\n")
  cat("cutoff", k, "_se_rb:", sprintf("%.15e", sqrt(as.numeric(result$V[k]))), "\n")
  cat("cutoff", k, "_weight:", sprintf("%.15e", as.numeric(result$W[k])), "\n")
}
