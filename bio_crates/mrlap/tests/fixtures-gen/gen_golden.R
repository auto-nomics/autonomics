#!/usr/bin/env Rscript
# Golden-output generator for the Rust MRlap port.
#
# MRlap's LDSC and IVW-MR stages reuse the already-validated `ldsc` and `mr`
# crates, so this generator isolates the *novel* MRlap maths for cross-
# validation: input standardisation (tidy_inputGWAS), allele harmonisation,
# distance-based pruning, and the de-biasing correction (get_correction).
#
# It is a faithful, self-contained base-R transcription of:
#   - reference/MRlap/R/tidy_inputGWAS.R   (Z / std_beta / std_SE / p)
#   - reference/MRlap/R/run_MR.R           (allele alignment, prune_byDistance)
#   - reference/MRlap/R/get_correction.R   (get_pi / get_alpha / bootstrap)
#
# GenomicSEM / TwoSampleMR / dplyr are NOT used (they are not installed in the
# r45 env); only stats + base R, so every number is reproducible. The LDSC
# outputs (lambda, h2_LDSC, ...) are FIXED inputs shared by R and Rust — the
# LDSC regression itself is cross-validated separately in the `ldsc` crate.
#
# Run from the repo root, in the r45 conda env:
#   conda activate r45
#   Rscript bio_crates/mrlap/tests/fixtures-gen/gen_golden.R
#
# Outputs (to tests/fixtures/golden/):
#   gwas_exp.tsv, gwas_out.tsv   — raw synthetic GWAS (Z/N/alleles/chr/pos)
#   tidy.tsv                     — per-SNP std_beta/std_SE/p (exposure)
#   harmonised.tsv               — joined + allele-aligned rows
#   pruned.tsv                   — IVs surviving threshold + distance pruning
#   ivw.tsv                      — alpha_obs / alpha_obs_se / n_exp / n_out
#   correction.tsv               — pi_x / sigma2_x / alpha_corrected / se / cov / ...
#
# rclone restore (test data archived at aliyun://autonomics-data):
#   rclone copy aliyun:autonomics-data/mrlap/golden bio_crates/mrlap/tests/fixtures/golden

suppressPackageStartupMessages({ library(data.table) })
set.seed(33)

repo <- normalizePath(".", winslash = "/")
out_dir <- file.path(repo, "bio_crates/mrlap/tests/fixtures/golden")
dir.create(out_dir, showWarnings = FALSE, recursive = TRUE)

# ---------------------------------------------------------------------------
# 1. Synthesize a small GWAS pair with a known set of strong exposure effects.
# ---------------------------------------------------------------------------
n_snp <- 120
chr <- rep(1:6, each = 20)
pos <- as.integer(runif(n_snp, 1e6, 5e7))
rsid <- sprintf("rs%d", seq_len(n_snp))
alt <- sample(c("A", "C", "G", "T"), n_snp, replace = TRUE)
ref <- sample(c("A", "C", "G", "T"), n_snp, replace = TRUE)
N_exp <- 100000
N_out <- 80000

# Exposure: ~12 SNPs with genome-significant effects, rest null.
beta_exp_true <- rnorm(n_snp, 0, 0.01)
sig_idx <- sort(sample(n_snp, 12))
beta_exp_true[sig_idx] <- rnorm(length(sig_idx), 0.05, 0.02)
se_exp <- rep(0.005, n_snp)
z_exp <- beta_exp_true / se_exp + rnorm(n_snp, 0, 0.3)

# Outcome: causal effect 0.3 on the exposure scale + noise.
beta_out_true <- 0.3 * beta_exp_true + rnorm(n_snp, 0, 0.01)
se_out <- rep(0.006, n_snp)
z_out <- beta_out_true / se_out + rnorm(n_snp, 0, 0.3)

gwas_exp <- data.table(rsid = rsid, chr = chr, pos = pos, alt = alt, ref = ref,
                       Z = z_exp, N = N_exp)
gwas_out <- data.table(rsid = rsid, chr = chr, pos = pos, alt = alt, ref = ref,
                       Z = z_out, N = N_out)
