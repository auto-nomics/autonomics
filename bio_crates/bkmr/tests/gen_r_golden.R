#!/usr/bin/env Rscript
# Generate golden BKMR output from the reference R package (bkmr 0.2.2.9000)
# for cross-validation against the Rust port.
#
# Usage (r45 conda env: R 4.5.3 + bkmr):
#   export R_HOME=$HOME/miniconda3/envs/r45/lib/R
#   export PATH=$HOME/miniconda3/envs/r45/bin:$PATH
#   export LD_LIBRARY_PATH=$HOME/miniconda3/envs/r45/lib:$LD_LIBRARY_PATH
#   Rscript tests/gen_r_golden.R tests/bkmr_golden.json
#
# The Rust port reproduces R's RNG stream bit-exactly (MT19937 + inversion
# rnorm + Ahrens-Dieter rexp/rgamma + truncnorm rejection), so chains starting
# from identical set.seed should match to ~1e-12 (limited by faer vs LAPACK
# Cholesky agreement at ~1e-15 and statrs vs Rmath lgamma/pnorm at ~1e-12).

suppressMessages({
  library(bkmr)
  library(jsonlite)
})

out_file <- commandArgs(trailingOnly = TRUE)[1]
if (is.na(out_file)) stop("Usage: Rscript gen_r_golden.R <out.json>")

# Common settings: small n, M=4, 100 iterations for fast cross-validation.
# Explicit starting values to decouple from R's lm() (which uses LAPACK QR
# and would differ from faer's OLS).
ITER <- 100

run_scenario <- function(name, varsel, r_prior = "invunif", seed = 111,
                         n = 50, M = 4, hfun = 3, Zgen = "norm",
                         starting.values = NULL) {
  set.seed(seed)
  dat <- SimData(n = n, M = M, hfun = hfun, Zgen = Zgen)

  cp <- list(r.prior = r_prior)
  if (is.null(starting.values)) {
    # Use explicit starting values that don't depend on lm()
    starting.values <- list(beta = 0, sigsq.eps = 0.5, r = rep(1, M),
                            lambda = 10, delta = rep(1, M), h.hat = 1)
  }

  set.seed(seed)
  fit <- kmbayes(y = dat$y, Z = dat$Z, X = dat$X, iter = ITER,
                 verbose = FALSE, varsel = varsel,
                 control.params = cp,
                 starting.values = starting.values)

  # Extract chain values for cross-validation
  list(
    name = name,
    seed = seed,
    n = n, M = M, hfun = hfun, Zgen = Zgen,
    varsel = varsel, r_prior = r_prior,
    iter = ITER,
    y = dat$y,
    Z = as.numeric(dat$Z),
    X = as.numeric(dat$X),
    beta = as.numeric(t(fit$beta)),
    lambda = as.numeric(fit$lambda),
    sigsq_eps = fit$sigsq.eps,
    r = as.numeric(t(fit$r)),
    delta = as.numeric(t(fit$delta)),
    acc_lambda = as.numeric(fit$acc.lambda),
    acc_r = as.numeric(t(fit$acc.r)),
    acc_rdelta = fit$acc.rdelta,
    move_type = fit$move.type
  )
}

scenarios <- list()
scenarios$gaussian_novarsel <- run_scenario("gaussian_novarsel", varsel = FALSE)
scenarios$gaussian_varsel <- run_scenario("gaussian_varsel", varsel = TRUE)
scenarios$gaussian_varsel_gamma <-
  run_scenario("gaussian_varsel_gamma", varsel = TRUE, r_prior = "gamma", seed = 222)
scenarios$gaussian_novarsel_unif <-
  run_scenario("gaussian_novarsel_unif", varsel = FALSE, Zgen = "unif", seed = 333)

out <- list(scenarios = scenarios)
writeLines(toJSON(out, auto_unbox = TRUE, digits = 17), out_file)
cat("wrote", out_file, "\n")
