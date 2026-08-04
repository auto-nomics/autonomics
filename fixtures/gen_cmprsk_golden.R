#!/usr/bin/env Rscript
# =============================================================================
# Golden-test data generator for the Rust `cmprsk` port (stat_crates/cmprsk).
#
# For each scenario this dumps BOTH the exact input data AND the full R output
# object, so the Rust cross-validation test replays identical inputs and can
# compare every component (coef, loglik, score, inf, var, res, uftime, bfitj,
# tfs, summary tables, predicted curves; for cuminc: every curve and Gray's
# test).
#
# Because the inputs are dumped alongside the outputs, the fixtures do not
# depend on R's RNG staying stable across versions — re-running this script on
# any R with cmprsk installed regenerates a self-consistent set.
#
# Run (r45 conda env has R 4.5.3 + cmprsk + survival):
#   export R_HOME=$HOME/miniconda3/envs/r45/lib/R
#   export PATH=$HOME/miniconda3/envs/r45/bin:$PATH
#   export LD_LIBRARY_PATH=$HOME/miniconda3/envs/r45/lib:$LD_LIBRARY_PATH
#   Rscript fixtures/gen_cmprsk_golden.R
#
# Output: fixtures/golden/cmprsk_*.json
# =============================================================================

suppressMessages({
  library(cmprsk)
  library(jsonlite)
})

out_dir <- "fixtures/golden"
dir.create(out_dir, showWarnings = FALSE, recursive = TRUE)

# jsonlite defaults to 4 significant digits — fatal for a numerical fixture.
# digits = NA writes full double precision; auto_unbox = FALSE keeps
# length-1 vectors as arrays so the Rust side sees a stable shape.
write_fixture <- function(name, payload) {
  path <- file.path(out_dir, paste0("cmprsk_", name, ".json"))
  jsonlite::write_json(payload, path,
    digits = NA, auto_unbox = FALSE, na = "null", null = "null", pretty = FALSE
  )
  cat(sprintf("wrote %-34s\n", basename(path)))
}

# Matrices must round-trip as row-major nested arrays even when they are 1×1
# or have a single row, hence the explicit lapply rather than relying on
# jsonlite's matrix handling.
mat_rows <- function(m) {
  if (is.null(m)) {
    return(NULL)
  }
  m <- as.matrix(m)
  lapply(seq_len(nrow(m)), function(i) as.numeric(m[i, ]))
}

na_mat_to_null <- function(m) {
  if (is.null(m)) {
    return(NULL)
  }
  if (all(is.na(m))) {
    return(NULL)
  }
  mat_rows(m)
}

