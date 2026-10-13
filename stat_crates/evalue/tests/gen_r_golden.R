#!/usr/bin/env Rscript
## Cross-validation: generate E-value reference outputs from R EValue package.
## Usage: Rscript gen_r_golden.R output.json
##
## Outputs JSON with E-values computed by the real R EValue package for
## every measure type, bias scenario, and example from the vignettes/tests.

library(EValue)
library(jsonlite)

results <- list()

# ── Simple confounding E-values ────────────────────────────────────────

# RR: leukemia example (VanderWeele & Ding 2017)
r <- evalues.RR(est = 0.80, lo = 0.71, hi = 0.91)
results$rr_leukemia <- list(
  point_rr = r["RR", "point"],
  lower_rr = r["RR", "lower"],
  upper_rr = r["RR", "upper"],
  point_e = r["E-values", "point"],
  lower_e = r["E-values", "lower"],
  upper_e = r["E-values", "upper"]
)

# RR: only point estimate
r <- evalues.RR(est = 3.5)
results$rr_point_only <- list(
  point_rr = r["RR", "point"],
  point_e  = r["E-values", "point"]
)

# RR: non-null true
r <- evalues.RR(est = 2.0, true = 1.5)
results$rr_non_null <- list(
  point_e = r["E-values", "point"]
)

# OR: common outcome (rare=FALSE)
r <- evalues.OR(est = 0.86, lo = 0.75, hi = 0.99, rare = FALSE)
results$or_common <- list(
  point_rr = r["RR", "point"],
  point_e  = r["E-values", "point"]
)

# OR: rare outcome
r <- evalues.OR(est = 3.0, rare = TRUE)
results$or_rare <- list(
  point_rr = r["RR", "point"],
  point_e  = r["E-values", "point"]
)

# HR: common outcome
r <- evalues.HR(est = 0.56, lo = 0.46, hi = 0.69, rare = FALSE)
results$hr_common <- list(
  point_rr = r["RR", "point"],
  point_e  = r["E-values", "point"]
)

# HR: rare outcome
r <- evalues.HR(est = 0.56, lo = 0.46, hi = 0.69, rare = TRUE)
results$hr_rare <- list(
  point_rr = r["RR", "point"],
  point_e  = r["E-values", "point"]
)

# MD: Cohen's d = 0.5, SE = 0.25
r <- evalues.MD(est = 0.5, se = 0.25)
results$md_basic <- list(
  point_rr = r["RR", "point"],
  point_e  = r["E-values", "point"]
)

# OLS: est=0.3, se=0.1, sd=1.0
r <- evalues.OLS(est = 0.3, se = 0.1, sd = 1.0)
results$ols_basic <- list(
  point_rr = r["RR", "point"],
  point_e  = r["E-values", "point"]
)

# Smoking example via twoXtwoRR
rr <- twoXtwoRR(397, 78557, 51, 108778)
r <- evalues.RR(est = rr["point"], lo = rr["lower"], hi = rr["upper"])
results$smoking <- list(
  point_rr = r["RR", "point"],
  point_e  = r["E-values", "point"]
)

# RD: smoking data
r <- evalues.RD(397, 78557, 51, 108778, true = 0)
results$rd_smoking <- list(
  est_evalue   = r$est.Evalue,
  lower_evalue = r$lower.Evalue
)

# ── Multi-bias E-values ────────────────────────────────────────────────

# Confounding only (should match simple E-value)
biases <- multi_bias(confounding())
r <- multi_evalue(biases, est = RR(3.5))
results$multi_confounding <- list(
  point_e = summary(r)
)

# Selection general
biases <- multi_bias(selection("general"))
r <- multi_evalue(biases, est = RR(0.5))
results$multi_sel_general <- list(
  point_e = summary(r)
)

# Selection selected
biases <- multi_bias(selection("selected"))
r <- multi_evalue(biases, est = RR(4.7))
results$multi_sel_selected <- list(
  point_e = summary(r)
)

# Selection increased risk
biases <- multi_bias(selection("general", "increased risk"))
r <- multi_evalue(biases, est = RR(0.8))
results$multi_sel_incr <- list(
  point_e = summary(r)
)

# Selection S=U
biases <- multi_bias(selection("general", "S = U"))
r <- multi_evalue(biases, est = RR(5.3))
results$multi_sel_su <- list(
  point_e = summary(r)
)

# Confounding + selection general
biases <- multi_bias(confounding(), selection("general"))
r <- multi_evalue(biases, est = RR(2.5))
results$multi_conf_sel <- list(
  point_e = summary(r)
)

# Confounding + selection + outcome misclass
biases <- multi_bias(confounding(), selection("general"), misclassification("outcome"))
r <- multi_evalue(biases, est = RR(2.5))
results$multi_conf_sel_misclass <- list(
  point_e = summary(r)
)

# ── Multi-bias bound ───────────────────────────────────────────────────

biases <- multi_bias(confounding())
b <- multi_bound(biases, RRAUc = 2, RRUcY = 2)
results$bound_confounding <- b

biases <- multi_bias(confounding(), selection("general", "increased risk"))
b <- multi_bound(biases, RRAUc = 2, RRUcY = 2, RRUsYA1 = 3, RRSUsA1 = 3)
results$bound_conf_sel_incr <- b

# ── Selection bias E-values (svalues) ──────────────────────────────────

# Zika virus
r <- svalues.RR(est = 73.1, lo = 13.0)
results$svalues_zika <- list(
  point_e = summary(r)
)

# Obesity paradox (sel_pop)
r <- svalues.RR(est = 1.50, lo = 1.22, sel_pop = TRUE)
results$svalues_obesity <- list(
  point_e = summary(r)
)

# Endometrial cancer
r <- svalues.RR(est = 2.30, true = 11.98, S_eq_U = TRUE, risk_inc = TRUE)
results$svalues_endometrial <- list(
  point_e = summary(r)
)

# ── Write output ────────────────────────────────────────────────────────

args <- commandArgs(trailingOnly = TRUE)
outfile <- if (length(args) >= 1) args[1] else "evalue_golden.json"
cat(toJSON(results, auto_unbox = TRUE, digits = 12, pretty = TRUE), file = outfile)
cat("Written", length(results), "reference results to", outfile, "\n")