# Introduce a few allele swaps in the outcome to exercise harmonisation.
swap <- sample(n_snp, 10)
tmp <- gwas_out$alt[swap]; gwas_out$alt[swap] <- gwas_out$ref[swap]; gwas_out$ref[swap] <- tmp
# And flip the sign of those outcome Z-scores so alignment recovers them.
gwas_out$Z[swap] <- -gwas_out$Z[swap]

fwrite(gwas_exp, file.path(out_dir, "gwas_exp.tsv"), sep = "\t")
fwrite(gwas_out, file.path(out_dir, "gwas_out.tsv"), sep = "\t")

# ---------------------------------------------------------------------------
# 2. tidy_inputGWAS  — std_beta = Z/sqrt(N), std_SE = 1/sqrt(N), p
# ---------------------------------------------------------------------------
tidy <- function(g) {
  g[, std_beta := Z / sqrt(N)]
  g[, std_SE   := 1 / sqrt(N)]
  g[, p        := 2 * pnorm(-abs(Z))]
  g
}
te <- tidy(copy(gwas_exp))
to <- tidy(copy(gwas_out))
fwrite(te[, .(rsid, chr, pos, alt, ref, Z, N, std_beta, std_SE, p)],
       file.path(out_dir, "tidy.tsv"), sep = "\t")

# ---------------------------------------------------------------------------
# 3. Harmonise — inner join on rsid, align outcome alleles to exposure.
# ---------------------------------------------------------------------------
m <- merge(te, to, by = "rsid", suffixes = c(".exp", ".out"))
m[, std_beta.out := fifelse(
       alt.exp == alt.out & ref.exp == ref.out, std_beta.out,
       fifelse(ref.exp == alt.out & alt.exp == ref.out, -std_beta.out, NA_real_))]
m <- m[!is.na(std_beta.out)]
fwrite(m[, .(rsid, chr.exp, pos.exp, alt.exp, ref.exp,
             std_beta.exp, std_SE.exp, p.exp, N.exp,
             std_beta.out, std_SE.out, p.out, N.out)],
       file.path(out_dir, "harmonised.tsv"), sep = "\t")

# ---------------------------------------------------------------------------
# 4. Distance-based pruning (run_MR.R prune_byDistance, default r2 = 0).
#    threshold p_exp < 5e-8, prune distance 500 kb, no reverse filtering.
# ---------------------------------------------------------------------------
mr_threshold <- 5e-8
pruning_dist_kb <- 500
th <- m[p.exp < mr_threshold]
prune_by_distance <- function(data, prune.dist) {
  data <- data[order(p.exp)]
  i <- 1
  while (i <= nrow(data)) {
    keep <- rep(TRUE, nrow(data))
    for (j in seq_len(nrow(data))) {
      if (j == i) next
      if (data$chr.exp[j] == data$chr.exp[i] &&
          abs(data$pos.exp[j] - data$pos.exp[i]) < prune.dist * 1000) {
        keep[j] <- FALSE
      }
    }
    # remove the flagged ones (except the current anchor)
    rm_idx <- which(!keep)
    rm_idx <- setdiff(rm_idx, i)
    if (length(rm_idx) > 0) data <- data[-rm_idx]
    i <- i + 1
  }
  data$rsid
}
kept <- prune_by_distance(copy(th), pruning_dist_kb)
pruned <- th[rsid %in% kept]
fwrite(pruned[, .(rsid, std_beta.exp, std_SE.exp, std_beta.out, std_SE.out, N.exp, N.out)],
       file.path(out_dir, "pruned.tsv"), sep = "\t")

# ---------------------------------------------------------------------------
# 5. IVW-MR (multiplicative random effects, TwoSampleMR::mr_ivw transcription).
# ---------------------------------------------------------------------------
mr_ivw <- function(b_exp, b_out, se_exp, se_out) {
  w <- 1 / se_out^2
  b <- sum(b_exp * b_out * w) / sum(b_exp^2 * w)
  Q <- sum(w * (b_out - b * b_exp)^2)
  n <- length(b_exp)
  sigma <- sqrt(max(0, Q / (n - 1)))
  sigma <- max(1, sigma)            # multiplicative, under-dispersion guard
  se <- sqrt(1 / sum(b_exp^2 * w)) * sigma
  c(b = b, se = se)
}
ivw <- mr_ivw(pruned$std_beta.exp, pruned$std_beta.out,
              pruned$std_SE.exp, pruned$std_SE.out)
