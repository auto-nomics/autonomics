#!/usr/bin/env Rscript
# Closed-form golden values for the non-identity-R GLS competitive test in
# setanalysis.rs (test_gls_non_identity_r_matches_closed_form).
#
# Regenerate with: Rscript gen_gls_nonidentity_golden.R
#
# Mirrors the Rust fixture exactly: 15 genes, AR(1) correlation R with
# rho = 0.3, z-truncation inactive, internal covariates
# size / log(size) / density / log(density) / inv-MAC / log(inv-MAC),
# set indicator on 0-based indices 2, 5, 7, 9, 12.
#
# The attribute values were searched (30k random draws) to minimise
# kappa(X'R^-1X); ~5.8e5 is the attainable floor because the internal
# log-transformed covariates are intrinsically near-collinear with their
# linear counterparts. At that conditioning three independent solve paths
# (LU on the Gram matrix + residual SSR, Cholesky on the Gram matrix +
# z'R^-1z - b'X'R^-1z SSR, QR of the R-whitened design) agree on
# beta / se / pval / beta_std to within ~1e-13 relative, which bounds the
# achievable Rust-vs-R agreement. The script asserts that cross-path
# agreement before printing the golden values.

n <- 15
rho <- 0.3
z <- c(0.5, -1.2, 2.0, -0.3, 1.1, 0.0, -2.5, 1.8, 0.9, -0.7, 1.3, -1.6, 0.4, 2.2, -0.9)
n_snps <- c(35, 32, 17, 4, 15, 20, 19, 64, 26, 2, 18, 59, 22, 40, 23)
n_param <- c(16, 12, 3, 2, 3, 14, 8, 46, 5, 2, 9, 15, 6, 37, 11)
mac <- c(94, 49, 118, 52, 22, 3, 29, 45, 103, 8, 66, 82, 79, 95, 20)

# z truncation bounds as computed in truncate_zstats (population SD):
# both must be inactive for the closed form to hold.
mean_z <- mean(z)
sd_z <- sqrt(sum((z - mean_z)^2) / n)
stopifnot(min(z) >= -3, max(z) < mean_z + 6 * sd_z)

R <- outer(0:(n - 1), 0:(n - 1), function(i, j) rho^abs(i - j))
Rinv <- solve(R)

size <- n_snps
density <- n_param / n_snps
inv_mac <- 1 / mac
# Set members are 0-BASED gene indices, exactly as in the Rust fixture's
# `vec![2, 5, 7, 9, 12]` (analyze_gene_sets matches them against 0..n).
# R is 1-based, so add 1 — an earlier revision indexed directly and silently
# scored a different gene set (the AR(1) inverse is tridiagonal, so two sets
# with the same gap structure can even share the same indicator quadratic
# form and mask the off-by-one in G's diagonal).
set0 <- c(2, 5, 7, 9, 12)
setind <- rep(0, n)
setind[set0 + 1] <- 1

X <- cbind(1, size, log(size), density, log(density), inv_mac, log(inv_mac), setind)
p <- ncol(X)
df <- n - p
stopifnot(df >= 5)

G <- t(X) %*% Rinv %*% X
rhs <- t(X) %*% Rinv %*% z

# Path A: LU solve on the Gram matrix, residual-based SSR (the golden).
bA <- solve(G, rhs)
eA <- z - X %*% bA
ssrA <- as.numeric(t(eA) %*% Rinv %*% eA)
# Path B: Cholesky solve, z'Rinv z - b'X'Rinv z SSR form.
C <- chol(G)
bB <- backsolve(C, forwardsolve(t(C), rhs))
ssrB <- as.numeric(t(z) %*% Rinv %*% z - t(bB) %*% rhs)
# Path C: QR of the R-whitened design (no normal equations at all).
L <- t(chol(R))
Xw <- forwardsolve(L, X)
zw <- forwardsolve(L, z)
bC <- qr.solve(Xw, zw)
eC <- zw - Xw %*% bC
ssrC <- as.numeric(t(eC) %*% eC)

summarise <- function(b, ssr) {
  rv <- ssr / df
  V <- rv * solve(G)
  se <- sqrt(V[p, p])
  tt <- b[p] / se
  pv <- pt(tt, df, lower.tail = FALSE)
  sdp <- sqrt(sum((setind - mean(setind))^2) / n)
  c(beta = b[p], se = se, pval = pv, beta_std = b[p] * sdp)
}

gA <- summarise(bA, ssrA)
gB <- summarise(bB, ssrB)
gC <- summarise(bC, ssrC)
for (nm in c("beta", "se", "pval", "beta_std")) {
  rel <- max(abs(gA[nm] - gB[nm]), abs(gA[nm] - gC[nm])) / abs(gA[nm])
  stopifnot(rel < 1e-12)
  cat(sprintf("%-9s cross-solver rel spread = %.3g\n", nm, rel))
}
cat(sprintf("kappa(X'Rinv X) = %.4g\n", kappa(G, exact = TRUE)))

cat(sprintf("beta     = %.17g\n", gA["beta"]))
cat(sprintf("se       = %.17g\n", gA["se"]))
cat(sprintf("pval     = %.17g\n", gA["pval"]))
cat(sprintf("beta_std = %.17g\n", gA["beta_std"]))