# ── crr scenario driver ─────────────────────────────────────────────────────
# tf_names is the vocabulary the Rust TimeFn enum understands; tf_fun is the
# matching R closure. Keeping them side by side is what lets the Rust test
# assert the two implementations of tf agree.
gen_crr <- function(name, ftime, fstatus,
                    cov1 = NULL, cov2 = NULL, tf_names = NULL, tf_fun = NULL,
                    cengroup = NULL, failcode = 1, cencode = 0,
                    gtol = 1e-6, maxiter = 10, init = NULL, variance = TRUE,
                    pred_cov1 = NULL, pred_cov2 = NULL) {
  args <- list(ftime = ftime, fstatus = fstatus)
  if (!is.null(cov1)) args$cov1 <- cov1
  if (!is.null(cov2)) {
    args$cov2 <- cov2
    args$tf <- tf_fun
  }
  if (!is.null(cengroup)) args$cengroup <- cengroup
  args$failcode <- failcode
  args$cencode <- cencode
  args$gtol <- gtol
  args$maxiter <- maxiter
  if (!is.null(init)) args$init <- init
  args$variance <- variance

  z <- do.call(cmprsk::crr, args)

  # Full-precision Wald quantities. summary.crr rounds its p-value column via
  # signif(), so recompute rather than parse it.
  smry <- NULL
  if (variance) {
    beta <- z$coef
    se <- sqrt(diag(as.matrix(z$var)))
    zs <- beta / se
    pv <- 2 * (1 - pnorm(abs(zs)))
    qa <- qnorm(c(0.025, 0.975))
    s_obj <- summary(z, conf.int = 0.95)
    smry <- list(
      se = as.numeric(se),
      z = as.numeric(zs),
      p_value = as.numeric(pv),
      exp_coef = as.numeric(exp(beta)),
      exp_neg_coef = as.numeric(exp(-beta)),
      ci_lower = as.numeric(exp(beta + qa[1] * se)),
      ci_upper = as.numeric(exp(beta + qa[2] * se)),
      logtest = as.numeric(s_obj$logtest["test"]),
      df = as.numeric(s_obj$logtest["df"]),
      # summary()'s own confidence-interval matrix, as a shape check
      conf_int = mat_rows(s_obj$conf.int)
    )
  }

  pred <- NULL
  if (!is.null(pred_cov1) || !is.null(pred_cov2)) {
    p <- if (is.null(pred_cov2)) {
      predict(z, pred_cov1)
    } else if (is.null(pred_cov1)) {
      predict(z, cov1 = NULL, cov2 = pred_cov2)
    } else {
      predict(z, pred_cov1, pred_cov2)
    }
    p <- as.matrix(p)
    pred <- list(
      cov1 = mat_rows(pred_cov1),
      cov2 = mat_rows(pred_cov2),
      # column 1 is uftime; the remainder are one curve per covariate row
      curves = lapply(2:ncol(p), function(j) as.numeric(p[, j]))
    )
  }

  payload <- list(
    name = name,
    kind = "crr",
    input = list(
      ftime = as.numeric(ftime),
      fstatus = as.numeric(fstatus),
      cov1 = mat_rows(cov1),
      cov1_names = if (is.null(cov1)) NULL else colnames(as.matrix(cov1)),
      cov2 = mat_rows(cov2),
      cov2_names = if (is.null(cov2)) NULL else colnames(as.matrix(cov2)),
      tf = tf_names,
      cengroup = if (is.null(cengroup)) NULL else as.numeric(cengroup),
      failcode = failcode,
      cencode = cencode,
      gtol = gtol,
      maxiter = maxiter,
      init = init,
      variance = variance
    ),
    fit = list(
      coef = as.numeric(z$coef),
      terms = names(z$coef),
      loglik = z$loglik,
      loglik_null = z$loglik.null,
      score = as.numeric(z$score),
      inf = na_mat_to_null(z$inf),
      var = na_mat_to_null(z$var),
      invinf = na_mat_to_null(z$invinf),
      res = if (is.null(z$res)) NULL else mat_rows(z$res),
      uftime = as.numeric(z$uftime),
      bfitj = as.numeric(z$bfitj),
      tfs = if (length(z$tfs) <= 1) NULL else mat_rows(z$tfs),
      converged = z$converged,
      n = z$n,
      n_missing = z$n.missing
    ),
    summary = smry,
    predict = pred
  )
  write_fixture(name, payload)
  invisible(z)
}

# ── cuminc scenario driver ──────────────────────────────────────────────────
gen_cuminc <- function(name, ftime, fstatus, group = NULL, strata = NULL,
                       rho = 0, cencode = 0) {
  args <- list(ftime = ftime, fstatus = fstatus)
  if (!is.null(group)) args$group <- group
  if (!is.null(strata)) args$strata <- strata
  args$rho <- rho
  args$cencode <- cencode
  x <- do.call(cmprsk::cuminc, args)

  tests <- NULL
  if (!is.null(x$Tests)) {
    tt <- x$Tests
    tests <- lapply(seq_len(nrow(tt)), function(i) {
      list(
        cause = rownames(tt)[i],
        stat = as.numeric(tt[i, "stat"]),
        p_value = as.numeric(tt[i, "pv"]),
        df = as.numeric(tt[i, "df"])
      )
    })
    x <- x[names(x) != "Tests"]
  }

  curves <- lapply(seq_along(x), function(i) {
    list(
      name = names(x)[i],
      time = as.numeric(x[[i]]$time),
      est = as.numeric(x[[i]]$est),
      var = as.numeric(x[[i]]$var)
    )
  })

  payload <- list(
    name = name,
    kind = "cuminc",
    input = list(
      ftime = as.numeric(ftime),
      fstatus = as.numeric(fstatus),
      group = if (is.null(group)) NULL else as.numeric(group),
      strata = if (is.null(strata)) NULL else as.numeric(strata),
      rho = rho,
      cencode = cencode
    ),
    curves = curves,
    tests = tests
  )
  write_fixture(name, payload)
  invisible(x)
}

named <- function(m, prefix) {
  m <- as.matrix(m)
  colnames(m) <- paste0(prefix, seq_len(ncol(m)))
  m
}

# =============================================================================
# Scenarios
# =============================================================================

# ── 1. the crr.Rd example: n = 200, three covariates ────────────────────────
set.seed(10)
ftime <- rexp(200)
fstatus <- sample(0:2, 200, replace = TRUE)
cov <- matrix(runif(600), nrow = 200)
dimnames(cov)[[2]] <- c("x1", "x2", "x3")