n_exp <- mean(pruned$N.exp)
n_out <- mean(pruned$N.out)
fwrite(data.table(alpha_obs = ivw["b"], alpha_obs_se = unname(ivw["se"]),
                  n_exp = n_exp, n_out = n_out, n_iv = nrow(pruned)),
       file.path(out_dir, "ivw.tsv"), sep = "\t")

# ---------------------------------------------------------------------------
# 6. get_correction — faithful base-R transcription of get_correction.R.
#    LDSC outputs are FIXED inputs (the LDSC regression is validated elsewhere).
# ---------------------------------------------------------------------------
M  <- 1150000
Tr <- -qnorm(mr_threshold / 2)

# Fixed LDSC outputs (would come from cross-trait LDSC on real data).
lambda     <- 0.005     # cross-trait intercept (mild sample overlap)
lambda_se  <- 0.001
h2_LDSC    <- 0.15      # exposure SNP-h2
h2_LDSC_se <- 0.02

get_pi <- function(my_pi, sumbeta2, Tr, n_exp, h2_LDSC, M) {
  if (0 >= my_pi) return(1e6)
  sigma2 <- h2_LDSC / (my_pi * M)
  sigma  <- sqrt(sigma2)
  A <- pnorm(-Tr / sqrt(1 + n_exp * sigma2))
  B <- 2 * Tr * exp(-Tr^2 / (2 * (n_exp * sigma2 + 1))) /
       (sqrt(2 * pi) * (1 + n_exp * sigma2)^(3 / 2))
  Cp <- pnorm(-Tr)
  Cd <- dnorm(Tr)
  denom <- (my_pi * (2 * (sigma2 + 1/n_exp) * A +
                     B * (n_exp * sigma2^2 + 2*sigma2 + 1/n_exp)) +
             (1 - my_pi) * 1/n_exp * (2*Cp + 2*Tr*Cd)) * M
  abs(denom - sumbeta2)
}

get_genA <- function(effects, h2_LDSC, n_exp, M, Tr) {
  sumb <- sum(effects^2)
  res <- optimise(get_pi, interval = c(1e-7, 0.3),
                  sumbeta2 = sumb, Tr = Tr, n_exp = n_exp,
                  h2_LDSC = h2_LDSC, M = M, tol = 1e-6)
  pi_x <- res$minimum
  sigma <- sqrt(h2_LDSC / (M * pi_x))
  c(pi_x, sigma)
}

get_alpha <- function(n_exp, lambdaPrime, pi_x, sigma, alpha_obs, Tr) {
  sigma2 <- sigma^2
  A <- pnorm(-Tr / sqrt(1 + n_exp * sigma2))
  B <- 2 * Tr * exp(-Tr^2 / (2 * (n_exp * sigma2 + 1))) /
       (sqrt(2 * pi) * (1 + n_exp * sigma2)^(3 / 2))
  C <- pnorm(-Tr) + Tr * dnorm(Tr)
  a <- pi_x * (2 * (sigma2 + 1/n_exp) * A +
               B * (n_exp * sigma2^2 + 2*sigma2 + 1/n_exp))
  b <- (1 - pi_x) * 2/n_exp * C
  d <- a + b
  (alpha_obs * d -
     (lambdaPrime * pi_x * (2*A + B + sigma2 * n_exp * B) +
      lambdaPrime * (1 - pi_x) * 2 * C)) /
   (pi_x * sigma2 * (2*A + B * n_exp * sigma2 + B))
}

lambdaPrime <- lambda / sqrt(n_exp * n_out)
ga <- get_genA(pruned$std_beta.exp, h2_LDSC, n_exp, M, Tr)
pi_x  <- ga[1]
sigma <- ga[2]
alpha_corrected <- get_alpha(n_exp, lambdaPrime, pi_x, sigma, ivw["b"], Tr)

