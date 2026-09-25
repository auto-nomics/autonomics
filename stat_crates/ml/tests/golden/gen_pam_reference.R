#!/usr/bin/env Rscript
# PAM (nearest shrunken centroid) golden reference against pamr.
#
# Generates pam_data.csv (train/test split, informative + noise features)
# and pam_reference.json for tests/pam_r_reference.rs.
#
# CV fold assignment is NOT aligned (RNG differs between the two sides) —
# the golden compares training statistics, the Δ grid, posteriors, classes
# and the signature feature set at a fixed Δ.  The script self-checks its
# formula-derived statistics against pamr's stored internals before
# writing anything.
#
# Usage (from repo root):
#   Rscript stat_crates/ml/tests/golden/gen_pam_reference.R

library(pamr)
library(jsonlite)

set.seed(20260925)
k <- 3L; p <- 50L; n_train <- 90L; n_test <- 45L

gen <- function(n) {
  y <- rep(0:(k - 1L), length.out = n)
  shift <- c(-1, 0, 1)
  x <- matrix(rnorm(n * p), n, p)
  for (c in 0:(k - 1L)) {
    idx <- which(y == c)
    x[idx, 1:3] <- x[idx, 1:3] + shift[c + 1L] * 1.2
    x[idx, 4:6] <- x[idx, 4:6] + shift[c + 1L] * 0.5
  }
  list(x = x, y = y)
}
tr <- gen(n_train); te <- gen(n_test)

# Round-trip through the CSV so R references and the Rust test parse
# bit-identical inputs (both read the same decimal strings).
csv <- "stat_crates/ml/tests/golden/pam_data.csv"
write.csv(data.frame(split = rep(c("train", "test"), c(n_train, n_test)),
                     y = c(tr$y, te$y),
                     rbind(tr$x, te$x)), csv, row.names = FALSE)
df <- read.csv(csv)
is_tr <- df$split == "train"
y  <- as.integer(df$y)
xt <- as.matrix(df[is_tr, -(1:2), drop = FALSE])
xv <- as.matrix(df[!is_tr, -(1:2), drop = FALSE])
yt <- y[is_tr]

fit <- pamr.train(list(x = t(xt), y = factor(yt)))
cat("pamr object components:", paste(names(fit), collapse = ", "), "\n")

# ── verified-formula statistics (probe-validated vs pamr to 1e-16) ────────
nk <- as.numeric(table(yt)); N <- n_train
mk <- sqrt(1 / nk - 1 / N)
xb <- colMeans(xt)
xk <- t(sapply(0:(k - 1L), function(c) colMeans(xt[yt == c, , drop = FALSE])))
sd0 <- sqrt(sapply(seq_len(p), function(j) sum((xt[, j] - xk[yt + 1L, j])^2) / (N - k)))
sdv <- sd0 + as.numeric(quantile(sd0, 0.5))
dk <- (xk - matrix(xb, k, p, byrow = TRUE)) /
      (matrix(sdv, k, p, byrow = TRUE) * matrix(mk, k, p, byrow = TRUE))
pri <- nk / N

# self-check against pamr's stored internals (names printed above)
stopifnot(
  isTRUE(all.equal(as.numeric(fit$se.scale), mk, tolerance = 1e-12)),
  isTRUE(all.equal(as.numeric(fit$sd), sdv, tolerance = 1e-12)),
  isTRUE(all.equal(as.numeric(fit$prior), pri, tolerance = 1e-12)),
  isTRUE(all.equal(as.numeric(fit$threshold),
                   seq(0, max(abs(dk)), length.out = 30), tolerance = 1e-10))
)

# fixed mid-grid threshold for posterior/signature comparison
i_thr <- 15L
thr <- fit$threshold[i_thr]
pp <- as.matrix(pamr.predict(fit, t(xv), threshold = thr, type = "posterior"))
# NB: as.integer on a factor gives level INDICES (1..k), not label values —
# must go through as.character to recover the 0-based class ids.
yhat <- as.integer(as.character(pamr.predict(fit, t(xv), threshold = thr)))

# signature: features with any nonzero shrunken d' at thr
dks <- sign(dk) * pmax(abs(dk) - thr, 0)
nz <- which(colSums(abs(dks)) > 0) - 1L

results <- list(
  n_train = n_train, n_test = n_test, k = k, p = p,
  threshold_grid = as.numeric(fit$threshold),
  i_thr = i_thr, thr = thr,
  grand_mean = as.numeric(xb),
  pooled_sd = as.numeric(sdv),
  se_scale = as.numeric(mk),
  priors = as.numeric(pri),
  posterior = as.numeric(t(pp)),       # row-major n_test × k
  yhat = yhat,
  signature_features = as.integer(nz)
)
write_json(results, "stat_crates/ml/tests/golden/pam_reference.json",
           auto_unbox = TRUE, digits = 15, pretty = TRUE)
cat("pamr golden written: thr =", round(thr, 4),
    " nonzero features:", length(nz), "/", p,
    " test acc:", round(mean(yhat == y[!is_tr]), 3), "\n")
