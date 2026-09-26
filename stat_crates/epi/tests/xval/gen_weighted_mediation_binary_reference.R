## Golden generator for tests/xval_weighted_mediation_binary.rs.
##
## Regenerates mediation_binary_data.csv + weighted_mediation_binary_reference.json
## from R survey 4.5 (svyglm gaussian mediator + quasibinomial outcome), with
## the VanderWeele decomposition evaluated at the weighted covariate means.
##
## Run inside the one-shot probe image (see docs/design/survey-factor-rrr-mediation-nodes.md):
##   podman build -t survey-probe .   # rocker/r-ver:4.5.1 + survey + jsonlite
##   podman run -i --rm -v <repo>:/repo:Z -w /repo survey-probe \
##     Rscript stat_crates/epi/tests/xval/gen_weighted_mediation_binary_reference.R
suppressPackageStartupMessages({library(survey); library(jsonlite)})
stopifnot(requireNamespace("survey", quietly = TRUE))

out_dir <- "stat_crates/epi/tests/xval"

set.seed(31415)
n2 <- 600
st2 <- rep(1:6, each = 100)
psu2 <- rep(1:30, each = 20)
x2 <- rbinom(n2, 1, 0.4)
c1 <- rnorm(n2, 0, 1)
m2 <- 0.5 + 0.8 * x2 + 0.4 * c1 + rnorm(n2) * 0.5
eta2 <- -1.2 + 0.6 * x2 + 0.5 * m2 + 0.3 * c1
y2 <- rbinom(n2, 1, plogis(eta2))
wt2 <- runif(n2, 2000, 8000)
mdat <- data.frame(st2, psu2, x2, c1, m2, y2, wt2)
dsn2 <- svydesign(ids = ~psu2, strata = ~st2, weights = ~wt2, data = mdat)

fit_m <- svyglm(m2 ~ x2 + c1, design = dsn2, family = gaussian())
fit_y <- svyglm(y2 ~ x2 + m2 + c1, design = dsn2, family = quasibinomial())
a <- coef(fit_m); b <- coef(fit_y)
wbar <- function(v) sum(wt2 * v) / sum(wt2)
m_uc <- a[1] + a[2] * 0 + a[3] * wbar(c1)
ref <- list(
  survey_version = as.character(packageVersion("survey")),
  mediation_binary = list(
    alpha = as.numeric(a), beta = as.numeric(b),
    mean_c1_weighted = wbar(c1),
    m_under_control = as.numeric(m_uc),
    cde = as.numeric(b[2]), nie = as.numeric(b[3] * a[2]),
    te  = as.numeric(b[2] + b[3] * a[2]),
    prop_mediated = as.numeric(b[3] * a[2] / (b[2] + b[3] * a[2]))
  )
)
write.csv(mdat, file.path(out_dir, "mediation_binary_data.csv"), row.names = FALSE)
write_json(ref, file.path(out_dir, "weighted_mediation_binary_reference.json"),
           auto_unbox = TRUE, digits = 16)
cat("regenerated", out_dir, "goldens\n")
