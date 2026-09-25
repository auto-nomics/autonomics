#!/usr/bin/env Rscript
# Multinomial elastic-net golden reference against glmnet(family="multinomial").
#
# Generates mnet_data.csv (train/test split + stratified fold column) and
# mnet_reference.json for tests/mnet_r_reference.rs.
#
# glmnet conventions pinned by the 2026-09-25 probes (glmnet 5.1):
#   - internal standardisation = mean + n-divisor SD, coefficients reported
#     back on the original scale;
#   - lambda_max = max |gradient of unpenalised NLL at W=0| / alpha on the
#     standardised scale (verified to 1e-15 against glmnet's own lambda[1]);
#   - cvm = pooled OOF mean of -2 log p_true, cvsd from fold-level means;
#   - df counts features nonzero in any class.
#
# The multinomial objective has flat valleys, so two glmnet runs on
# equivalent parameterisations drift ~2e-2 in coefficients at equal
# objective (probs agree to ~3e-4).  The golden therefore compares
# probabilities / support sets / objective values / the lambda grid —
# NOT coefficients at 1e-3.
#
# The lambda grid and the CV fold ids are shared bit-for-bit with the Rust
# side (same CSV/JSON), so no RNG alignment is needed.
#
# Usage (from repo root):
#   Rscript stat_crates/ml/tests/golden/gen_mnet_reference.R

library(glmnet)
library(jsonlite)

set.seed(20260925)
k <- 3L; p <- 12L; n_train <- 100L; n_test <- 50L
nlam <- 25L; nfolds <- 5L; min_ratio <- 0.01
alpha <- 1.0

gen <- function(n) {
  y <- rep(0:(k - 1L), length.out = n)
  shift <- c(-1, 0, 1)
  x <- matrix(rnorm(n * p), n, p)
  for (c in 0:(k - 1L)) {
    idx <- which(y == c)
    x[idx, 1:2] <- x[idx, 1:2] + shift[c + 1L] * 0.9
    x[idx, 3:4] <- x[idx, 3:4] + shift[c + 1L] * 0.5
  }
  list(x = x, y = y)
}
tr <- gen(n_train); te <- gen(n_test)

# stratified fold assignment for the train part (balanced, 0-based in CSV)
foldid <- integer(n_train)
for (c in 0:(k - 1L)) {
  idx <- sample(which(tr$y == c))
  foldid[idx] <- rep(seq_len(nfolds), length.out = length(idx))
}

# Round-trip through the CSV so both sides parse bit-identical inputs.
csv <- "stat_crates/ml/tests/golden/mnet_data.csv"
write.csv(data.frame(split = rep(c("train", "test"), c(n_train, n_test)),
                     y = c(tr$y, te$y),
                     fold = c(foldid - 1L, rep(NA_integer_, n_test)),
                     rbind(tr$x, te$x)), csv, row.names = FALSE)
df <- read.csv(csv)
is_tr <- df$split == "train"
y  <- as.integer(df$y)
yt <- y[is_tr]
xt <- as.matrix(df[is_tr, -(1:3), drop = FALSE])
xv <- as.matrix(df[!is_tr, -(1:3), drop = FALSE])
foldid <- as.integer(df$fold[is_tr]) + 1L   # back to 1-based
stopifnot(length(yt) == n_train, !any(is.na(foldid)))

# ── lambda_max (probe-verified formula) and the shared grid ────────────────
n <- n_train
Y <- model.matrix(~ 0 + factor(yt, levels = 0:(k - 1L)))
mean_tr <- colMeans(xt)
sdv <- sqrt(colSums((sweep(xt, 2, mean_tr)^2)) / n)
z <- sweep(sweep(xt, 2, mean_tr), 2, sdv, "/")
prior <- colMeans(Y)
grad0 <- (1 / n) * crossprod(z, Y - matrix(prior, n, k, byrow = TRUE))
lambda_max <- max(abs(grad0)) / alpha

grid <- exp(seq(log(lambda_max), log(lambda_max * min_ratio), length.out = nlam))

# self-check: the formula equals glmnet's own lambda[1]
fit_def <- glmnet(xt, factor(yt, levels = 0:(k - 1L)), family = "multinomial",
                  alpha = alpha, standardize = TRUE)
stopifnot(abs(fit_def$lambda[1] - lambda_max) < 1e-12)

# ── tight-threshold fit on the shared grid ─────────────────────────────────
yf <- factor(yt, levels = 0:(k - 1L))
fit <- glmnet(xt, yf, family = "multinomial", alpha = alpha,
              standardize = TRUE, lambda = grid,
              control = list(thresh = 1e-12))
cf_mat <- function(f, s) sapply(coef(f, s = s), function(m) as.matrix(m)[, 1])

