#!/usr/bin/env Rscript
# Golden-output generator for the Rust HDL-L port.
#
# Builds a local LD reference (lam, V, LDsc) from the demo PLINK that ships with
# the HDL repo, harmonises the example GWAS sumstats to it, and runs the HDL-L
# numerical core — a faithful, self-contained transcription of `HDL/R/HDL.L.R`
# lines 73-870 (the eigen-space likelihoods + L-BFGS-B multi-start + LRT +
# profile-likelihood CI), using R's own `optim(method="L-BFGS-B")`. This bypasses
# the dplyr/RSpectra/argparser wrapper dependencies and the banded-LD Fortran
# pipeline, isolating exactly the numerical algorithm the Rust crate ports.
#
# Outputs (to tests/fixtures/golden/):
#   lam.tsv, V.tsv, ldsc.tsv      — R's exact per-region eigen reference
#   bhat1.tsv, bhat2.tsv          — aligned Z/√N per SNP
#   meta.tsv                      — N1, N2, N0, M, Nref, eigen.cut, n.retained
#   snps.tsv, a2.tsv              — reference SNP ids + A2 alleles
#   result.tsv                    — h11/h22/h12/rg/rg.lower/rg.upper/p_*/int_*
#
# Run from the repo root, in the r45 conda env:
#   conda activate r45
#   Rscript bio_crates/hdl/tests/fixtures-gen/gen_golden.R
#
# Test data: ships from reference/HDL/ (the cloned zhenin/HDL repo). The demo
# PLINK (build_ld_ref/demo/test.{bed,bim,fam}, 504 individuals) and the example
# GWAS (HDL/data/gwas{1,2}.example.rda) are committed there.

suppressPackageStartupMessages({ library(data.table) })
set.seed(1)

repo <- normalizePath(".", winslash = "/")
hdl_ref <- file.path(repo, "reference/HDL")
demo_prefix <- file.path(hdl_ref, "build_ld_ref/demo/test")
plink <- file.path(hdl_ref, "build_ld_ref/utils/plink_1.9_linux")
out_dir <- file.path(repo, "bio_crates/hdl/tests/fixtures/golden")
dir.create(out_dir, showWarnings = FALSE, recursive = TRUE)
tmp <- file.path(out_dir, ".plink_work")
dir.create(tmp, showWarnings = FALSE, recursive = TRUE)

# ---- 1. pick a local region: chr1 SNPs shared by the demo PLINK & sumstats ----
bim <- fread(paste0(demo_prefix, ".bim"), header = FALSE,
             col.names = c("chr", "snp", "gpos", "pos", "ref", "alt"))
g1 <- { e <- new.env(); load(file.path(hdl_ref, "HDL/data/gwas1.example.rda"), envir = e)
        get(ls(e)[1], envir = e) }
g2 <- { e <- new.env(); load(file.path(hdl_ref, "HDL/data/gwas2.example.rda"), envir = e)
        get(ls(e)[1], envir = e) }
ov <- Reduce(intersect, list(bim$snp[bim$chr == 1], g1$SNP, g2$SNP))
# keep the demo .bim order on chr1
region_snps <- bim$snp[bim$chr == 1 & bim$snp %in% ov]
cat(sprintf("region: %d chr1 SNPs\n", length(region_snps)))
writeLines(region_snps, file.path(tmp, "region.snps"))

# ---- 2. build the LD correlation matrix from the demo genotypes (plink raw) ----
raw_prefix <- file.path(tmp, "region")
system2(plink, c("--silent", "--bfile", demo_prefix, "--extract",
                 file.path(tmp, "region.snps"), "--recode", "A",
                 "--out", raw_prefix), stdout = FALSE, stderr = FALSE)
raw <- fread(paste0(raw_prefix, ".raw"))
# columns: FID IID PAT MAT SEX PHENOTYPE then <alt>_<snp> dosage columns
geno <- as.matrix(raw[, 7:ncol(raw), with = FALSE])
# map dosage columns back to SNP ids (plink --recode A format: "<snp>_<A1>")
col_snps <- sub("_[A-Z0-9]+$", "", colnames(geno))
# order geno to match region_snps
idx <- match(region_snps, col_snps)
geno <- geno[, idx, drop = FALSE]
# Pearson LD correlation matrix (complete-obs per pair → here mean-impute NA to 0
# after centering, matching plink's column-wise standardisation for no-missing
# common SNPs). Standardise columns then R = crossprod/(n-1).
gstd <- scale(geno)
gstd[is.na(gstd)] <- 0
R_ld <- crossprod(gstd) / (nrow(geno) - 1)
LDsc <- rowSums(R_ld^2)

