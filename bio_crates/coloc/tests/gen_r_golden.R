#!/usr/bin/env Rscript
# Generate golden coloc output from the reference R package (coloc v5.x)
# for cross-validation against the Rust port.
#
# Usage (r45 conda env: R 4.5.3 + coloc):
#   export R_HOME=$HOME/miniconda3/envs/r45/lib/R
#   export PATH=$HOME/miniconda3/envs/r45/bin:$PATH
#   export LD_LIBRARY_PATH=$HOME/miniconda3/envs/r45/lib:$LD_LIBRARY_PATH
#   Rscript tests/gen_r_golden.R <out.json>
#
# Writes a JSON object keyed by scenario name. Each scenario captures
# inputs + all outputs needed for cross-validation: summary PPs, per-SNP
# lABF/SNP.PP.H4, and intermediates.

suppressMessages(library(coloc))
suppressMessages(library(jsonlite))

set.seed(42)

# ── Generate synthetic data (same as test-abf.R) ─────────────────────────────

n_snp <- 100
n_sample <- 400
X <- do.call("cbind", lapply(runif(n_snp, 0.05, 0.5), function(maf) rbinom(n_sample, 2, maf)))
colnames(X) <- paste0("S", 1:n_snp)
maf <- colMeans(X) / 2

# Quantitative trait (signal at SNP 1)
Y <- rnorm(n_sample, mean = as(X[, 1], "numeric"), sd = 4) + rnorm(n_sample, sd = 2)
m <- sapply(1:ncol(X), function(i) {
    mod <- lm(Y ~ X[, i])
    c(coefficients(mod)[-1], vcov(mod)[-1, -1])
})
beta.q <- m[1, ]
vbeta.q <- m[2, ]
p.q <- pchisq(beta.q^2 / vbeta.q, df = 1, lower.tail = FALSE)
sd.est <- suppressWarnings(coloc:::sdY.est(vbeta = vbeta.q, maf = maf, n = n_sample))

# Case-control trait (signal at SNP 1)
cc <- rbinom(n_sample, 1, p = (1 + as(X[, 1], "numeric")) / 4)
m <- sapply(1:ncol(X), function(i) {
    mod <- glm(cc ~ X[, i], family = "binomial")
    c(coefficients(mod)[-1], vcov(mod)[-1, -1])
})
beta.cc <- m[1, ]
vbeta.cc <- m[2, ]
p.cc <- pchisq(beta.cc^2 / vbeta.cc, df = 1, lower.tail = FALSE)
s.cc <- mean(cc)

# ── Datasets ────────────────────────────────────────────────────────────────

DQ <- list(beta = beta.q, varbeta = vbeta.q, type = "quant",
           snp = colnames(X), sdY = sd.est, N = n_sample)
DCC <- list(beta = beta.cc, varbeta = vbeta.cc, type = "cc",
            snp = colnames(X), MAF = maf, s = s.cc, N = n_sample)
PQ <- list(pvalues = p.q, type = "quant", MAF = maf,
           snp = colnames(X), sdY = sd.est, N = n_sample)
PCC <- list(pvalues = p.cc, type = "cc", MAF = maf,
            snp = colnames(X), s = s.cc, N = n_sample)

scenarios <- list()

# ── Scenario 1: quant-beta × cc-beta (coloc.abf) ────────────────────────────
r1 <- suppressWarnings(coloc.abf(DQ, DCC))
scenarios$dd <- list(
    type = "coloc_abf",
    nsnps = as.integer(r1$summary["nsnps"]),
    pp = as.numeric(r1$summary[paste0("PP.H", 0:4, ".abf")]),
    pp_names = paste0("PP.H", 0:4, ".abf"),
    priors = c(p1 = r1$priors["p1"], p2 = r1$priors["p2"], p12 = r1$priors["p12"]),
    results = lapply(1:nrow(r1$results), function(i) {
        list(
            snp = as.character(r1$results$snp[i]),
            lABF.df1 = r1$results$lABF.df1[i],
            lABF.df2 = r1$results$lABF.df2[i],
            internal.sum.lABF = r1$results$internal.sum.lABF[i],
            SNP.PP.H4 = r1$results$SNP.PP.H4[i]
        )
    })
)

# ── Scenario 2: quant-beta × cc-pvalue (coloc.abf) ──────────────────────────
r2 <- suppressWarnings(coloc.abf(DQ, PCC))
scenarios$dp <- list(
    type = "coloc_abf",
    nsnps = as.integer(r2$summary["nsnps"]),
    pp = as.numeric(r2$summary[paste0("PP.H", 0:4, ".abf")]),
    priors = c(p1 = r2$priors["p1"], p2 = r2$priors["p2"], p12 = r2$priors["p12"]),
    results = lapply(1:nrow(r2$results), function(i) {
        list(
            snp = as.character(r2$results$snp[i]),
            lABF.df1 = r2$results$lABF.df1[i],
            lABF.df2 = r2$results$lABF.df2[i],
            internal.sum.lABF = r2$results$internal.sum.lABF[i],
            SNP.PP.H4 = r2$results$SNP.PP.H4[i]
        )
    })
)

