#!/usr/bin/env Rscript
# Extracts `data_hlme` from lcmm + reference fits to CSV + JSON for cross-validation.
#
# Run with r45 conda env:
#   R_HOME=/home/wjx/miniconda3/envs/r45/lib/R \
#   PATH=/home/wjx/miniconda3/envs/r45/bin:$PATH \
#   Rscript tests/extract_data.R
#
# Restoring from aliyun:autonomics-data/lcmm/test-data/ via rclone:
#   rclone copy aliyun:autonomics-data/lcmm/test-data/ bio_crates/lcmm/tests/

suppressMessages({
  library(lcmm)
  library(jsonlite)
})

data(data_hlme)

out_dir <- commandArgs(trailingOnly = TRUE)[1]
if (is.na(out_dir)) out_dir <- "tests"
dir.create(out_dir, showWarnings = FALSE, recursive = TRUE)

# --- write data CSV --------------------------------------------------------
write.csv(data_hlme, file.path(out_dir, "data_hlme.csv"), row.names = FALSE)

# --- helper to serialize a fitted hlme object -----------------------------
serialize_fit <- function(tag, fit, init_B = NULL) {
  ppi <- if (fit$ng > 1) as.matrix(fit$pprob[, -(1:2)]) else matrix(nrow = 0, ncol = 0)
  list(
    tag            = tag,
    ng             = fit$ng,
    idiag          = fit$idiag,
    N              = fit$N,
    conv           = fit$conv,
    loglik         = fit$loglik,
    niter          = fit$niter,
    gconv          = fit$gconv,
    best           = fit$best,
    names_best     = names(fit$best),
    V_upper        = fit$V,
    AIC            = fit$AIC,
    BIC            = fit$BIC,
    cholesky       = fit$cholesky,
    Xnames         = fit$Xnames,
    idea0          = fit$idea0,
    idprob0        = fit$idprob0,
    idg0           = fit$idg0,
    idcor0         = fit$idcor0,
    ns             = fit$ns,
    ppi            = unlist(split(ppi, seq_len(nrow(ppi)))),
    ppi_dim        = c(nrow(ppi), ncol(ppi)),
    pprob_class    = if (fit$ng > 1) fit$pprob$class else integer(0),
    predRE         = if (!is.null(fit$predRE)) as.matrix(fit$predRE[, -1]) else matrix(nrow = 0, ncol = 0),
    classpredRE    = if (!is.null(fit$classpredRE)) as.matrix(fit$classpredRE[, -(1:2)]) else matrix(nrow = 0, ncol = 0),
    init_B         = init_B,
    wRandom        = fit$wRandom,
    b0Random       = fit$b0Random
  )
}

# --- helper to intercept the initial B vector passed to mla() ---------------
# We trace the optimization by re-evaluating the loglik at the extracted init.
# For B=<hlme_obj>, the init B is computed by hlme()'s internal transformation.
extract_init_B <- function(fit_call) {
  # Re-run the call but with maxiter=0 to get the init B without optimizing.
  m0 <- tryCatch(eval(fit_call), error = function(e) NULL)
  if (is.null(m0)) return(NULL)
  m0$best
}

# --- fits ------------------------------------------------------------------
set.seed(1)
cat("Fitting m1 (ng=1) ...\n")
m1 <- hlme(Y ~ Time * X1, random = ~Time, subject = "ID", ng = 1,
           data = data_hlme)

cat("Fitting m2a (ng=2, init from m1) ...\n")
m2a <- hlme(Y ~ Time * X1, mixture = ~Time, random = ~Time,
            classmb = ~X2 + X3, subject = "ID", ng = 2,
            data = data_hlme, B = m1)

cat("Fitting m3a (ng=3, init from m1) ...\n")
m3a <- tryCatch(
  hlme(Y ~ Time * X1, mixture = ~Time, random = ~Time,
       classmb = ~X2 + X3, subject = "ID", ng = 3,
       data = data_hlme, B = m1),
  error = function(e) NULL
)

