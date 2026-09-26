## Golden generator for tests/svy_rcs_r_reference.rs — design-based RCS parity.
##
## Uses EXPLICIT knots so both sides share the knot positions; the Harrell /
## Stone-Koo basis is implemented independently in R here (the same closed
## form as epi::rcs::rcs_basis), fitted with survey::svyglm and tested with
## regTermTest(method="Wald"). Weighted-knot placement has its own unit
## tests in stat_crates/epi.
##
## Run inside the survey probe image (rocker/r-ver:4.5.1 + survey + jsonlite),
## from the repo root:
##   podman run -i --rm -v <repo>:/repo:Z -w /repo survey-probe Rscript \
##     crates/node-bundles/nodes-survey/tests/golden/gen_svy_rcs_reference.R
suppressPackageStartupMessages({library(survey); library(jsonlite)})
stopifnot(requireNamespace("survey", quietly = TRUE))

base_dir <- "crates/node-bundles/nodes-survey/tests/golden"
dat <- read.csv(file.path(base_dir, "rcs_data.csv"))
knots <- c(4.0, 5.5, 7.0, 8.5)

## Harrell restricted cubic spline basis (identical closed form to
## epi::rcs::rcs_basis): k−2 columns, linear beyond the boundary knots.
rcs_basis_r <- function(x, knots) {
  k <- length(knots)
  tmin <- knots[1]; tmax <- knots[k]
  denom <- (tmax - tmin)^2
  pos3 <- function(v) pmax(v, 0)^3
  sapply(2:(k - 1), function(j) {
    lj <- (tmax - knots[j]) / denom
    mj <- (knots[j] - tmin) / denom
    pos3(x - knots[j]) - lj * pos3(x - tmin) - mj * pos3(x - tmax)
  })
}

basis <- rcs_basis_r(dat$xx, knots)
dat$basis1 <- basis[, 1]
dat$basis2 <- basis[, 2]
dsn <- svydesign(ids = ~psu3, strata = ~st3, weights = ~wt3, data = dat)

fit_g <- svyglm(yy_lin ~ xx + basis1 + basis2 + cc, design = dsn)
fit_b <- svyglm(yy_bin ~ xx + basis1 + basis2 + cc, design = dsn,
                family = quasibinomial())

nl_g <- regTermTest(fit_g, ~basis1 + basis2, method = "Wald")
nl_b <- regTermTest(fit_b, ~basis1 + basis2, method = "Wald")
ov_g <- regTermTest(fit_g, ~xx + basis1 + basis2 + cc, method = "Wald")

## eta at three grid points with cc at its weighted mean.
wmean_cc <- sum(dat$wt3 * dat$cc) / sum(dat$wt3)
eta_at <- function(fit, x) {
  b <- rcs_basis_r(c(x), knots)
  x0 <- c(1, x, b[1], b[2], wmean_cc)
  sum(x0 * coef(fit))
}

ref <- list(
  survey_version = as.character(packageVersion("survey")),
  knots = knots,
  gaussian = list(
    coefficients = as.numeric(coef(fit_g)),
    nonlinear = list(F = as.numeric(nl_g$Ftest), p = as.numeric(nl_g$p),
                     ndf = as.numeric(nl_g$df), ddf = as.numeric(nl_g$ddf)),
    overall = list(F = as.numeric(ov_g$Ftest), p = as.numeric(ov_g$p),
                   ndf = as.numeric(ov_g$df), ddf = as.numeric(ov_g$ddf)),
    eta = sapply(c(4.0, 6.5, 9.0), eta_at, fit = fit_g)
  ),
  binomial = list(
    coefficients = as.numeric(coef(fit_b)),
    nonlinear = list(F = as.numeric(nl_b$Ftest), p = as.numeric(nl_b$p),
                     ndf = as.numeric(nl_b$df), ddf = as.numeric(nl_b$ddf))
  ),
  mean_cc_weighted = wmean_cc
)
write_json(ref, file.path(base_dir, "svy_rcs_reference.json"),
           auto_unbox = TRUE, digits = 16)
cat("regenerated svy_rcs goldens\n")