gen_crr("crr_basic", ftime, fstatus, cov1 = cov,
  pred_cov1 = rbind(c(.1, .5, .8), c(.1, .5, .2))
)

# ── 2/3. status recoding ────────────────────────────────────────────────────
gen_crr("crr_failcode2", ftime, fstatus, cov1 = cov, failcode = 2)
gen_crr("crr_cencode2", ftime, fstatus, cov1 = cov, failcode = 1, cencode = 2)

# ── 4. stratified censoring distribution ────────────────────────────────────
set.seed(11)
cg <- sample(1:3, 200, replace = TRUE)
gen_crr("crr_cengroup", ftime, fstatus, cov1 = cov, cengroup = cg)

# ── 5. cov2 + tf: quadratic in time (the documented example) ────────────────
cov2q <- named(cbind(cov[, 1], cov[, 1]), "q")
gen_crr("crr_cov2_tf_quadratic", ftime, fstatus,
  cov1 = cov,
  cov2 = cov2q,
  tf_names = c("identity", "square"),
  tf_fun = function(uft) cbind(uft, uft^2),
  pred_cov1 = rbind(c(.1, .5, .8), c(.9, .2, .3)),
  pred_cov2 = rbind(c(.1, .1), c(.9, .9))
)

# ── 6. cov2 only (the nc1 == 0 branch) ──────────────────────────────────────
cov2only <- named(matrix(cov[, 1], ncol = 1), "z")
gen_crr("crr_cov2_only", ftime, fstatus,
  cov2 = cov2only,
  tf_names = c("identity"),
  tf_fun = function(x) x
)

# ── 7. cov1 + cov2 + cengroup, with a log time function ─────────────────────
gen_crr("crr_cov1_cov2_cengroup", ftime, fstatus,
  cov1 = cov,
  cov2 = named(matrix(cov[, 2], ncol = 1), "w"),
  tf_names = c("log"),
  tf_fun = function(uft) cbind(log(uft)),
  cengroup = cg
)

# ── 8. heavy ties (integer times) ───────────────────────────────────────────
set.seed(12)
tie_t <- as.numeric(sample(1:8, 150, replace = TRUE))
tie_s <- sample(0:2, 150, replace = TRUE)
tie_x <- named(matrix(rnorm(300), nrow = 150), "t")
gen_crr("crr_ties_heavy", tie_t, tie_s, cov1 = tie_x)

# ── 9. events at time 0 (the 2019 min(0, u$time) fix) ───────────────────────
set.seed(13)
z0_t <- c(rep(0, 12), as.numeric(sample(1:6, 108, replace = TRUE)))
z0_s <- sample(0:2, 120, replace = TRUE)
z0_x <- named(matrix(rnorm(240), nrow = 120), "e")
gen_crr("crr_events_at_zero", z0_t, z0_s, cov1 = z0_x)

# ── 10. missing values → na.omit + n.missing ────────────────────────────────
cov_na <- cov
cov_na[c(3, 17, 88), 2] <- NA
ft_na <- ftime
ft_na[c(5, 6)] <- NA
gen_crr("crr_missing", ft_na, fstatus, cov1 = cov_na)

# ── 11. maxiter = 0 at a non-zero init (scores/variance only) ───────────────
gen_crr("crr_maxiter0_init", ftime, fstatus, cov1 = cov,
  maxiter = 0, init = c(0.15, -0.20, 0.05)
)

# ── 12. variance = FALSE ────────────────────────────────────────────────────
gen_crr("crr_variance_false", ftime, fstatus, cov1 = cov, variance = FALSE)

# ── 13. a single covariate (np == 1) ────────────────────────────────────────
gen_crr("crr_single_cov", ftime, fstatus,
  cov1 = named(matrix(cov[, 1], ncol = 1), "s")
)

# ── 14/15. cuminc ───────────────────────────────────────────────────────────
set.seed(2)
cu_t <- rexp(150)
cu_s <- sample(0:2, 150, replace = TRUE)
cu_g <- sample(1:3, 150, replace = TRUE)
cu_st <- sample(1:2, 150, replace = TRUE)

gen_cuminc("cuminc_basic", cu_t, cu_s)
gen_cuminc("cuminc_group_strata_rho", cu_t, cu_s,
  group = cu_g, strata = cu_st, rho = 1
)
gen_cuminc("cuminc_group_only", cu_t, cu_s, group = cu_g)
gen_cuminc("cuminc_ties", as.numeric(sample(1:6, 150, replace = TRUE)), cu_s,
  group = cu_g, strata = cu_st
)

cat("\nAll fixtures written to ", out_dir, "\n", sep = "")
