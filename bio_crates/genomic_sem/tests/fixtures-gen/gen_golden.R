#!/usr/bin/env Rscript
# Generate golden fixtures for GenomicSEM cross-validation.
#
# Since GenomicSEM is not installed, we reproduce its exact pipeline:
# 1. Construct known S (genetic covariance) and V (sampling covariance) matrices
# 2. Run lavaan::sem() with the same options GenomicSEM uses:
#    - estimator = "DWLS"
#    - WLS.V = custom weight matrix (inverse of diag(V))
#    - sample.nobs = 2
#    - se = "standard"
#    - optim.force.converged = TRUE
# 3. Extract parameter estimates, model-implied covariance, and compute
#    GenomicSEM's sandwich SE formula
#
# Output: JSON files in tests/fixtures/golden/

library(lavaan)
library(Matrix)
library(jsonlite)

out_dir <- "tests/fixtures/golden"
dir.create(out_dir, showWarnings = FALSE, recursive = TRUE)

# Helper: vech (column-major lower triangle including diagonal)
vech <- function(m) {
  m[lower.tri(m, diag = TRUE)]
}

# Helper: sandwich SE computation (GenomicSEM formula)
# Ohtt = bread · lettuce' · V_Reorder · lettuce · bread
# where bread = (Δ'WΔ)⁻¹, lettuce = WΔ
# Returns NULL if the information matrix is singular (e.g. saturated model).
compute_sandwich <- function(fit, v_reorder) {
  delta <- lavInspect(fit, "delta")
  W <- lavInspect(fit, "WLS.V")
  dtwd <- tryCatch(solve(t(delta) %*% W %*% delta), error = function(e) NULL)
  if (is.null(dtwd)) {
    n_par <- ncol(delta)
    return(list(bread = NULL, Ohtt = NULL, SE = rep(NA_real_, n_par), delta = delta, W = W))
  }
  bread <- dtwd
  lettuce <- W %*% delta
  Ohtt <- bread %*% t(lettuce) %*% v_reorder %*% lettuce %*% bread
  SE <- sqrt(diag(Ohtt))
  list(bread = bread, Ohtt = Ohtt, SE = SE, delta = delta, W = W)
}

# =====================================================================
# Scenario 1: 3-trait one-factor model (saturated, df=0)
# Known loadings: [0.5, 0.4, 0.3], resid var: [0.1, 0.15, 0.12], F var: 1.0
# =====================================================================

cat("Generating scenario 1: 3-trait saturated one-factor...\n")
loadings1 <- c(0.5, 0.4, 0.3)
resid1 <- c(0.10, 0.15, 0.12)
fvar1 <- 1.0

S1 <- outer(loadings1, loadings1) * fvar1 + diag(resid1)
colnames(S1) <- rownames(S1) <- c("V1", "V2", "V3")

# V matrix: diagonal sampling covariance
z1 <- 6  # 3*4/2
V1 <- diag(rep(0.001, z1))
V_names <- c("V1 V1", "V2 V1", "V2 V2", "V3 V1", "V3 V2", "V3 V3")
colnames(V1) <- rownames(V1) <- V_names

# Weight matrix: W = diag(1/diag(V_Reorder))
# GenomicSEM: W_Reorder = diag(z); diag(W_Reorder) = diag(V_Reorder); W_Reorder = solve(W_Reorder)
W1 <- diag(z1)
diag(W1) <- 1.0 / diag(V1)

model1 <- "F1 =~ NA*V1 + V2 + V3\nF1 ~~ 1*F1"

fit1 <- sem(model1, sample.cov = S1, estimator = "DWLS", se = "standard",
            WLS.V = W1, sample.nobs = 2, optim.dx.tol = 0.01, optim.force.converged = TRUE, ordered = FALSE)

# Extract results
par_table1 <- parTable(fit1)
implied1 <- fitted(fit1)[[1]]
delta1 <- lavInspect(fit1, "delta")
W1_fit <- lavInspect(fit1, "WLS.V")

# Sandwich SE (GenomicSEM uses V_Reorder = V since order is identity for this case)
sand1 <- compute_sandwich(fit1, V1)

