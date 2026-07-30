#!/usr/bin/env Rscript
# Golden fixture generator for the Rust LCV cross-validation test.
#
# Sources the upstream R reference (reference/LCV/R/) and produces:
#   fixtures/scenarioA.tsv  + scenarioA_result.json — strong causality (gcp>0)
#   fixtures/scenarioB.tsv  + scenarioB_result.json — reversed (gcp<0)
#
# Regenerate (from repo root):
#   Rscript bio_crates/lcv/tests/fixtures/gen/gen_golden.R
#
# Requires the r45 conda env + jsonlite.

refdir <- normalizePath(file.path(getwd(), "reference", "LCV", "R"))
if (!file.exists(file.path(refdir, "RunLCV.R"))) {
  refdir <- normalizePath(file.path(dirname(getwd()), "reference", "LCV", "R"))
}
cat("Reference R dir:", refdir, "\n")

source(file.path(refdir, "MomentFunctions.R"))
source(file.path(refdir, "SimulateLCV.R"))
# RunLCV.R calls source("MomentFunctions.R") internally with a relative path,
# so wd must be refdir when RunLCV() is called.
source(file.path(refdir, "RunLCV.R"))

outdir <- file.path(getwd(), "bio_crates", "lcv", "tests", "fixtures")
dir.create(outdir, showWarnings = FALSE, recursive = TRUE)

dump_result <- function(lcv, path) {
  res <- list(
    zscore               = as.numeric(lcv$zscore),
    pval_gcpzero_2tailed = as.numeric(lcv$pval.gcpzero.2tailed),
    gcp_pm               = as.numeric(lcv$gcp.pm),
    gcp_pse              = as.numeric(lcv$gcp.pse),
    rho_est              = as.numeric(lcv$rho.est),
    rho_err              = as.numeric(lcv$rho.err),
    pval_fullycausal_1   = as.numeric(lcv$pval.fullycausal[1]),
    pval_fullycausal_2   = as.numeric(lcv$pval.fullycausal[2]),
    h2_zscore_1          = as.numeric(lcv$h2.zscore[1]),
    h2_zscore_2          = as.numeric(lcv$h2.zscore[2])
  )
  cat(jsonlite::toJSON(res, auto_unbox = TRUE, pretty = TRUE, digits = 17), file = path)
}

dump_input <- function(ell, z1, z2, weights, path) {
  df <- data.frame(ell = ell, z1 = z1, z2 = z2, w = weights)
  write.table(df, file = path, sep = "\t", row.names = FALSE, quote = FALSE)
}

run_scenario <- function(tag, seed, q1, q2) {
  M <- 50000; N.1 <- 20000; N.2 <- 50000
  h2.1 <- 0.3; h2.2 <- 0.3
  p.pi <- 0.05; p.g1 <- 0.05; p.g2 <- 0.2
  ell <- rep(1, M)
  weights <- rep(1, M)

  set.seed(seed)
  alpha <- SimulateLCV(M, N.1, N.2, h2.1, h2.2, q1, q2, p.pi, p.g1, p.g2)

  oldwd <- getwd(); setwd(refdir)
  lcv <- RunLCV(ell, alpha$a1, alpha$a2,
                no.blocks = 100,
                crosstrait.intercept = 0,
                ldsc.intercept = 0,
                weights = weights,
                n.1 = N.1, n.2 = N.2,
                intercept.12 = 0)
  setwd(oldwd)

  gcp_true <- log(q2/q1)/log(q2*q1)
  cat(sprintf("Scenario %s: gcp_true=%.3f  gcp_est=%.4f(%.4f)  rho=%.4f  z=%.2f\n",
              tag, gcp_true, lcv$gcp.pm, lcv$gcp.pse, lcv$rho.est, lcv$zscore))

  dump_input(ell, alpha$a1, alpha$a2, weights,
             file.path(outdir, paste0("scenario", tag, ".tsv")))
  dump_result(lcv, file.path(outdir, paste0("scenario", tag, "_result.json")))
}

# Scenario A: strong partial causality, trait1→trait2 (gcp>0)
run_scenario("A", seed = 42, q1 = 1.0, q2 = 0.2)

# Scenario B: reversed direction, trait2→trait1 (gcp<0)
run_scenario("B", seed = 777, q1 = 0.2, q2 = 1.0)

cat("Done. Fixtures written to ", outdir, "\n")