# ---- 3. eigen-decompose (descending, like R eigen()) ----
eig <- eigen(R_ld, symmetric = TRUE)
lam <- eig$values            # descending
V <- eig$vectors             # SNP × n_eigen, columns = eigenvectors
cat(sprintf("eigen: %d components, top var share %.4f\n",
            length(lam), lam[1] / sum(lam)))

# ---- 4. SIMULATE per-SNP bhat from the HDL eigen-space model ----
# The demo region (233 SNPs, Nref=504) carries no real heritability signal in
# the example sumstats, which would leave the gcov/CI code paths unexercised.
# We therefore simulate bhat from the HDL model with KNOWN parameters under a
# fixed seed, so both R and Rust estimate from identical, non-degenerate data.
# (This validates the estimator fidelity, not the sumstats harmonisation —
# which is covered by the PLINK-path test.)
a2.ref <- bim$alt[match(region_snps, bim$snp)]   # demo .bim col 6 = "alt" (A2)
names(a2.ref) <- region_snps
rm(g1, g2)

N1 <- 150000; N2 <- 180000; N0 <- 0
M <- length(region_snps)
Nref <- 335272                    # the production UKB reference size
eigen.cut <- 0.99
lim <- exp(-18)
alpha <- 0.05

set.seed(42)
h11.true <- 0.08
h22.true <- 0.06
h12.true <- 0.03                  # → rg ≈ 0.03/√(0.08·0.06) ≈ 0.43
# HDL operates in bhat-space (bhat = Z/√N); Var[bstar_k] = h²·lam²/M + lam/N
# (matching llfun's lamh2 = h2/M·lam² − h2·lam/Nref + int·lam/N at int=1).
# bstar[k] ~ N(0, Σ_k) per eigen-component, then bhat = V·bstar (V orthogonal).
var1 <- h11.true * lam^2 / M + lam / N1
var2 <- h22.true * lam^2 / M + lam / N2
cov12 <- h12.true * lam^2 / M
bstar1.sim <- bstar2.sim <- numeric(length(lam))
for (k in seq_along(lam)) {
  S <- matrix(c(var1[k], cov12[k], cov12[k], var2[k]), 2, 2)
  # draw via Cholesky
  L <- chol(S)
  z <- matrix(rnorm(2), 1, 2)
  d <- as.numeric(z %*% L)       # N(0, S)
  bstar1.sim[k] <- d[1]
  bstar2.sim[k] <- d[2]
}
bhat1 <- as.numeric(V %*% bstar1.sim)
bhat2 <- as.numeric(V %*% bstar2.sim)
LDsc <- rowSums(R_ld^2)
cat(sprintf("simulated bhat from h11=%.2f h22=%.2f h12=%.2f\n",
            h11.true, h22.true, h12.true))

# ---- 5. HDL-L numerical core (faithful transcription of HDL.L.R 73-870) ----
bstar1 <- as.numeric(crossprod(V, bhat1))
bstar2 <- as.numeric(crossprod(V, bhat2))

# eigen-cut (eigen_select_num.fun)
eigen.percent <- cumsum(lam) / sum(lam)
eigen.num.cut <- if (!(tail(eigen.percent, 1) > eigen.cut)) length(lam) else which(eigen.percent > eigen.cut)[1]
lam.cut   <- lam[1:eigen.num.cut]
bstar1.c  <- bstar1[1:eigen.num.cut]
bstar2.c  <- bstar2[1:eigen.num.cut]
M.ref <- M