# Model chi-square (GenomicSEM uses eigen-decomposition of V)
eig1 <- eigen(V1)
Eig2_1 <- diag(z1); diag(Eig2_1) <- eig1$values
P1 <- eig1$vectors
implied2_1 <- S1 - implied1
eta1 <- as.vector(vech(implied2_1))
Q1 <- as.numeric(t(eta1) %*% P1 %*% solve(Eig2_1) %*% t(P1) %*% eta1)

# Save as JSON
scen1 <- list(
  scenario = "one_factor_3trait_saturated",
  S = S1,
  V_diag = diag(V1),
  model = model1,
  n_traits = 3,
  converged = lavInspect(fit1, "converged"),
  # Parameter estimates
  par_lhs = par_table1$lhs,
  par_op = par_table1$op,
  par_rhs = par_table1$rhs,
  par_free = par_table1$free,
  par_est = par_table1$est,
  # Sandwich SEs
  sandwich_SE = sand1$SE,
  # Model-implied covariance
  implied = unname(implied1),
  # Chi-square
  chisq = Q1,
  df = as.integer(lavInspect(fit1, "fit")["df"]),
  srmr = as.numeric(lavInspect(fit1, "fit")["srmr"]),
  # Known true parameters
  true_loadings = loadings1,
  true_resid_var = resid1,
  true_factor_var = fvar1
)

write_json(scen1, file.path(out_dir, "scenario1_one_factor_3trait.json"), auto_unbox = TRUE, digits = 12)

# =====================================================================
# Scenario 2: 5-trait one-factor model (df=5)
# =====================================================================

cat("Generating scenario 2: 5-trait one-factor...\n")
loadings2 <- c(0.6, 0.5, 0.4, 0.7, 0.3)
resid2 <- c(0.15, 0.20, 0.25, 0.10, 0.30)
fvar2 <- 0.5

S2 <- outer(loadings2, loadings2) * fvar2 + diag(resid2)
colnames(S2) <- rownames(S2) <- paste0("V", 1:5)

z2 <- 15  # 5*6/2
V2 <- diag(rep(0.0005, z2))
colnames(V2) <- rownames(V2) <- colnames(V2)

W2 <- diag(z2)
diag(W2) <- 1.0 / diag(V2)

model2 <- "F1 =~ NA*V1 + V2 + V3 + V4 + V5\nF1 ~~ 1*F1"

fit2 <- sem(model2, sample.cov = S2, estimator = "DWLS", se = "standard",
            WLS.V = W2, sample.nobs = 2, optim.dx.tol = 0.01, optim.force.converged = TRUE, ordered = FALSE)

par_table2 <- parTable(fit2)
implied2 <- fitted(fit2)[[1]]
sand2 <- compute_sandwich(fit2, V2)

# Chi-square
eig2 <- eigen(V2)
Eig2_2 <- diag(z2); diag(Eig2_2) <- eig2$values
P2 <- eig2$vectors
eta2 <- as.vector(vech(S2 - implied2))
Q2 <- as.numeric(t(eta2) %*% P2 %*% solve(Eig2_2) %*% t(P2) %*% eta2)

scen2 <- list(
  scenario = "one_factor_5trait",
  S = S2,
  V_diag = diag(V2),
  model = model2,
  n_traits = 5,
  converged = lavInspect(fit2, "converged"),
  par_lhs = par_table2$lhs,
  par_op = par_table2$op,
  par_rhs = par_table2$rhs,
  par_free = par_table2$free,
  par_est = par_table2$est,
  sandwich_SE = sand2$SE,
  implied = unname(implied2),
  chisq = Q2,
  df = as.integer(lavInspect(fit2, "fit")["df"]),
  srmr = as.numeric(lavInspect(fit2, "fit")["srmr"]),
  true_loadings = loadings2,
  true_resid_var = resid2,
  true_factor_var = fvar2
)

write_json(scen2, file.path(out_dir, "scenario2_one_factor_5trait.json"), auto_unbox = TRUE, digits = 12)

# =====================================================================
# Scenario 3: 4-trait two-factor model
# =====================================================================

cat("Generating scenario 3: 4-trait two-factor...\n")
l_f1 <- c(0.7, 0.5, 0.0, 0.0)
l_f2 <- c(0.0, 0.0, 0.6, 0.4)
f1_var <- 0.4
f2_var <- 0.6
f_cov <- 0.1
resid3 <- c(0.20, 0.25, 0.15, 0.20)

