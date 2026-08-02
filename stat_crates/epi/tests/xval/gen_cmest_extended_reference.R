#!/usr/bin/env Rscript
# Cross-validation reference for extended cmest variants:
# 1. Multi-mediator (rb)
# 2. Binary outcome (rb, OR scale)
# 3. Binary mediator (rb)
# 4. Weighting-based (wb)
# 5. g-formula
#
# All formulas verified against CMAverse GitHub source (est_rb.R, est.wb.R).
#
# Usage: Rscript gen_cmest_extended_reference.R

library(jsonlite)
set.seed(42)

# ═══════════════════════════════════════════════════════════════════════════
# 1. Multi-mediator data + reference
# ═══════════════════════════════════════════════════════════════════════════

n <- 500
x <- rbinom(n, 1, 0.5)
c1 <- rnorm(n, 0, 1)
m1 <- 0.5 * x + 0.3 * c1 + rnorm(n, 0, 0.5)
m2 <- 0.3 * x - 0.2 * c1 + rnorm(n, 0, 0.5)
y_multi <- 0.3 * x + 0.8 * m1 + 0.5 * m2 + rnorm(n, 0, 0.3)

multi_df <- data.frame(x = x, m1 = m1, m2 = m2, y = y_multi, c1 = c1)
write.csv(multi_df, "stat_crates/epi/tests/xval/cmest_multi_data.csv", row.names = FALSE)

# Multi-mediator rb decomposition (no interaction).
fit_m1 <- lm(m1 ~ x + c1)
fit_m2 <- lm(m2 ~ x + c1)
fit_y_multi <- lm(y_multi ~ x + m1 + m2 + c1)

alpha1_m1 <- coef(fit_m1)["x"]
alpha1_m2 <- coef(fit_m2)["x"]
beta_x <- coef(fit_y_multi)["x"]
beta_m1 <- coef(fit_y_multi)["m1"]
beta_m2 <- coef(fit_y_multi)["m2"]

# NDE = beta_x; NIE_j = beta_mj * alpha1_j; TE = NDE + sum(NIE_j)
nde_multi <- as.numeric(beta_x)
nie_m1 <- as.numeric(beta_m1 * alpha1_m1)
nie_m2 <- as.numeric(beta_m2 * alpha1_m2)
nie_multi <- nie_m1 + nie_m2
te_multi <- nde_multi + nie_multi

# ═══════════════════════════════════════════════════════════════════════════
# 2. Binary outcome reference (OR scale)
# ═══════════════════════════════════════════════════════════════════════════

m_by <- 1.0 + 0.5 * x + rnorm(n, 0, 0.5)
eta_y <- -1.0 + 0.5 * x + 0.3 * m_by
y_bin <- rbinom(n, 1, 1 / (1 + exp(-eta_y)))

biny_df <- data.frame(x = x, m = m_by, y = y_bin)
write.csv(biny_df, "stat_crates/epi/tests/xval/cmest_biny_data.csv", row.names = FALSE)

fit_m_biny <- lm(m_by ~ x)
fit_y_biny <- glm(y ~ x + m, family = binomial)

alpha1_biny <- coef(fit_m_biny)["x"]
beta1_biny <- coef(fit_y_biny)["x"]
beta2_biny <- coef(fit_y_biny)["m"]

# OR-scale effects (no interaction).
nde_or <- as.numeric(exp(beta1_biny))
nie_or <- as.numeric(exp(beta2_biny * alpha1_biny))
te_or <- nde_or * nie_or
pm_or <- log(nie_or) / log(te_or)

# ═══════════════════════════════════════════════════════════════════════════
# 3. Binary mediator reference
# ═══════════════════════════════════════════════════════════════════════════

eta_m_bin <- -0.5 + 0.8 * x
m_bin <- rbinom(n, 1, 1 / (1 + exp(-eta_m_bin)))
y_binm <- 0.3 * x + 0.8 * m_bin + rnorm(n, 0, 0.3)

binm_df <- data.frame(x = x, m = m_bin, y = y_binm)
write.csv(binm_df, "stat_crates/epi/tests/xval/cmest_binm_data.csv", row.names = FALSE)

fit_m_binm <- glm(m ~ x, family = binomial)
fit_y_binm <- lm(y ~ x + m)

# P(M=1|X=0) and P(M=1|X=1) at intercept.
p_m1_x0 <- as.numeric(1 / (1 + exp(-coef(fit_m_binm)["(Intercept)"])))
p_m1_x1 <- as.numeric(1 / (1 + exp(-(coef(fit_m_binm)["(Intercept)"] + coef(fit_m_binm)["x"]))))
delta_p <- p_m1_x1 - p_m1_x0