llfun <- function(param, N, M, Nref, lam, bstar, lim) {
  h2 <- param[1]; int <- param[2]
  lamh2 <- h2 / M * lam^2 - h2 * lam / Nref + int * lam / N
  lamh2 <- ifelse(lamh2 < lim, lim, lamh2)
  -1 / 2 * (sum(log(lamh2)) + sum(bstar^2 / lamh2))
}
llfun0 <- function(int, N, M, Nref, lam, bstar, lim) {
  lamh2 <- int * lam / N
  lamh2 <- ifelse(lamh2 < lim, lim, lamh2)
  -1 / 2 * (sum(log(lamh2)) + sum(bstar^2 / lamh2))
}
llfun.gcov.part.2 <- function(param, h11, h22, M, N1, N2, N0, Nref,
                              lam1, lam2, bstar1, bstar2, lim) {
  h12 <- param[1]; int <- param[2]
  p1 <- N0 / N1; p2 <- N0 / N2
  lam11 <- h11[1] / M * lam1^2 - h11[1] * lam1 / Nref + h11[2] * lam1 / N1
  lam11 <- ifelse(lam11 < lim, lim, lam11)
  lam22 <- h22[1] / M * lam2^2 - h22[1] * lam2 / Nref + h22[2] * lam2 / N2
  lam22 <- ifelse(lam22 < lim, lim, lam22)
  lam12 <- if (N0 > 0) h12 / M * lam1 * lam2 + p1 * p2 * int * lam1 / N0
           else h12 / M * lam1 * lam2
  ustar <- bstar2 - lam12 / lam11 * bstar1
  lam22.1 <- lam22 - lam12^2 / lam11
  lam22.1 <- ifelse(lam22.1 < lim, lim, lam22.1)
  -1 / 2 * (sum(log(lam22.1)) + sum(ustar^2 / lam22.1))
}
llfun0.gcov.part.2 <- function(int, h11, h22, M, N1, N2, N0, Nref,
                               lam1, lam2, bstar1, bstar2, lim) {
  h12 <- 0; param <- c(h12, int)
  llfun.gcov.part.2(param, h11, h22, M, N1, N2, N0, Nref, lam1, lam2, bstar1, bstar2, lim)
}

# h² MLE for one trait (multistart, exact from HDL.L.R 548-595)
h2_mle <- function(bstar, N, sv1_wls) {
  s1 <- c(sv1_wls, 0, 0.5); s2 <- c(1, 0.5, 1.5, 0); nds <- c(1e-5, 1e-8, 1e-16)
  best <- -Inf; out <- list()
  for (a in s1) for (b in s2) for (nd in nds) {
    opt <- optim(c(a, b), llfun, N = N, Nref = Nref, lam = lam.cut, bstar = bstar,
                 M = M.ref, lim = lim, method = "L-BFGS-B",
                 lower = c(0, 0), upper = c(1, 20),
                 control = list(factr = 1e-8, maxit = 1000, ndeps = c(nd, nd * 100), fnscale = -1))
    opt0 <- optim(b, llfun0, N = N, Nref = Nref, lam = lam.cut, bstar = bstar,
                  M = M.ref, lim = lim, method = "L-BFGS-B", lower = 0, upper = 20,
                  control = list(factr = 1e-8, maxit = 1000, ndeps = nd * 100, fnscale = -1))
    if (opt$convergence == 0 && opt0$convergence == 0 &&
        opt$value > best && opt$value > opt0$value) {
      best <- opt$value
      out <- list(par = opt$par, ll_alt = opt$value, ll_null = opt0$value)
    }
  }
  out
}
r1 <- h2_mle(bstar1.c, N1, 0.1)   # starting h² guess (any reasonable; multistart covers)
r2 <- h2_mle(bstar2.c, N2, 0.1)
h11 <- r1$par[1]; int.h11 <- r1$par[2]
h22 <- r2$par[1]; int.h22 <- r2$par[2]

p.h1 <- pchisq(-2 * (r1$ll_null - r1$ll_alt), 1, lower.tail = FALSE)
p.h2 <- pchisq(-2 * (r2$ll_null - r2$ll_alt), 1, lower.tail = FALSE)

