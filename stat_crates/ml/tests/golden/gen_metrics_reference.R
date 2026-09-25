#!/usr/bin/env Rscript
# Multiclass metrics golden reference — base R only (no pROC needed; AUC via
# the hand-written Mann-Whitney rank formula below).
#
# Generates a 3-class probability table (metrics_data.csv) and golden values
# (metrics_reference.json) for tests/metrics_r_reference.rs.
#
# Bootstrap CIs are NOT golden-tested (Rust ChaCha8 vs R Mersenne Twister
# cannot be aligned); determinism and group integrity are covered by Rust
# unit tests.
#
# Usage (from repo root):
#   Rscript stat_crates/ml/tests/golden/gen_metrics_reference.R

library(jsonlite)

set.seed(20260925)
n <- 300L
k <- 3L

y <- sample(0:2, n, replace = TRUE, prob = c(0.45, 0.35, 0.20))
P <- matrix(rnorm(n * k, sd = 0.6), nrow = n, ncol = k)
P[cbind(seq_len(n), y + 1L)] <- P[cbind(seq_len(n), y + 1L)] + 1.0
P <- exp(P) / rowSums(exp(P))

# Round-trip through the CSV so R references and the Rust test read
# bit-identical inputs (both parse the same decimal strings).
csv <- "stat_crates/ml/tests/golden/metrics_data.csv"
write.csv(data.frame(y = y, p_0 = P[, 1], p_1 = P[, 2], p_2 = P[, 3]),
          csv, row.names = FALSE)
df <- read.csv(csv, colClasses = "numeric")
y <- as.integer(df$y)
P <- as.matrix(df[, c("p_0", "p_1", "p_2")])

# ── References (base R) ────────────────────────────────────────────────────
yhat <- max.col(P, ties.method = "first") - 1L  # first max = Rust argmax

auc_mw <- function(score, is_pos) {
  # Mann-Whitney U with tie handling, identical to ml::metrics::roc_auc
  s <- c(score[is_pos], score[!is_pos])
  r <- rank(s)
  np <- sum(is_pos); nn <- sum(!is_pos)
  (sum(r[seq_len(np)]) - np * (np + 1) / 2) / (np * nn)
}
aucs <- vapply(seq_len(k), function(c) auc_mw(P[, c], y == c - 1L), numeric(1))

acc <- mean(yhat == y)
cm <- table(factor(y, levels = seq_len(k) - 1L),
            factor(yhat, levels = seq_len(k) - 1L))
recall <- diag(cm) / rowSums(cm)
precision <- diag(cm) / colSums(cm)
f1 <- ifelse(precision + recall > 0,
             2 * precision * recall / (precision + recall), 0)
onehot <- model.matrix(~ 0 + factor(y, levels = seq_len(k) - 1L))
# standard multiclass Brier: per-sample sum over classes, then mean over
# samples (NOT the element-wise mean over the n x k matrix, which is this/k)
brier <- mean(rowSums((P - onehot)^2))
log_loss <- -mean(log(P[cbind(seq_len(n), y + 1L)]))

# calibration bins for class 0: 10 equal-width bins, p = 1 falls into bin 9
bins <- pmin(floor(P[, 1] * 10), 9)
g <- data.frame(bin = bins, p = P[, 1], hit = as.integer(y == 0L))
cal0 <- do.call(rbind, lapply(split(g, g$bin), function(d) {
  data.frame(bin = d$bin[1], n = nrow(d),
             mean_pred = mean(d$p), obs_rate = mean(d$hit))
}))
cal0 <- cal0[order(cal0$bin), ]

results <- list(
  n = n, k = k,
  auc = as.numeric(aucs),
  macro_auc = mean(aucs),
  accuracy = acc,
  balanced_accuracy = mean(recall),
  precision = as.numeric(precision),
  recall = as.numeric(recall),
  f1 = as.numeric(f1),
  brier = brier,
  log_loss = log_loss,
  cal0_bins = as.integer(cal0$bin),
  cal0_n = as.integer(cal0$n),
  cal0_mean_pred = as.numeric(cal0$mean_pred),
  cal0_obs_rate = as.numeric(cal0$obs_rate)
)
write_json(results, "stat_crates/ml/tests/golden/metrics_reference.json",
           auto_unbox = TRUE, digits = 15, pretty = TRUE)
cat("metrics golden written: n =", n, " accuracy =", round(acc, 4), "\n")
cat("  auc:", round(aucs, 4), " brier =", round(brier, 4),
    " log_loss =", round(log_loss, 4), "\n")
