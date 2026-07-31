#!/usr/bin/env Rscript
# Generate golden reference values from CPASSOC R reference (FunctionSet.R)
# Output: golden_values.json
#
# Usage: Rscript generate_golden.R

set.seed(42)
source("reference/FunctionSet.R")

library(jsonlite)

results <- list()

# ─── Test 1: SHom on fixed 3×2 matrix ───────────────────────────────────────
# 3 SNPs × 2 traits
X1 <- matrix(c(1.5, 2.0, -1.0,
                0.5, 3.0,  1.5), nrow=3, ncol=2, byrow=FALSE)
SampleSize1 <- c(1000, 800)
CorrMatrix1 <- matrix(c(1.0, 0.3,
                        0.3, 1.0), nrow=2, ncol=2, byrow=TRUE)

shom1 <- SHom(X=X1, SampleSize=SampleSize1, CorrMatrix=CorrMatrix1)
results$shom_test1 <- as.numeric(shom1)

# ─── Test 2: SHom on fixed 5×3 matrix ───────────────────────────────────────
X2 <- matrix(c(2.0, 1.5, 0.5, -1.0, 3.0,
               1.0, 2.5, 1.0,  0.5, 2.0,
               0.5, 1.0, 3.0, -2.0, 1.5), nrow=5, ncol=3, byrow=FALSE)
SampleSize2 <- c(1000, 1500, 800)
CorrMatrix2 <- matrix(c(1.00, 0.30, 0.10,
                        0.30, 1.00, 0.50,
                        0.10, 0.50, 1.00), nrow=3, ncol=3, byrow=TRUE)

shom2 <- SHom(X=X2, SampleSize=SampleSize2, CorrMatrix=CorrMatrix2)
results$shom_test2 <- as.numeric(shom2)

# ─── Test 3: SHet on same matrices ──────────────────────────────────────────
shet1 <- SHet(X=X1, SampleSize=SampleSize1, CorrMatrix=CorrMatrix1,
              correct=1, isAllpossible=T)
results$shet_test1 <- as.numeric(shet1)

shet2 <- SHet(X=X2, SampleSize=SampleSize2, CorrMatrix=CorrMatrix2,
              correct=1, isAllpossible=T)
results$shet_test2 <- as.numeric(shet2)

# ─── Test 4: SHet with correct=0 (no sign correction) ──────────────────────
shet1_nc <- SHet(X=X1, SampleSize=SampleSize1, CorrMatrix=CorrMatrix1,
                 correct=0, isAllpossible=T)
results$shet_test1_nocorrect <- as.numeric(shet1_nc)

# ─── Test 5: SHet with fixed cutoff sequence ────────────────────────────────
shet2_seq <- SHet(X=X2, SampleSize=SampleSize2, CorrMatrix=CorrMatrix2,
                  correct=1, startCutoff=0, endCutoff=1, CutoffStep=0.1,
                  isAllpossible=F)
results$shet_test2_seq <- as.numeric(shet2_seq)

# ─── Test 6: Correlation matrix (cor) ───────────────────────────────────────
# Generate a fixed dataset
set.seed(123)
data_corr <- matrix(rnorm(500*3), nrow=500, ncol=3)
# Add some correlation
data_corr[,2] <- data_corr[,1] * 0.6 + data_corr[,2] * 0.8
data_corr[,3] <- data_corr[,1] * (-0.3) + data_corr[,3] * 0.95
corr_est <- cor(data_corr)
results$corr_matrix <- as.numeric(t(corr_est))  # row-major

# ─── Test 7: EstimateGamma on 3×3 correlation matrix ────────────────────────
set.seed(999)
# Use the correlation matrix from test 2
gamma_params <- EstimateGamma(N=10000, SampleSize=SampleSize2,
                              CorrMatrix=CorrMatrix2, correct=1, isAllpossible=T)
results$gamma_test <- as.numeric(gamma_params)
names(results$gamma_test) <- c("shape", "scale", "shift")

# ─── Test 8: EstimateGamma on 2×2 ───────────────────────────────────────────
set.seed(888)
gamma_params2 <- EstimateGamma(N=10000, SampleSize=SampleSize1,
                               CorrMatrix=CorrMatrix1, correct=1, isAllpossible=T)
results$gamma_test2 <- as.numeric(gamma_params2)

# ─── Test 9: p-values ───────────────────────────────────────────────────────
# SHom p-value: pchisq(stat, df=1, lower.tail=F)
results$shom_pvalue <- as.numeric(pchisq(shom2, df=1, lower.tail=F))

# SHet p-value: pgamma(stat - a, shape=k, scale=theta, lower.tail=F)
results$shet_pvalue <- as.numeric(pgamma(shet2 - gamma_params[3],
                                         shape=gamma_params[1],
                                         scale=gamma_params[2],
                                         lower.tail=F))

# ─── Test 10: weight vector ─────────────────────────────────────────────────
Wi <- matrix(SampleSize2, nrow=1)
sumW <- sqrt(sum(Wi^2))
W <- Wi / sumW
results$weight_vector <- as.numeric(W)

# ─── Test 11: SHom/SHet with single row (edge case) ─────────────────────────
X_single <- matrix(c(2.5, -1.5, 0.8), nrow=1, ncol=3)
shom_single <- SHom(X=X_single, SampleSize=SampleSize2, CorrMatrix=CorrMatrix2)
shet_single <- SHet(X=X_single, SampleSize=SampleSize2, CorrMatrix=CorrMatrix2,
                    correct=1, isAllpossible=T)
results$shom_single <- as.numeric(shom_single)
results$shet_single <- as.numeric(shet_single)

# ─── Test 12: SHet with opposite effects (correct=1 vs correct=0) ───────────
X_opp <- matrix(c(2.0, -2.0, 1.5, -1.5, 0.5, -0.5), nrow=2, ncol=3, byrow=TRUE)
shet_opp_c1 <- SHet(X=X_opp, SampleSize=SampleSize2, CorrMatrix=CorrMatrix2,
                    correct=1, isAllpossible=T)
shet_opp_c0 <- SHet(X=X_opp, SampleSize=SampleSize2, CorrMatrix=CorrMatrix2,
                    correct=0, isAllpossible=T)
shom_opp <- SHom(X=X_opp, SampleSize=SampleSize2, CorrMatrix=CorrMatrix2)
results$shet_opp_correct1 <- as.numeric(shet_opp_c1)
results$shet_opp_correct0 <- as.numeric(shet_opp_c0)
results$shom_opp <- as.numeric(shom_opp)

# ─── Save ───────────────────────────────────────────────────────────────────
cat(toJSON(results, auto_unbox=TRUE, digits=17, pretty=TRUE),
    file="bio_crates/cpassoc/tests/golden_values.json")

cat("\nGolden values written to bio_crates/cpassoc/tests/golden_values.json\n")
cat("SHom test1:", shom1, "\n")
cat("SHet test1:", shet1, "\n")
cat("Gamma params (test 7):", gamma_params, "\n")
