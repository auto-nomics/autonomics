#!/usr/bin/env Rscript
# Generate golden MVMR output from the reference R package (MVMR v0.4.8)
# for cross-validation against the Rust port.
#
# Usage (r45 conda env: R 4.5.x + MVMR):
#   export R_HOME=$HOME/miniconda3/envs/r45/lib/R
#   export PATH=$HOME/miniconda3/envs/r45/bin:$PATH
#   export LD_LIBRARY_PATH=$HOME/miniconda3/envs/r45/lib:$LD_LIBRARY_PATH
#   Rscript tests/gen_r_golden.R <out.json>
#
# Writes a JSON object keyed by scenario name. Each scenario captures
# inputs + all outputs needed for cross-validation: IVW coefficients (est,
# se, t, p), conditional F-statistics, strhet F-statistics, Q validity
# statistic + p-value, legacy Q strength / Q validity, and qhet effect
# estimates.

suppressMessages(library(MVMR))
suppressMessages(library(jsonlite))

args <- commandArgs(trailingOnly = TRUE)
out_path <- if (length(args) >= 1) args[1] else "tests/mvmr_golden.json"

data(rawdat_mvmr)

run_scenario <- function(name, bx_cols, bxse_cols, by_col = "SBP_beta",
                         byse_col = "SBP_se", gencov = 0,
                         pcor = NULL) {
  r_input <- format_mvmr(
    BXGs = rawdat_mvmr[, bx_cols, drop = FALSE],
    BYG = rawdat_mvmr[, by_col],
    seBXGs = rawdat_mvmr[, bxse_cols, drop = FALSE],
    seBYG = rawdat_mvmr[, byse_col],
    RSID = rawdat_mvmr$SNP
  )

  # ── IVW ──
  ivw_res <- ivw_mvmr(r_input)
  ivw <- lapply(seq_len(nrow(ivw_res)), function(i) {
    list(estimate = ivw_res[i, 1], se = ivw_res[i, 2],
         t_stat = ivw_res[i, 3], pvalue = ivw_res[i, 4])
  })

  # ── Legacy mvmr() — IVW + Q strength + Q validity ──
  mvmr_res <- mvmr(r_input, gencov, 1)
  q_strength <- as.list(mvmr_res$Q_strength[1, ])
  q_valid <- mvmr_res$Q_valid
  p_valid <- mvmr_res$p_valid
  sigma <- summary(stats::lm(
    stats::as.formula(paste("betaYG ~ -1 +",
       paste(names(r_input)[4:(3 + length(bx_cols))], collapse = "+"))),
    weights = 1 / r_input[, 3]^2, data = r_input))$sigma

  # ── strength_mvmr — conditional F-statistics ──
  str_res <- strength_mvmr(r_input, gencov)
  fstat <- as.list(str_res[1, ])

  # ── strhet_mvmr — IRLS conditional F-statistics ──
  strhet_res <- strhet_mvmr(r_input, gencov)
  fstat_het <- as.list(strhet_res[1, ])

  # ── pleiotropy_mvmr — Q validity + p ──
  pleio_res <- pleiotropy_mvmr(r_input, gencov)
  qstat_pleio <- pleio_res$Qstat
  qpval_pleio <- pleio_res$Qpval

  out <- list(
    n = nrow(r_input),
    p = length(bx_cols),
    ivw = ivw,
    sigma = sigma,
    q_strength = q_strength,
    q_valid = q_valid,
    p_valid = p_valid,
    fstat = fstat,
    fstat_het = fstat_het,
    qstat_pleio = qstat_pleio,
    qpval_pleio = qpval_pleio
  )

  # ── qhet_mvmr (only when pcor supplied) ──
  if (!is.null(pcor)) {
    qhet_res <- qhet_mvmr(r_input, pcor, CI = FALSE)
    out$qhet <- lapply(seq_len(nrow(qhet_res)), function(i) {
      list(estimate = qhet_res[i, 1])
    })
  }

  out
}

# ── Scenarios ───────────────────────────────────────────────────────────────

scenarios <- list()

# 2-exposure: LDL + HDL → SBP
scenarios$ldl_hdl <- run_scenario(
  "ldl_hdl",
  bx_cols = c("LDL_beta", "HDL_beta"),
  bxse_cols = c("LDL_se", "HDL_se"),
  gencov = 0
)

# 2-exposure with non-zero gencov (legacy path)
scenarios$ldl_hdl_gencov <- run_scenario(
  "ldl_hdl_gencov",
  bx_cols = c("LDL_beta", "HDL_beta"),
  bxse_cols = c("LDL_se", "HDL_se"),
  gencov = 0.00001
)

# 3-exposure: LDL + HDL + Trg → SBP
scenarios$ldl_hdl_trg <- run_scenario(
  "ldl_hdl_trg",
  bx_cols = c("LDL_beta", "HDL_beta", "Trg_beta"),
  bxse_cols = c("LDL_se", "HDL_se", "Trg_se"),
  gencov = 0
)

# 3-exposure with phenotypic correlation matrix for qhet
pcor3 <- matrix(c(1.0, 0.3, 0.2,
                  0.3, 1.0, 0.4,
                  0.2, 0.4, 1.0), nrow = 3, ncol = 3, byrow = TRUE)
scenarios$ldl_hdl_trg_qhet <- run_scenario(
  "ldl_hdl_trg_qhet",
  bx_cols = c("LDL_beta", "HDL_beta", "Trg_beta"),
  bxse_cols = c("LDL_se", "HDL_se", "Trg_se"),
  gencov = 0,
  pcor = pcor3
)

# 2-exposure qhet with 2×2 pcor
pcor2 <- matrix(c(1.0, 0.3, 0.3, 1.0), nrow = 2, ncol = 2)
scenarios$ldl_hdl_qhet <- run_scenario(
  "ldl_hdl_qhet",
  bx_cols = c("LDL_beta", "HDL_beta"),
  bxse_cols = c("LDL_se", "HDL_se"),
  gencov = 0,
  pcor = pcor2
)

write_json(scenarios, out_path, auto_unbox = TRUE, digits = NA, pretty = TRUE)
cat("Wrote", out_path, "\n")
cat("Scenarios:", paste(names(scenarios), collapse = ", "), "\n")