# Note: hlme auto-adds classmb=~1 when ng>1 and classmb missing, contributing
# (ng-1) NPROB params. mixture=~Time has intercept+Time class-specific, so
# NEF = 2*ng. With no random/nwg/cor: NPM = (ng-1) + 2*ng + 0 + 0 + 0 + 1.

cat("Fitting gbtm1 (ng=1 GBTM, no random) ...\n")
gbtm1 <- hlme(Y ~ Time, subject = "ID", ng = 1, data = data_hlme)

# Use B=<prior hlme> mechanism to seed the latent-class GBTM fits — this is the
# canonical workflow described in ?hlme and avoids the degenerate local maxima
# that hand-written starting values fall into for ng>1 GBTM/LCGA.

cat("Fitting gbtm2 (pure GBTM/LCGA, ng=2, no random) ...\n")
gbtm2 <- hlme(Y ~ Time, mixture = ~Time, subject = "ID", ng = 2,
              data = data_hlme, B = gbtm1)

cat("Fitting gbtm3 (pure GBTM/LCGA, ng=3, no random) ...\n")
gbtm3 <- hlme(Y ~ Time, mixture = ~Time, subject = "ID", ng = 3,
              data = data_hlme, B = gbtm1)

cat("Fitting m1_idiag (ng=1, diagonal RE cov) ...\n")
m1_idiag <- hlme(Y ~ Time * X1, random = ~Time, subject = "ID", ng = 1,
                 idiag = TRUE, data = data_hlme)

cat("Fitting m2a_nwg (ng=2, class-scaled RE) ...\n")
m2a_nwg <- hlme(Y ~ Time * X1, mixture = ~Time, random = ~Time,
                classmb = ~X2 + X3, subject = "ID", ng = 2,
                data = data_hlme, B = m1, nwg = TRUE)

# --- serialize -------------------------------------------------------------
fits <- list(
  serialize_fit("m1",       m1),
  serialize_fit("m2a",      m2a),
  serialize_fit("m3a",      m3a, init_B = m1$best),
  serialize_fit("gbtm1",    gbtm1),
  serialize_fit("gbtm2",    gbtm2, init_B = gbtm1$best),
  serialize_fit("gbtm3",    gbtm3, init_B = gbtm1$best),
  serialize_fit("m1_idiag", m1_idiag),
  serialize_fit("m2a_nwg",  m2a_nwg)
)

cat("Fitting predictY for m2a ...\n")
newdata <- data.frame(Time = c(0, 2, 5), X1 = c(0, 0, 0),
                      X2 = c(0, 0, 0), X3 = c(0, 0, 0))
pred <- predictY(m2a, newdata = newdata, var.time = "Time", draws = FALSE)

pred_list <- list(
  tag       = "predictY_m2a",
  newdata   = newdata,
  pred      = pred$pred
)

cat("Fitting predictY for gbtm3 ...\n")
newdata2 <- data.frame(Time = seq(0, 6, by = 1))
pred_gbtm <- if (gbtm3$conv %in% c(1, 2, 3)) {
  predictY(gbtm3, newdata = newdata2, var.time = "Time", draws = FALSE)
} else {
  list(pred = NULL)
}

all_json <- list(
  lcmm_version = as.character(packageVersion("lcmm")),
  marqLevAlg_version = as.character(packageVersion("marqLevAlg")),
  r_version = R.version.string,
  fits = fits,
  predictY_m2a = pred_list,
  predictY_gbtm3 = list(tag = "predictY_gbtm3", newdata = newdata2, pred = pred_gbtm$pred)
)

write_json(all_json, file.path(out_dir, "hlme_golden.json"),
           auto_unbox = TRUE, digits = NA, null = "null")

cat("\nDone.\n")
cat("Wrote: ", file.path(out_dir, "data_hlme.csv"), "\n")
cat("Wrote: ", file.path(out_dir, "hlme_golden.json"), "\n")
