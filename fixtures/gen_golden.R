#!/usr/bin/env Rscript
# =============================================================================
# Golden-test data generator for the Rust SuSiE-RSS port.
#
# Faithfully follows susieR's own reference-test pattern (see
#   tests/testthat/reference/test_susie_rss_reference.R):
#   set.seed(seed); n=500, p=50, 3 causal variants; compute suff-stats,
#   R = cov2cor(XtX), z = betahat/sebetahat.
#
# For each scenario we dump BOTH the input (z, R, n, bhat, shat, var_y,
# prior_weights, maf) AND the full model output (alpha, mu, mu2, V, lbf,
# KL, sigma2, elbo, niter, converged, pip, sets) as a single JSON file.
#
# Run:  Rscript fixtures/gen_golden.R
# Output: fixtures/golden/*.json
# =============================================================================

suppressMessages({
  library(susieR)
  library(jsonlite)
})

out_dir <- "fixtures/golden"
dir.create(out_dir, showWarnings = FALSE, recursive = TRUE)

`%||%` <- function(a, b) if (is.null(a)) b else a

# -----------------------------------------------------------------------------
# Serialize a susie fit to a JSON-safe list
# -----------------------------------------------------------------------------
fit_to_list <- function(fit) {
  sets_json <- NULL
  if (!is.null(fit$sets) && !is.null(fit$sets$cs)) {
    purity <- fit$sets$purity
    if (!is.null(purity)) purity <- as.matrix(purity)
    sets_json <- list(
      cs       = lapply(fit$sets$cs, function(v) v - 1L),  # 0-indexed
      cs_index = if (!is.null(fit$sets$cs_index)) fit$sets$cs_index - 1L else NULL,
      purity   = purity,
      coverage = fit$sets$coverage,
      requested_coverage = fit$sets$requested_coverage
    )
  }
  list(
    alpha        = fit$alpha,
    mu           = fit$mu,
    mu2          = fit$mu2,
    V            = fit$V,
    lbf          = fit$lbf,
    lbf_variable = fit$lbf_variable,
    KL           = fit$KL,
    sigma2       = fit$sigma2,
    elbo         = fit$elbo,
    niter        = fit$niter,
    converged    = fit$converged,
    pip          = fit$pip,
    intercept    = fit$intercept,
    null_index   = if (is.null(fit$null_index)) 0L else fit$null_index,
    sets         = sets_json
  )
}

# -----------------------------------------------------------------------------
# Simulate the standard test dataset for a given seed
# -----------------------------------------------------------------------------
sim_data <- function(seed) {
  set.seed(seed)
  n <- 500; p <- 50
  X <- matrix(rnorm(n * p), n, p)
  beta <- rep(0, p); beta[c(5, 10, 20)] <- c(0.5, 0.4, 0.3)
  y <- as.vector(X %*% beta + rnorm(n))
  input_ss <- compute_suff_stat(X, y, standardize = TRUE)
  ss <- univariate_regression(X, y)
  R <- with(input_ss, cov2cor(XtX)); R <- (R + t(R)) / 2
  z <- with(ss, betahat / sebetahat)
  list(z=z, R=R, bhat=ss$betahat, shat=ss$sebetahat, var_y=var(y),
       n=as.integer(n), p=as.integer(p), seed=seed)
}