beta1_binm <- coef(fit_y_binm)["x"]
beta2_binm <- coef(fit_y_binm)["m"]

nde_binm <- as.numeric(beta1_binm)
nie_binm <- as.numeric(beta2_binm * delta_p)
te_binm <- nde_binm + nie_binm

# ═══════════════════════════════════════════════════════════════════════════
# 4+5. Weighting and g-formula (use same single-mediator data)
# ═══════════════════════════════════════════════════════════════════════════

# Reuse mediation_data.csv (from Phase 1 cross-validation).
# Weighting: TE from weighted Y~X; NDE from weighted Y~X+M.
ps_model <- glm(x ~ c1, data = data.frame(x = x, c1 = c1), family = binomial)
ps <- fitted(ps_model)
p_bar <- mean(x)
sw <- ifelse(x == 1, p_bar / ps, (1 - p_bar) / (1 - ps))

# Use simple data for weighting/gformula comparison.
y_simple <- 0.3 * x + 0.8 * m1 + rnorm(n, 0, 0.3)
simple_df <- data.frame(x = x, m = m1, y = y_simple, c1 = c1)
write.csv(simple_df, "stat_crates/epi/tests/xval/cmest_wb_gf_data.csv", row.names = FALSE)

te_wb <- coef(lm(y_simple ~ x, weights = sw))["x"]
nde_wb <- coef(lm(y_simple ~ x + m1, weights = sw))["x"]
nie_wb <- as.numeric(te_wb - nde_wb)

# g-formula.
fit_m_gf <- lm(m1 ~ x + c1)
fit_y_gf <- lm(y_simple ~ x + m1 + c1)
# Counterfactual predictions.
m_x0 <- predict(fit_m_gf, newdata = data.frame(x = 0, c1 = c1))
m_x1 <- predict(fit_m_gf, newdata = data.frame(x = 1, c1 = c1))
y_x1_m1 <- mean(predict(fit_y_gf, newdata = data.frame(x = 1, m1 = m_x1, c1 = c1)))
y_x0_m0 <- mean(predict(fit_y_gf, newdata = data.frame(x = 0, m1 = m_x0, c1 = c1)))
y_x1_m0 <- mean(predict(fit_y_gf, newdata = data.frame(x = 1, m1 = m_x0, c1 = c1)))

te_gf <- as.numeric(y_x1_m1 - y_x0_m0)
nde_gf <- as.numeric(y_x1_m0 - y_x0_m0)
nie_gf <- as.numeric(te_gf - nde_gf)

# ═══════════════════════════════════════════════════════════════════════════
# Save results
# ═══════════════════════════════════════════════════════════════════════════

results <- list(
  multi = list(
    nde = as.numeric(nde_multi),
    nie = as.numeric(nie_multi),
    te = as.numeric(te_multi),
    nie_m1 = as.numeric(nie_m1),
    nie_m2 = as.numeric(nie_m2)
  ),
  biny = list(
    nde = as.numeric(nde_or),
    nie = as.numeric(nie_or),
    te = as.numeric(te_or),
    pm = as.numeric(pm_or)
  ),
  binm = list(
    nde = as.numeric(nde_binm),
    nie = as.numeric(nie_binm),
    te = as.numeric(te_binm)
  ),
  wb = list(
    nde = as.numeric(nde_wb),
    nie = as.numeric(nie_wb),
    te = as.numeric(te_wb)
  ),
  gf = list(
    nde = as.numeric(nde_gf),
    nie = as.numeric(nie_gf),
    te = as.numeric(te_gf)
  ),
  n = n
)

write_json(results, "stat_crates/epi/tests/xval/cmest_extended_reference.json",
           auto_unbox = TRUE, digits = 12, pretty = TRUE)

cat("Extended cmest reference written.\n")
cat("  Multi:  NDE=", round(nde_multi, 4), "NIE=", round(nie_multi, 4), "TE=", round(te_multi, 4), "\n")
cat("  BinY:   NDE(OR)=", round(nde_or, 4), "NIE(OR)=", round(nie_or, 4), "TE(OR)=", round(te_or, 4), "\n")
cat("  BinM:   NDE=", round(nde_binm, 4), "NIE=", round(nie_binm, 4), "TE=", round(te_binm, 4), "\n")
cat("  WB:     NDE=", round(nde_wb, 4), "NIE=", round(nie_wb, 4), "TE=", round(te_wb, 4), "\n")
cat("  GF:     NDE=", round(nde_gf, 4), "NIE=", round(nie_gf, 4), "TE=", round(te_gf, 4), "\n")
