## Golden generator for tests/svy_ivreg_reference.rs — design-based 2SLS
## variance parity with R survey::svyivreg.
##
## survey::svyivreg needs AER (not installed locally), so the R side
## transcribes the authoritative semantics from the package sources:
##   AER::ivreg.fit — xz  = lm.wfit(Z, X, w)$fitted.values   (W-projection)
##                    fit = lm.wfit(xz, y, w)
##                    res = y - X %*% coef(fit)              (ORIGINAL X)
##                    ucov = chol2inv(qr.R(fit$qr))          # (X̂ᵀWX̂)⁻¹
##   AER::model.matrix.ivreg — default component "projected" (xz)
##   survey:::svyivreg.survey.design —
##                    U    = estfun.ivreg / w = res * xz
##                    infl = U %*% cov.unscaled
##                    v    = vcov(svytotal(infl, design))
##
## Run locally (survey 4.5 + jsonlite):
##   Rscript stat_crates/survey/tests/golden/gen_svy_ivreg_reference.R
suppressPackageStartupMessages({library(survey); library(jsonlite)})

base_dir <- "stat_crates/survey/tests/golden"

set.seed(20261004)
n <- 480
st  <- rep(1:8, each = 60)
psu <- rep(rep(1:5, each = 12), times = 8)  # nested within strata
z1  <- rnorm(n)                              # instrument
u   <- rnorm(n)                              # shared confounder
x1  <- 0.6 * z1 + u + 0.5 * rnorm(n)         # endogenous regressor
exo1 <- rnorm(n)
y   <- 1 + 0.8 * x1 + 0.4 * exo1 + 1.5 * u + rnorm(n)
wt  <- runif(n, 0.2, 3)
dat <- data.frame(st, psu, z1, x1, exo1, y, wt)
write.csv(dat, file.path(base_dir, "ivreg_data.csv"), row.names = FALSE)

## --- AER-faithful weighted 2SLS (raw sampling weights) -------------------
one <- rep(1, n)
X <- as.matrix(cbind(x1, exo1, one))  # regressors: endogenous, exogenous, const
Z <- as.matrix(cbind(z1, exo1, one))  # instruments + exogenous + const
w <- dat$wt
xz  <- lm.wfit(Z, X, w)$fitted.values # W-projection X̂ (AER ivreg.fit)
fit <- lm.wfit(xz, y, w)
beta <- coef(fit)
res  <- as.vector(y - X %*% beta)     # residuals against ORIGINAL X
ucov <- chol2inv(qr.R(fit$qr))        # (X̂ᵀWX̂)⁻¹

dsn  <- svydesign(ids = ~psu, strata = ~st, weights = ~wt, data = dat, nest = TRUE)
infl <- (res * xz) %*% ucov           # survey: estfun/w %*% cov.unscaled
v    <- vcov(svytotal(as.data.frame(infl), dsn))

ref <- list(
  survey_version = as.character(packageVersion("survey")),
  n = n,
  coefficients = as.numeric(beta),
  vcov = as.matrix(v),
  se = sqrt(diag(as.matrix(v))),
  degf = degf(dsn)
)
write_json(ref, file.path(base_dir, "svy_ivreg_reference.json"),
           auto_unbox = TRUE, digits = 16, matrix = "rowmajor")
cat("regenerated svy_ivreg goldens\n")
