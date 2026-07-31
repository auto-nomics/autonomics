#!/usr/bin/env Rscript
# Run CPASSOC R reference on real GWAS Z-scores extracted from VCFs.
# Output: real_gwas_golden.json
#
# Usage: Rscript generate_real_golden.R

source("reference/FunctionSet.R")
library(jsonlite)

# Read the extracted Z-scores
data <- read.csv("bio_crates/cpassoc/tests/real_gwas_zscores.csv", stringsAsFactors=FALSE)
cat("Loaded", nrow(data), "SNPs x", ncol(data)-1, "traits\n")

# Extract Z-score matrix (drop rsid column)
X <- as.matrix(data[, -1])
rownames(X) <- data$rsid

# Sample sizes (extracted from VCF headers)
SampleSize <- c(461823, 484121, 483078)

# Correlation matrix from the Z-scores
CorrMatrix <- cor(X)
cat("\nCorrelation matrix:\n")
print(CorrMatrix)

# SHom
shom_stats <- SHom(X=X, SampleSize=SampleSize, CorrMatrix=CorrMatrix)
cat("\nSHom stats (first 10):", head(shom_stats, 10), "\n")

# SHet
shet_stats <- SHet(X=X, SampleSize=SampleSize, CorrMatrix=CorrMatrix,
                   correct=1, isAllpossible=T)
cat("SHet stats (first 10):", head(shet_stats, 10), "\n")

# SHom p-values
shom_p <- pchisq(shom_stats, df=1, lower.tail=F)

# Estimate gamma (use fewer sims for speed)
set.seed(777)
gamma_params <- EstimateGamma(N=5000, SampleSize=SampleSize,
                              CorrMatrix=CorrMatrix, correct=1, isAllpossible=T)
cat("\nGamma params: shape=", gamma_params[1],
    " scale=", gamma_params[2], " shift=", gamma_params[3], "\n")

# SHet p-values
shet_p <- pgamma(shet_stats - gamma_params[3],
                 shape=gamma_params[1], scale=gamma_params[2], lower.tail=F)

# Save results
results <- list(
  snp_ids = data$rsid,
  sample_size = SampleSize,
  corr_matrix = as.numeric(t(CorrMatrix)),  # row-major
  shom_stats = as.numeric(shom_stats),
  shet_stats = as.numeric(shet_stats),
  shom_pvalue = as.numeric(shom_p),
  gamma_params = as.numeric(gamma_params),
  shet_pvalue = as.numeric(shet_p)
)

cat(toJSON(results, auto_unbox=TRUE, digits=17, pretty=TRUE),
    file="bio_crates/cpassoc/tests/real_gwas_golden.json")

cat("\nGolden values written to bio_crates/cpassoc/tests/real_gwas_golden.json\n")
cat("SHom range:", range(shom_stats), "\n")
cat("SHet range:", range(shet_stats), "\n")
cat("SHom p range:", range(shom_p), "\n")
cat("SHet p range:", range(shet_p), "\n")
