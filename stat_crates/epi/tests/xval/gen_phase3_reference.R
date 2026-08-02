#!/usr/bin/env Rscript
# Cross-validation reference for Phase 3: LCA + Random Forest.
#
# Generates shared synthetic data, runs R reference implementations
# (poLCA, randomForest), saves results as JSON for Rust comparison.
#
# Usage: Rscript gen_phase3_reference.R

library(jsonlite)
set.seed(42)

# ═══════════════════════════════════════════════════════════════════════════
# 1. LCA: binary indicator data with 2 latent classes
# ═══════════════════════════════════════════════════════════════════════════

n_lca <- 400
n_indicators <- 5

# Class 0 (n/2 subjects): P(indicator=1) = 0.85
# Class 1 (n/2 subjects): P(indicator=1) = 0.15
lca_data <- matrix(0, nrow = n_lca, ncol = n_indicators)
true_class <- rep(c(0, 1), each = n_lca / 2)

for (i in 1:n_lca) {
  p <- if (true_class[i] == 0) 0.85 else 0.15
  lca_data[i, ] <- rbinom(n_indicators, 1, p)
}

lca_df <- data.frame(lca_data)
colnames(lca_df) <- paste0("v", 1:n_indicators)

# Run poLCA with K=2.
# poLCA uses formula interface: cbind(v1, v2, ...) ~ 1
library(poLCA)
f <- as.formula(paste("cbind(", paste("v", 1:n_indicators, sep = "", collapse = ", "), ") ~ 1"))
lca_fit <- poLCA(f, lca_df + 1, nclass = 2, verbose = FALSE)  # poLCA needs 1-based values

# Item probabilities: lca_fit$probs[[j]] is a K×2 matrix (class × response).
# P(y_j = 1 | class k) = probs[[j]][k, 2].
# sapply gives K×J, transpose to J×K.
lca_item_prob <- t(sapply(lca_fit$probs, function(m) m[, 2]))  # J × K matrix

lca_results <- list(
  class_prevalence = as.numeric(lca_fit$P),
  item_probabilities = lapply(1:n_indicators, function(j) as.numeric(lca_item_prob[j, ])),
  bic = as.numeric(lca_fit$bic),
  aic = as.numeric(lca_fit$aic),
  ll = as.numeric(lca_fit$llik),
  n = n_lca,
  n_indicators = n_indicators,
  n_classes = 2,
  # Class assignments (poLCA uses 1-based; convert to 0-based)
  class_assignment = as.numeric(lca_fit$pred.class - 1)
)

write.csv(lca_df, "stat_crates/epi/tests/xval/lca_data.csv", row.names = FALSE)

# ═══════════════════════════════════════════════════════════════════════════
# 2. Random Forest: classification data
# ═══════════════════════════════════════════════════════════════════════════

n_rf <- 300
rf_x1 <- rnorm(n_rf, 0, 1)
rf_x2 <- rnorm(n_rf, 0, 1)
rf_x3 <- rnorm(n_rf, 0, 1)  # noise feature

# Decision rule: y = 1 if x1 + x2 > 0 (with some noise)
rf_y <- ifelse(rf_x1 + rf_x2 + rnorm(n_rf, 0, 0.5) > 0, 1, 0)

rf_df <- data.frame(x1 = rf_x1, x2 = rf_x2, x3 = rf_x3, y = rf_y)

# Run randomForest.
library(randomForest)
rf_fit <- randomForest(as.factor(y) ~ x1 + x2 + x3, data = rf_df, ntree = 100, importance = TRUE)

rf_results <- list(
  oob_accuracy = 1 - rf_fit$err.rate[100, 1],  # OOB error rate at ntree=100
  importance = as.numeric(importance(rf_fit)[, 1]),  # MeanDecreaseGini for x1, x2, x3
  importance_names = c("x1", "x2", "x3"),
  predictions = as.numeric(predict(rf_fit, type = "prob")[, 2]),  # P(class=1)
  n = n_rf,
  n_features = 3,
  n_trees = 100
)

write.csv(rf_df, "stat_crates/epi/tests/xval/rf_data.csv", row.names = FALSE)

# ═══════════════════════════════════════════════════════════════════════════
# Save combined results
# ═══════════════════════════════════════════════════════════════════════════

results <- list(
  lca = lca_results,
  rf = rf_results
)

write_json(results, "stat_crates/epi/tests/xval/phase3_reference.json",
           auto_unbox = TRUE, digits = 12, pretty = TRUE)

cat("Phase 3 reference written.\n")
cat("  LCA: prevalence =", round(lca_fit$P, 3), "\n")
cat("       BIC =", round(lca_fit$bic, 1), "\n")
cat("  RF:  OOB accuracy =", round(1 - rf_fit$err.rate[100, 1], 4), "\n")
cat("       importance =", round(importance(rf_fit)[, 1], 2), "\n")