# -----------------------------------------------------------------------------
# Run one scenario: simulate data, build real args, fit, dump JSON
#   spec: a list of overrides. Keys "z","R","bhat","shat","var_y","prior_weights"
#   that are TRUE in `spec` get replaced with the simulated value.
# -----------------------------------------------------------------------------
run <- function(name, seed, spec) {
  d <- sim_data(seed)

  # Build the actual args list from the spec
  args <- list()
  for (key in names(spec)) {
    val <- spec[[key]]
    if (isTRUE(val)) {
      args[[key]] <- d[[key]]
    } else {
      args[[key]] <- val
    }
  }

  fit <- do.call(susie_rss, args)

  # Build input JSON (always include all simulated data)
  input <- list(z=d$z, R=d$R, n=d$n, bhat=d$bhat, shat=d$shat,
                var_y=d$var_y, p=d$p, seed=d$seed)
  if (!is.null(spec$prior_weights) && isTRUE(spec$prior_weights))
    input$prior_weights <- d$prior_weights

  payload <- list(name=name, args=args, input=input, fit=fit_to_list(fit))
  write_json(payload, file.path(out_dir, paste0(name, ".json")),
             digits=15, auto_unbox=TRUE, matrix="rowmajor")
  cat(sprintf("  %-45s niter=%-3d conv=%-5s elbo=%.4f n_cs=%d\n",
              name, fit$niter, fit$converged, tail(fit$elbo, 1),
              length(fit$sets$cs %||% list())))
}

# =============================================================================
# Scenario matrix
# =============================================================================

cat("Generating golden scenarios for SuSiE-RSS port...\n\n")

# --- z + R + n, three prior methods ---
run("z_n_optim",    1, list(z=TRUE, R=TRUE, n=500, L=10, estimate_prior_method="optim"))
run("z_n_EM",       1, list(z=TRUE, R=TRUE, n=500, L=10, estimate_prior_method="EM"))
run("z_n_simple",   1, list(z=TRUE, R=TRUE, n=500, L=10, estimate_prior_method="simple"))

# --- z + R without n ---
run("z_non_optim",  4, list(z=TRUE, R=TRUE, L=10, estimate_prior_method="optim"))
run("z_non_EM",     4, list(z=TRUE, R=TRUE, L=10, estimate_prior_method="EM"))
run("z_non_simple", 4, list(z=TRUE, R=TRUE, L=10, estimate_prior_method="simple"))

# --- Different L values ---
run("L1_optim",   5, list(z=TRUE, R=TRUE, n=500, L=1,  estimate_prior_method="optim"))
run("L5_optim",   5, list(z=TRUE, R=TRUE, n=500, L=5,  estimate_prior_method="optim"))
run("L20_optim",  5, list(z=TRUE, R=TRUE, n=500, L=20, estimate_prior_method="optim"))
run("L1_EM",      5, list(z=TRUE, R=TRUE, n=500, L=1,  estimate_prior_method="EM"))
run("L5_EM",      5, list(z=TRUE, R=TRUE, n=500, L=5,  estimate_prior_method="EM"))
run("L20_EM",     5, list(z=TRUE, R=TRUE, n=500, L=20, estimate_prior_method="EM"))
run("L1_simple",  5, list(z=TRUE, R=TRUE, n=500, L=1,  estimate_prior_method="simple"))
run("L5_simple",  5, list(z=TRUE, R=TRUE, n=500, L=5,  estimate_prior_method="simple"))
run("L20_simple", 5, list(z=TRUE, R=TRUE, n=500, L=20, estimate_prior_method="simple"))

# --- estimate_prior_variance = FALSE ---
run("fixV_optim",  6, list(z=TRUE, R=TRUE, n=500, L=10, estimate_prior_variance=FALSE, estimate_prior_method="optim"))
run("fixV_EM",     6, list(z=TRUE, R=TRUE, n=500, L=10, estimate_prior_variance=FALSE, estimate_prior_method="EM"))
run("fixV_simple", 6, list(z=TRUE, R=TRUE, n=500, L=10, estimate_prior_variance=FALSE, estimate_prior_method="simple"))

# --- estimate_residual_variance = TRUE ---
run("estR_var_optim",  7, list(z=TRUE, R=TRUE, n=500, L=10, estimate_residual_variance=TRUE, estimate_prior_method="optim"))
run("estR_var_EM",     7, list(z=TRUE, R=TRUE, n=500, L=10, estimate_residual_variance=TRUE, estimate_prior_method="EM"))
run("estR_var_simple", 7, list(z=TRUE, R=TRUE, n=500, L=10, estimate_residual_variance=TRUE, estimate_prior_method="simple"))