# df along the path (features nonzero in any class)
df_path <- sapply(grid, function(l) {
  cf <- cf_mat(fit, l)
  sum(apply(abs(cf[-1, , drop = FALSE]) > 0, 1, any))
})

# fixed mid-path lambda for probability/objective/support comparison
i_fix <- 9L
lam_fix <- grid[i_fix]
cf_fix <- cf_mat(fit, lam_fix)          # (p+1) x k, original scale

# self-check: glmnet's predict == softmax of the reported coefficients
eta_fix <- cbind(1, xv) %*% cf_fix
pm <- exp(eta_fix) / rowSums(exp(eta_fix))
pg <- drop(predict(fit, newx = xv, s = lam_fix, type = "response"))
if (length(dim(pg)) == 3) pg <- pg[, , 1]
stopifnot(max(abs(pm - pg)) < 1e-12)

yhat_fix <- max.col(pm, ties.method = "first") - 1L
support_fix <- which(apply(abs(cf_fix[-1, , drop = FALSE]) > 0, 1, any)) - 1L

# objective (z-scale) at glmnet's solution, for the Rust side to compare
w_z <- sweep(cf_fix[-1, , drop = FALSE], 1, sdv, "*")
b_z <- cf_fix[1, ] + as.vector(mean_tr %*% w_z)
eta_z <- cbind(1, z) %*% rbind(b_z, w_z)
mx <- apply(eta_z, 1, max)
loglik <- eta_z[cbind(1:n, yt + 1L)] - mx - log(rowSums(exp(eta_z - mx)))
obj_fix <- -mean(loglik) + lam_fix * alpha * sum(abs(w_z)) +
  0.5 * lam_fix * (1 - alpha) * sum(w_z^2)

# ── CV on the shared grid and shared folds ─────────────────────────────────
cvfit <- cv.glmnet(xt, yf, family = "multinomial", alpha = alpha,
                   standardize = TRUE, lambda = grid, foldid = foldid,
                   control = list(thresh = 1e-12))
# self-check: cvm == pooled per-sample mean deviance (probe5 formula)
D <- matrix(NA_real_, n, nlam)
for (f in seq_len(nfolds)) {
  tr_f <- foldid != f
  ff <- glmnet(xt[tr_f, ], yf[tr_f], family = "multinomial", alpha = alpha,
               standardize = TRUE, lambda = grid,
               control = list(thresh = 1e-12))
  for (li in seq_len(nlam)) {
    pp <- drop(predict(ff, newx = xt[!tr_f, ], s = grid[li], type = "response"))
    if (length(dim(pp)) == 3) pp <- pp[, , 1]
    D[!tr_f, li] <- -2 * log(pp[cbind(seq_len(sum(!tr_f)), yt[!tr_f] + 1L)])
  }
}
# 1e-5 not 1e-8: at the smallest lambdas glmnet's internal fold fits and
# explicit refits drift ~2e-7 along the objective's flat valleys (probe:
# the formula itself matches to 4e-16 on minimal data).
stopifnot(max(abs(cvfit$cvm - colMeans(D))) < 1e-5)
# NB: cvfit$nzero is NOT cross-checked — cv.glmnet's internal full fit runs
# a different code path than an explicit glmnet() and reports fewer active
# features at small lambda.  The golden's df reference is the explicit fit.

results <- list(
  n_train = n_train, n_test = n_test, k = k, p = p,
  alpha = alpha, nfolds = nfolds,
  lambda_max = lambda_max,
  lambda_grid = as.numeric(grid),
  i_fix = i_fix - 1L,                    # 0-based
  lambda_fix = lam_fix,
  coef_fix = as.numeric(t(cf_fix)),      # class-major: k rows of (p+1)
  support_fix = as.integer(support_fix),
  obj_fix = obj_fix,
  probs_fix = as.numeric(t(pm)),         # row-major n_test x k (t()! R
                                         # flattens matrices column-major)
  yhat_fix = as.integer(yhat_fix),
  df_path = as.integer(df_path),
  cvm = as.numeric(cvfit$cvm),
  cvsd = as.numeric(cvfit$cvsd),
  lambda_min = cvfit$lambda.min,
  lambda_1se = cvfit$lambda.1se
)
write_json(results, "stat_crates/ml/tests/golden/mnet_reference.json",
           auto_unbox = TRUE, digits = 15, pretty = TRUE)
cat("glmnet multinomial golden written: lambda_max =", round(lambda_max, 4),
    " support at fix:", length(support_fix), "/", p,
    " test acc:", round(mean(yhat_fix == y[!is_tr]), 3),
    " lambda.min index:", which.min(cvfit$cvm), "\n")
