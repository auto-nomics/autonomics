## Golden generator for tests/svy_coxph_reference.rs — svycoxph parity.
##
## survey::svycoxph (rescale = TRUE) fits coxph with rescaled probability
## weights and feeds `resid(fit, "dfbeta", weighted = TRUE)` — weighted
## score residuals with risk-set mean centering — to svyrecvar.  Event
## times are generated tie-free so Breslow == Efron.
##
## Run locally (survey 4.5 + survival + jsonlite):
##   Rscript stat_crates/survey/tests/golden/gen_svy_coxph_reference.R
suppressPackageStartupMessages({library(survey); library(survival); library(jsonlite)})

base_dir <- "stat_crates/survey/tests/golden"

set.seed(20261005)
n <- 480
st  <- rep(1:8, each = 60)
psu <- rep(rep(1:5, each = 12), times = 8)
x1 <- rnorm(n)
x2 <- rnorm(n)
lp <- 0.5 * x1 - 0.3 * x2
tt <- rexp(n, rate = 0.05 * exp(lp))
cc <- rexp(n, rate = 0.02)
time <- pmin(tt, cc)
event <- as.integer(tt <= cc)
stopifnot(!any(duplicated(time)))  # no ties: Breslow == Efron
wt <- runif(n, 0.2, 3)
dat <- data.frame(st, psu, x1, x2, time, event, wt)
write.csv(dat, file.path(base_dir, "coxph_data.csv"), row.names = FALSE)

dsn <- svydesign(ids = ~psu, strata = ~st, weights = ~wt, data = dat, nest = TRUE)
f <- svycoxph(Surv(time, event) ~ x1 + x2, design = dsn)

ref <- list(
  survey_version = as.character(packageVersion("survey")),
  n = n,
  n_events = sum(event),
  coefficients = as.numeric(coef(f)),
  vcov = as.matrix(vcov(f)),
  se = as.numeric(SE(f)),
  degf = degf(dsn)
)
write_json(ref, file.path(base_dir, "svy_coxph_reference.json"),
           auto_unbox = TRUE, digits = 16, matrix = "rowmajor")
cat("regenerated svy_coxph goldens\n")