# --- estimate_residual_variance = FALSE, residual_variance = 1.0 ---
run("fixR_var_optim",  8, list(z=TRUE, R=TRUE, n=500, L=10, estimate_residual_variance=FALSE, residual_variance=1.0, estimate_prior_method="optim"))
run("fixR_var_EM",     8, list(z=TRUE, R=TRUE, n=500, L=10, estimate_residual_variance=FALSE, residual_variance=1.0, estimate_prior_method="EM"))
run("fixR_var_simple", 8, list(z=TRUE, R=TRUE, n=500, L=10, estimate_residual_variance=FALSE, residual_variance=1.0, estimate_prior_method="simple"))

# --- prior_weights ---
run("priorW_optim",  9, list(z=TRUE, R=TRUE, n=500, L=10, prior_weights=TRUE, estimate_prior_method="optim"))
run("priorW_EM",     9, list(z=TRUE, R=TRUE, n=500, L=10, prior_weights=TRUE, estimate_prior_method="EM"))
run("priorW_simple", 9, list(z=TRUE, R=TRUE, n=500, L=10, prior_weights=TRUE, estimate_prior_method="simple"))

# --- scaled_prior_variance ---
run("scaleV05_optim",  10, list(z=TRUE, R=TRUE, n=500, L=10, scaled_prior_variance=0.5, estimate_prior_method="optim"))
run("scaleV05_EM",     10, list(z=TRUE, R=TRUE, n=500, L=10, scaled_prior_variance=0.5, estimate_prior_method="EM"))
run("scaleV05_simple", 10, list(z=TRUE, R=TRUE, n=500, L=10, scaled_prior_variance=0.5, estimate_prior_method="simple"))

# --- bhat/shat/var_y ---
run("bhat_shat_optim",  11, list(bhat=TRUE, shat=TRUE, R=TRUE, n=500, L=10, var_y=TRUE, estimate_prior_method="optim"))
run("bhat_shat_EM",     11, list(bhat=TRUE, shat=TRUE, R=TRUE, n=500, L=10, var_y=TRUE, estimate_prior_method="EM"))
run("bhat_shat_simple", 11, list(bhat=TRUE, shat=TRUE, R=TRUE, n=500, L=10, var_y=TRUE, estimate_prior_method="simple"))

# --- coverage = 0.99 ---
run("cov99_optim",  12, list(z=TRUE, R=TRUE, n=500, L=10, coverage=0.99, estimate_prior_method="optim"))
run("cov99_simple", 12, list(z=TRUE, R=TRUE, n=500, L=10, coverage=0.99, estimate_prior_method="simple"))

# --- min_abs_corr = 0.7 ---
run("mincorr07_optim",  13, list(z=TRUE, R=TRUE, n=500, L=10, min_abs_corr=0.7, estimate_prior_method="optim"))
run("mincorr07_simple", 13, list(z=TRUE, R=TRUE, n=500, L=10, min_abs_corr=0.7, estimate_prior_method="simple"))

# --- check_null_threshold ---
run("nullthresh_optim",  16, list(z=TRUE, R=TRUE, n=500, L=10, check_null_threshold=0.1, estimate_prior_method="optim"))
run("nullthresh_simple", 16, list(z=TRUE, R=TRUE, n=500, L=10, check_null_threshold=0.1, estimate_prior_method="simple"))

# --- null_weight ---
run("nullW_optim",  18, list(z=TRUE, R=TRUE, n=500, L=10, null_weight=0.1, estimate_prior_method="optim"))
run("nullW_simple", 18, list(z=TRUE, R=TRUE, n=500, L=10, null_weight=0.1, estimate_prior_method="simple"))

# --- z_method = "score" ---
run("zscore_score", 1, list(z=TRUE, R=TRUE, n=500, L=10, z_method="score", estimate_prior_method="optim"))

cat("\nDone. JSON files written to", out_dir, "\n")