# Bootstrap SE + cov (get_correctedSE), set.seed for reproducibility.
get_correctedSE <- function(IVs_beta, IVs_se, lambda, lambda_se, h2_LDSC, h2_LDSC_se,
                            alpha_obs, alpha_obs_se, n_exp, n_out, M, Tr,
                            s = 1000, sthreshold = 0.05, num_groups = 10) {
  get_s <- function(s) {
    L <- rnorm(s, lambda, lambda_se) / sqrt(n_exp * n_out)
    E <- matrix(rnorm(length(IVs_beta) * s, IVs_beta, IVs_se), ncol = s)
    negH2 <- FALSE
    H <- rnorm(s, h2_LDSC, h2_LDSC_se)
    if (any(H < 0)) {
      negH2 <- TRUE
      while (!all(H > 0)) H[H < 0] <- rnorm(sum(H < 0), h2_LDSC, h2_LDSC_se)
    }
    D <- rbind(E, H)
    pis <- apply(D, 2, function(x) get_genA(x[-length(x)], x[length(x)], n_exp, M, Tr))
    B <- rnorm(s, alpha_obs, alpha_obs_se)
    pi_v   <- pis[1, ]
    sig_v  <- pis[2, ]
    data.frame(pi = pi_v, sigma = sig_v, alpha = B, lambda = L,
               corrected = sapply(seq_len(s), function(j)
                 get_alpha(n_exp, L[j], pi_v[j], sig_v[j], B[j], Tr)),
               warning_negH2 = negH2)
  }
  res <- get_s(s)
  tmp_sd <- sd(res$corrected)
  needmore <- TRUE
  while (needmore) {
    v <- sapply(split(res, (seq_len(nrow(res)) - 1) %/% (nrow(res) / num_groups)),
                function(x) var(x$corrected))
    co <- sapply(split(res, (seq_len(nrow(res)) - 1) %/% (nrow(res) / num_groups)),
                 function(x) cov(x$corrected, x$alpha))
    needmore <- (sd(v) / abs(mean(v)) > sthreshold) ||
                (sd(co) / abs(mean(co)) > sthreshold) ||
                (alpha_obs_se^2 + sd(res$corrected)^2 - 2 * cov(res$alpha, res$corrected) < 0)
    if (needmore) {
      res <- rbind(res, get_s(s))
      # filter only for the CV diagnostic
      f <- subset(res, corrected < alpha_corrected + 10 * tmp_sd &
                       corrected > alpha_corrected - 10 * tmp_sd)
      v <- sapply(split(f, (seq_len(nrow(f)) - 1) %/% (nrow(f) / num_groups)),
                  function(x) var(x$corrected))
      co <- sapply(split(f, (seq_len(nrow(f)) - 1) %/% (nrow(f) / num_groups)),
                   function(x) cov(x$corrected, x$alpha))
      needmore <- (sd(v) / abs(mean(v)) > sthreshold) ||
                  (sd(co) / abs(mean(co)) > sthreshold) ||
                  (alpha_obs_se^2 + sd(res$corrected)^2 - 2 * cov(res$alpha, res$corrected) < 0)
    }
  }
  c(sd_corrected = sd(res$corrected),
    cov = cov(res$alpha, res$corrected),
    n_sim = nrow(res),
    neg_h2 = any(res$warning_negH2))
}

set.seed(42)
se_cov <- get_correctedSE(pruned$std_beta.exp, pruned$std_SE.exp,
                          lambda, lambda_se, h2_LDSC, h2_LDSC_se,
                          ivw["b"], unname(ivw["se"]), n_exp, n_out, M, Tr)
test_diff <- (ivw["b"] - alpha_corrected) /
             sqrt(ivw["se"]^2 + se_cov["sd_corrected"]^2 - 2 * se_cov["cov"])
p_diff <- 2 * pnorm(-abs(test_diff))

fwrite(data.table(
  pi_x = pi_x, sigma2_x = sigma^2, alpha_corrected = alpha_corrected,
  alpha_corrected_se = se_cov["sd_corrected"], cov_obs_corrected = se_cov["cov"],
  test_diff = test_diff, p_diff = p_diff, n_sim = se_cov["n_sim"],
  lambda = lambda, lambda_se = lambda_se, h2_LDSC = h2_LDSC, h2_LDSC_se = h2_LDSC_se,
  mr_threshold = mr_threshold, n_exp = n_exp, n_out = n_out
), file.path(out_dir, "correction.tsv"), sep = "\t")

cat("golden fixtures written to", out_dir, "\n")