# ── Scenario 3: quant-pvalue × cc-beta (coloc.abf) ──────────────────────────
r3 <- suppressWarnings(coloc.abf(PQ, DCC))
scenarios$pd <- list(
    type = "coloc_abf",
    nsnps = as.integer(r3$summary["nsnps"]),
    pp = as.numeric(r3$summary[paste0("PP.H", 0:4, ".abf")]),
    priors = c(p1 = r3$priors["p1"], p2 = r3$priors["p2"], p12 = r3$priors["p12"]),
    results = lapply(1:nrow(r3$results), function(i) {
        list(
            snp = as.character(r3$results$snp[i]),
            lABF.df1 = r3$results$lABF.df1[i],
            lABF.df2 = r3$results$lABF.df2[i],
            internal.sum.lABF = r3$results$internal.sum.lABF[i],
            SNP.PP.H4 = r3$results$SNP.PP.H4[i]
        )
    })
)

# ── Scenario 4: quant-pvalue × cc-pvalue (coloc.abf) ────────────────────────
r4 <- suppressWarnings(coloc.abf(PQ, PCC))
scenarios$pp <- list(
    type = "coloc_abf",
    nsnps = as.integer(r4$summary["nsnps"]),
    pp = as.numeric(r4$summary[paste0("PP.H", 0:4, ".abf")]),
    priors = c(p1 = r4$priors["p1"], p2 = r4$priors["p2"], p12 = r4$priors["p12"]),
    results = lapply(1:nrow(r4$results), function(i) {
        list(
            snp = as.character(r4$results$snp[i]),
            lABF.df1 = r4$results$lABF.df1[i],
            lABF.df2 = r4$results$lABF.df2[i],
            internal.sum.lABF = r4$results$internal.sum.lABF[i],
            SNP.PP.H4 = r4$results$SNP.PP.H4[i]
        )
    })
)

# ── Scenario 5: finemap.abf on quantitative trait ───────────────────────────
r5 <- finemap.abf(DQ)
scenarios$finemap_dq <- list(
    type = "finemap_abf",
    snp = as.character(r5$snp),
    lABF = r5$lABF.,
    prior = r5$prior,
    SNP.PP = r5$SNP.PP
)

# ── Scenario 6: finemap.abf on case-control trait ───────────────────────────
r6 <- finemap.abf(DCC)
scenarios$finemap_dcc <- list(
    type = "finemap_abf",
    snp = as.character(r6$snp),
    lABF = r6$lABF.,
    prior = r6$prior,
    SNP.PP = r6$SNP.PP
)

# ── Scenario 7: coloc.abf with prior weights ────────────────────────────────
pw1 <- runif(n_snp)
pw2 <- runif(n_snp)
r7 <- suppressWarnings(coloc.abf(DQ, DCC, prior_weights1 = pw1, prior_weights2 = pw2))
scenarios$weighted <- list(
    type = "coloc_abf_weighted",
    nsnps = as.integer(r7$summary["nsnps"]),
    pp = as.numeric(r7$summary[paste0("PP.H", 0:4, ".abf")]),
    priors = c(p1 = r7$priors["p1"], p2 = r7$priors["p2"], p12 = r7$priors["p12"]),
    prior_weights1 = pw1,
    prior_weights2 = pw2,
    results = lapply(1:nrow(r7$results), function(i) {
        list(
            snp = as.character(r7$results$snp[i]),
            lABF.df1 = r7$results$lABF.df1[i],
            lABF.df2 = r7$results$lABF.df2[i],
            SNP.PP.H4 = r7$results$SNP.PP.H4[i]
        )
    })
)

# ── Scenario 8: custom priors ───────────────────────────────────────────────
r8 <- suppressWarnings(coloc.abf(DQ, DCC, p1 = 1e-3, p2 = 5e-4, p12 = 1e-6))
scenarios$custom_priors <- list(
    type = "coloc_abf",
    nsnps = as.integer(r8$summary["nsnps"]),
    pp = as.numeric(r8$summary[paste0("PP.H", 0:4, ".abf")]),
    priors = c(p1 = r8$priors["p1"], p2 = r8$priors["p2"], p12 = r8$priors["p12"]),
    results = lapply(1:nrow(r8$results), function(i) {
        list(
            snp = as.character(r8$results$snp[i]),
            lABF.df1 = r8$results$lABF.df1[i],
            lABF.df2 = r8$results$lABF.df2[i],
            SNP.PP.H4 = r8$results$SNP.PP.H4[i]
        )
    })
)

# ── Save input data for reproduction ────────────────────────────────────────
inputs <- list(
    snp = colnames(X),
    beta_q = beta.q, vbeta_q = vbeta.q, p_q = p.q,
    beta_cc = beta.cc, vbeta_cc = vbeta.cc, p_cc = p.cc,
    maf = maf, N = n_sample, sdY = sd.est, s = s.cc
)

out <- list(inputs = inputs, scenarios = scenarios)

out_file <- commandArgs(trailingOnly = TRUE)[1]
writeLines(toJSON(out, auto_unbox = FALSE, digits = 17), out_file)
cat("wrote", out_file, "\n")