# gcov MLE (multistart, exact from HDL.L.R 718-790)
gcov_mle <- function() {
  bound <- sqrt(h11 * h22)
  s1 <- c(0, 0, -bound * 0.5, bound * 0.5); s2 <- c(0, 1, 0); nds <- c(1e-5, 1e-8, 1e-16)
  best <- -Inf; out <- list()
  h11v <- c(h11, int.h11); h22v <- c(h22, int.h22)
  for (a in s1) for (b in s2) for (nd in nds) {
    opt <- optim(c(a, b), llfun.gcov.part.2, h11 = h11v, h22 = h22v, M = M.ref,
                 N1 = N1, N2 = N2, N0 = N0, Nref = Nref, lam1 = lam.cut, lam2 = lam.cut,
                 bstar1 = bstar1.c, bstar2 = bstar2.c, lim = lim, method = "L-BFGS-B",
                 lower = c(-bound, -20), upper = c(bound, 20),
                 control = list(factr = 1e-8, maxit = 1000, ndeps = c(nd, nd * 100), fnscale = -1))
    opt0 <- optim(b, llfun0.gcov.part.2, h11 = h11v, h22 = h22v, M = M.ref,
                  N1 = N1, N2 = N2, N0 = N0, Nref = Nref, lam1 = lam.cut, lam2 = lam.cut,
                  bstar1 = bstar1.c, bstar2 = bstar2.c, lim = lim, method = "L-BFGS-B",
                  lower = -20, upper = 20,
                  control = list(factr = 1e-8, maxit = 1000, ndeps = nd * 100, fnscale = -1))
    if (opt$convergence == 0 && opt0$convergence == 0 &&
        opt$value > best && opt$value > opt0$value) {
      best <- opt$value
      out <- list(par = opt$par, ll_alt = opt$value, ll_null = opt0$value)
    }
  }
  out
}
rg <- NA; rg.lo <- NA; rg.hi <- NA; h12 <- NA; int.h12 <- NA; p.h12 <- NA
if (h11 > 0 && h22 > 0) {
  r12 <- gcov_mle()
  h12 <- r12$par[1]; int.h12 <- r12$par[2]
  p.h12 <- pchisq(-2 * (r12$ll_null - r12$ll_alt), 1, lower.tail = FALSE)
  denom <- sqrt(h11 * h22)
  h12_val <- seq(-denom, denom, length = 10000)
  ll_v <- sapply(h12_val, function(p)
    llfun.gcov.part.2(c(p, int.h12), h11 = c(h11, int.h11), h22 = c(h22, int.h22),
                      M = M.ref, N1 = N1, N2 = N2, N0 = N0, Nref = Nref,
                      lam1 = lam.cut, lam2 = lam.cut, bstar1 = bstar1.c, bstar2 = bstar2.c, lim = lim))
  h12.LCI <- h12_val[which.max(ll_v)]
  likelihoods <- exp(ll_v - max(ll_v))
  cc <- exp(-qchisq(1 - alpha, 1) / 2)
  ci <- h12_val[which(likelihoods > cc)]
  rg <- h12.LCI / denom
  rg.lo <- max(-1, min(ci) / denom)
  rg.hi <- min(1, max(ci) / denom)
}

cat(sprintf("h11=%.4f h22=%.4f rg=%.4f [%.4f,%.4f] p_h12=%.3e  K=%d\n",
            h11, h22, rg, rg.lo, rg.hi, p.h12, eigen.num.cut))

# ---- 6. export ----
write.table(data.frame(lam = lam), file.path(out_dir, "lam.tsv"),
            sep = "\t", row.names = FALSE, col.names = FALSE, quote = FALSE)
write.table(V, file.path(out_dir, "V.tsv"), sep = "\t",
            row.names = FALSE, col.names = FALSE, quote = FALSE)
write.table(data.frame(ldsc = LDsc), file.path(out_dir, "ldsc.tsv"),
            sep = "\t", row.names = FALSE, col.names = FALSE, quote = FALSE)
write.table(data.frame(bhat1 = bhat1), file.path(out_dir, "bhat1.tsv"),
            sep = "\t", row.names = FALSE, col.names = FALSE, quote = FALSE)
write.table(data.frame(bhat2 = bhat2), file.path(out_dir, "bhat2.tsv"),
            sep = "\t", row.names = FALSE, col.names = FALSE, quote = FALSE)
write.table(data.frame(snps = region_snps), file.path(out_dir, "snps.tsv"),
            sep = "\t", row.names = FALSE, col.names = FALSE, quote = FALSE)
write.table(data.frame(a2 = a2.ref), file.path(out_dir, "a2.tsv"),
            sep = "\t", row.names = FALSE, col.names = FALSE, quote = FALSE)
write.table(data.frame(N1 = N1, N2 = N2, N0 = N0, M = M, Nref = Nref,
                       eigen.cut = eigen.cut, n.retained = eigen.num.cut, lim = lim,
                       alpha = alpha, h11 = h11, h22 = h22, h12 = h12, rg = rg,
                       rg.lower = rg.lo, rg.upper = rg.hi,
                       p.h1 = p.h1, p.h2 = p.h2, p.h12 = p.h12,
                       int.h11 = int.h11, int.h22 = int.h22, int.h12 = int.h12),
            file.path(out_dir, "result.tsv"), sep = "\t",
            row.names = FALSE, quote = FALSE)
unlink(tmp, recursive = TRUE)
cat("golden written to", out_dir, "\n")