S3 <- outer(l_f1, l_f1) * f1_var + outer(l_f2, l_f2) * f2_var +
      outer(l_f1, l_f2) * f_cov + outer(l_f2, l_f1) * f_cov + diag(resid3)
colnames(S3) <- rownames(S3) <- paste0("V", 1:4)

z3 <- 10  # 4*5/2
V3 <- diag(rep(0.0008, z3))

W3 <- diag(z3)
diag(W3) <- 1.0 / diag(V3)

model3 <- "F1 =~ NA*V1 + V2\nF2 =~ NA*V3 + V4\nF1 ~~ F2"

fit3 <- sem(model3, sample.cov = S3, estimator = "DWLS", se = "standard",
            WLS.V = W3, sample.nobs = 2, optim.dx.tol = 0.01, optim.force.converged = TRUE, ordered = FALSE)

par_table3 <- parTable(fit3)
implied3 <- fitted(fit3)[[1]]
sand3 <- compute_sandwich(fit3, V3)

eig3 <- eigen(V3)
Eig2_3 <- diag(z3); diag(Eig2_3) <- eig3$values
P3 <- eig3$vectors
eta3 <- as.vector(vech(S3 - implied3))
Q3 <- as.numeric(t(eta3) %*% P3 %*% solve(Eig2_3) %*% t(P3) %*% eta3)

scen3 <- list(
  scenario = "two_factor_4trait",
  S = S3,
  V_diag = diag(V3),
  model = model3,
  n_traits = 4,
  converged = lavInspect(fit3, "converged"),
  par_lhs = par_table3$lhs,
  par_op = par_table3$op,
  par_rhs = par_table3$rhs,
  par_free = par_table3$free,
  par_est = par_table3$est,
  sandwich_SE = sand3$SE,
  implied = unname(implied3),
  chisq = Q3,
  df = as.integer(lavInspect(fit3, "fit")["df"]),
  srmr = as.numeric(lavInspect(fit3, "fit")["srmr"])
)

write_json(scen3, file.path(out_dir, "scenario3_two_factor_4trait.json"), auto_unbox = TRUE, digits = 12)

# =====================================================================
# Scenario 4: nearPD test
# =====================================================================

cat("Generating scenario 4: nearPD test...\n")

# Non-PD matrix
bad_mat <- matrix(c(1, 0.9, 0.9,
                    0.9, 1, 0.9,
                    0.9, 0.9, 1), 3, 3)
# eigenvalues: 2.8, 0.1, 0.1 → actually PD!
# Let's make it non-PD:
bad_mat2 <- matrix(c(1, 2, 0,
                     2, 1, 0,
                     0, 0, 1), 3, 3)
# eigenvalues: 3, -1, 1

nearpd_result <- as.matrix(nearPD(bad_mat2, corr = FALSE)$mat)
nearpd_eigenvalues <- as.numeric(eigen(nearpd_result, symmetric = TRUE, only.values = TRUE)$values)

scen4 <- list(
  scenario = "near_pd",
  input = bad_mat2,
  output = nearpd_result,
  output_eigenvalues = nearpd_eigenvalues,
  is_pd = min(nearpd_eigenvalues) > 0
)

write_json(scen4, file.path(out_dir, "scenario4_near_pd.json"), auto_unbox = TRUE, digits = 12)

# =====================================================================
# Scenario 5: Liability conversion
# =====================================================================

cat("Generating scenario 5: liability conversion...\n")

prevs <- c(0.01, 0.05, 0.10, 0.50)
samp_prev <- 0.5

conv_factors <- sapply(prevs, function(pp) {
  (pp^2 * (1-pp)^2) / (samp_prev * (1-samp_prev) * dnorm(qnorm(1-pp))^2)
})

scen5 <- list(
  scenario = "liability_conversion",
  population_prev = prevs,
  sample_prev = samp_prev,
  conversion_factors = conv_factors
)

write_json(scen5, file.path(out_dir, "scenario5_liability.json"), auto_unbox = TRUE, digits = 12)

cat("All golden fixtures generated in", out_dir, "\n")