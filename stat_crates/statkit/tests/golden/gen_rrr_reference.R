## Golden generator for tests/rrr_r_reference.rs.
##
## Reduced-rank regression has no base-R package; the reference here
## implements the SAME estimator as statkit::regression::reduced_rank_regression
## with base primitives (lm + svd): per-response WLS, weighted-centred fitted
## matrix SVD, rank-k projection of the slopes, intercept re-added from the
## weighted means. The script self-checks the rank-1 lossless case.
##
## Run locally (R ≥ 4.x with jsonlite):  Rscript stat_crates/statkit/tests/golden/gen_rrr_reference.R
suppressPackageStartupMessages(library(jsonlite))

out_dir <- "stat_crates/statkit/tests/golden"
set.seed(20260926)
n <- 120
x1 <- rnorm(n)
x2 <- 0.5 * x1 + rnorm(n) * 0.8
x3 <- rnorm(n)
## Two latent directions drive the responses; y3 adds idiosyncratic noise.
f1 <- 1.2 * x1 - 0.7 * x2 + 0.3 * x3
f2 <- 0.4 * x1 + 0.9 * x2 - 0.5 * x3
y1 <- 2.0 + 1.0 * f1 + 0.2 * f2 + rnorm(n) * 0.1
y2 <- -1.0 + 0.6 * f1 + 1.1 * f2 + rnorm(n) * 0.1
y3 <- 0.5 + 1.5 * f1 - 0.3 * f2 + rnorm(n) * 0.5
wt <- rep(1, n)  # unweighted golden; weighted parity is covered by unit tests
dat <- data.frame(x1, x2, x3, y1, y2, y3, wt)
write.csv(dat, file.path(out_dir, "rrr_data.csv"), row.names = FALSE)

rrr_ref <- function(X, Y, w, k) {
  n <- nrow(X); p <- ncol(X); r <- ncol(Y)
  B <- sapply(seq_len(r), function(j) coef(lm(Y[, j] ~ X, weights = w)))  # (p+1) × r
  H <- cbind(1, X) %*% B
  wmean <- colSums(w * H) / sum(w)
  Hc <- sweep(H, 2, wmean)
  s <- svd(Hc)
  Vk <- s$v[, seq_len(k), drop = FALSE]
  slopes <- B[-1, , drop = FALSE] %*% Vk %*% t(Vk)          # p × r
  xbar <- colSums(w * X) / sum(w)
  ybar <- colSums(w * Y) / sum(w)
  intercept <- as.numeric(ybar - xbar %*% slopes)
  yhat <- cbind(1, X) %*% rbind(intercept, slopes)
  sse <- colSums(w * (Y - yhat)^2)
  sst <- colSums(w * sweep(Y, 2, ybar)^2)
  list(
    intercept = intercept,
    slopes = slopes,
    d = s$d,
    ve = s$d^2 / sum(s$d^2),
    cum = cumsum(s$d^2 / sum(s$d^2)),
    r2 = as.numeric(1 - sse / sst)
  )
}

X <- as.matrix(dat[, c("x1", "x2", "x3")])
Y <- as.matrix(dat[, c("y1", "y2", "y3")])
ref <- list(
  ranks = list(
    k1 = rrr_ref(X, Y, wt, 1),
    k2 = rrr_ref(X, Y, wt, 2),
    k3 = rrr_ref(X, Y, wt, 3)
  )
)
## Flatten matrices column-major (jsonlite drops dims) — record slope layout
## explicitly: slopes[i, j] = predictor i on response j, rows stacked.
flatten <- function(m) as.numeric(t(m))
ref$ranks$k1$slopes <- flatten(ref$ranks$k1$slopes)
ref$ranks$k2$slopes <- flatten(ref$ranks$k2$slopes)
ref$ranks$k3$slopes <- flatten(ref$ranks$k3$slopes)

## Self-check: a noise-free shared-direction system must be lossless at rank 1.
ok <- local({
  y1e <- 2.0 + 1.0 * f1            # exactly in span(X)
  y2e <- 2 * y1e + 1
  a <- rrr_ref(X, cbind(y1e, y2e), rep(1, n), 1)
  all(abs(a$r2 - 1) < 1e-8)
})
stopifnot(ok)

write_json(ref, file.path(out_dir, "rrr_reference.json"), auto_unbox = TRUE, digits = 16)
cat("regenerated", out_dir, "goldens\n")
