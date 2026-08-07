#!/usr/bin/env Rscript
# Cross-validation harness for autonomics DAG codegen.
#
# For each node kind, this script:
# 1. Generates a synthetic dataset
# 2. Runs the analysis in R (the "reference" implementation)
# 3. Saves the result to CSV
#
# The Rust test (tests/cross_validate.rs) runs the same DAG through the Rust
# engine, compiles it to R, runs the generated R script, and compares the
# reference output against the Rust output.
#
# Usage: Rscript cross_validate.R <test_name> <output_dir>
# Writes: <output_dir>/<test_name>_reference.csv

args <- commandArgs(trailingOnly = TRUE)
test_name <- args[1]
out_dir <- args[2]

dir.create(out_dir, showWarnings = FALSE, recursive = TRUE)
suppressPackageStartupMessages({
  library(mice)
})
set.seed(42)

# ── linear_regression ──────────────────────────────────────────────────────
if (test_name == "linear_regression") {
    n <- 100
    x1 <- rnorm(n, mean = 5, sd = 2)
    x2 <- rnorm(n, mean = 10, sd = 3)
    y <- 3 + 2 * x1 - 1.5 * x2 + rnorm(n, sd = 1)
    df <- data.frame(x1 = x1, x2 = x2, y = y)
    data_path <- file.path(out_dir, "linear_regression_data.csv")
    data.table::fwrite(df, data_path)

    # Reference
    fit <- lm(y ~ x1 + x2, data = df)
    smry <- summary(fit)
    coefs <- as.data.frame(smry$coefficients)
    names(coefs) <- c("coefficient", "std_error", "t_stat", "p_value")
    coefs$term <- rownames(coefs)
    ref <- coefs[, c("term", "coefficient", "std_error", "t_stat", "p_value")]
    ref$r_squared <- smry$r.squared
    ref$n_obs <- n
    data.table::fwrite(ref, file.path(out_dir, "linear_regression_reference.csv"))
    cat("OK linear_regression\n")
}

# ── logistic_regression ────────────────────────────────────────────────────
if (test_name == "logistic_regression") {
    n <- 200
    x1 <- rnorm(n)
    x2 <- rnorm(n)
    linpred <- 0.5 + 0.8 * x1 - 0.6 * x2
    prob <- 1 / (1 + exp(-linpred))
    y <- rbinom(n, 1, prob)
    df <- data.frame(x1 = x1, x2 = x2, y = y)
    data_path <- file.path(out_dir, "logistic_regression_data.csv")
    data.table::fwrite(df, data_path)

    # Reference
    fit <- glm(y ~ x1 + x2, data = df, family = binomial)
    smry <- summary(fit)
    coefs <- as.data.frame(smry$coefficients)
    names(coefs) <- c("coefficient", "std_error", "z_stat", "p_value")
    coefs$term <- rownames(coefs)
    ref <- coefs[, c("term", "coefficient", "std_error", "z_stat", "p_value")]
    data.table::fwrite(ref, file.path(out_dir, "logistic_regression_reference.csv"))
    cat("OK logistic_regression\n")
}

# ── chi_square ─────────────────────────────────────────────────────────────
if (test_name == "chi_square") {
    n <- 500
    group <- sample(c("A", "B", "C"), n, replace = TRUE)
    outcome <- sample(c("yes", "no"), n, replace = TRUE, prob = c(0.4, 0.6))
    df <- data.frame(group = group, outcome = outcome)
    data_path <- file.path(out_dir, "chi_square_data.csv")
    data.table::fwrite(df, data_path)

    # Reference
    test_result <- chisq.test(table(df$group, df$outcome))
    ref <- data.frame(
        chi_squared = as.numeric(test_result$statistic),
        df = as.integer(test_result$parameter),
        p_value = test_result$p.value,
        n = sum(test_result$observed),
        small_expected = sum(test_result$expected < 5)
    )
    data.table::fwrite(ref, file.path(out_dir, "chi_square_reference.csv"))
    cat("OK chi_square\n")
}

# ── cox_regression ─────────────────────────────────────────────────────────
if (test_name == "cox_regression") {
    n <- 200
    x1 <- rnorm(n)
    x2 <- rbinom(n, 1, 0.4)
    # Simulate survival times via exponential hazard
    hazard <- 0.1 * exp(0.5 * x1 + 0.8 * x2)
    time <- rexp(n, rate = hazard)
    event <- ifelse(time < 10, 1, 0)
    time <- pmin(time, 10)
    df <- data.frame(x1 = x1, x2 = x2, time = time, event = event)
    data_path <- file.path(out_dir, "cox_regression_data.csv")
    data.table::fwrite(df, data_path)

    # Reference
    fit <- survival::coxph(Surv(time, event) ~ x1 + x2, data = df)
    smry <- summary(fit)
    ref <- data.frame(
        term = rownames(smry$coefficients),
        coefficient = smry$coefficients[, "coef"],
        std_error = smry$coefficients[, "se(coef)"],
        z_stat = smry$coefficients[, "z"],
        p_value = smry$coefficients[, "Pr(>|z|)"]
    )
    data.table::fwrite(ref, file.path(out_dir, "cox_regression_reference.csv"))
    cat("OK cox_regression\n")
}

# ── epi_roc ────────────────────────────────────────────────────────────────
if (test_name == "epi_roc") {
    n <- 200
    label <- rbinom(n, 1, 0.3)
    score <- ifelse(label == 1, rnorm(n, 1, 1), rnorm(n, 0, 1))
    df <- data.frame(score = score, label = label)
    data_path <- file.path(out_dir, "epi_roc_data.csv")
    data.table::fwrite(df, data_path)

    # Reference
    roc_obj <- pROC::roc(df$label, df$score, quiet = TRUE)
    ref <- data.frame(
        auc = as.numeric(pROC::auc(roc_obj)),
        ci_lower = as.numeric(pROC::ci.auc(roc_obj))[1],
        ci_upper = as.numeric(pROC::ci.auc(roc_obj))[3]
    )
    data.table::fwrite(ref, file.path(out_dir, "epi_roc_reference.csv"))
    cat("OK epi_roc\n")
}

# ── survival (Kaplan-Meier) ────────────────────────────────────────────────
if (test_name == "survival") {
    n <- 200
    time <- rexp(n, rate = 0.1 * exp(rnorm(n, 0, 0.5)))
    event <- ifelse(time < 10, 1, 0)
    time <- pmin(time, 10)
    df <- data.frame(time = time, event = event)
    data_path <- file.path(out_dir, "survival_data.csv")
    data.table::fwrite(df, data_path)

    fit <- survival::survfit(Surv(time, event) ~ 1, data = df)
    smry <- summary(fit)
    ref <- data.frame(
        time = smry$time,
        survival = smry$surv,
        std_error = smry$std.err,
        n_at_risk = smry$n.risk,
        n_events = smry$n.event
    )
    data.table::fwrite(ref, file.path(out_dir, "survival_reference.csv"))
    cat("OK survival\n")
}

# ── epi_lasso ──────────────────────────────────────────────────────────────
if (test_name == "epi_lasso") {
    n <- 150
    p <- 5
    X <- matrix(rnorm(n * p), n, p)
    colnames(X) <- paste0("x", 1:p)
    beta_true <- c(1.5, 0, -0.8, 0, 0.6)
    y <- as.numeric(X %*% beta_true + rnorm(n, sd = 0.5))
    df <- as.data.frame(cbind(X, y = y))
    data_path <- file.path(out_dir, "epi_lasso_data.csv")
    data.table::fwrite(df, data_path)

    set.seed(42)
    x_mat <- as.matrix(df[, paste0("x", 1:p)])
    y_vec <- df$y
    cv_fit <- glmnet::cv.glmnet(x_mat, y_vec, alpha = 1, nfolds = 5, nlambda = 50)
    ref <- data.frame(
        feature = paste0("x", 1:p),
        coef_min = as.numeric(coef(cv_fit, s = "lambda.min")[-1]),
        coef_1se = as.numeric(coef(cv_fit, s = "lambda.1se")[-1]),
        lambda_min = cv_fit$lambda.min,
        lambda_1se = cv_fit$lambda.1se
    )
    data.table::fwrite(ref, file.path(out_dir, "epi_lasso_reference.csv"))
    cat("OK epi_lasso\n")
}

# ── liability ──────────────────────────────────────────────────────────────
if (test_name == "liability") {
    # Liability takes an LDSC h² summary as input. We create a synthetic one.
    h2 <- 0.25
    h2_se <- 0.02
    samp_prev <- 0.5
    pop_prev <- 0.01
    df <- data.frame(
        h2 = h2, h2_se = h2_se,
        intercept = 1.0, intercept_se = 0.01,
        ratio = 0.1, ratio_se = 0.05,
        mean_chisq = 1.1, lambda_gc = 1.05, n_snp = 500000,
        coef = "[0.1]", coef_se = "[0.01]"
    )
    data_path <- file.path(out_dir, "liability_data.csv")
    data.table::fwrite(df, data_path)

    # Reference: liability-threshold conversion
    K <- pop_prev
    P <- samp_prev
    z <- qnorm(1 - K)
    factor <- K^2 * (1 - K)^2 / (P * (1 - P) * dnorm(z)^2)
    ref <- data.frame(
        h2 = h2,
        h2_se = h2_se,
        h2_liab = h2 * factor,
        h2_se_liab = h2_se * factor,
        factor = factor
    )
    data.table::fwrite(ref, file.path(out_dir, "liability_reference.csv"))
    cat("OK liability\n")
}

# ── causal (PSM) ───────────────────────────────────────────────────────────
if (test_name == "causal_psm") {
    n <- 300
    age <- rnorm(n, 50, 10)
    female <- rbinom(n, 1, 0.5)
    # Treatment propensity depends on covariates
    ps <- plogis(-1 + 0.05 * age + 0.5 * female)
    treat <- rbinom(n, 1, ps)
    # Outcome depends on treatment + covariates (true ATT = 2.0)
    y <- 10 + 2 * treat + 0.1 * age + 3 * female + rnorm(n, sd = 2)
    df <- data.frame(treat = treat, y = y, age = age, female = female)
    data_path <- file.path(out_dir, "causal_psm_data.csv")
    data.table::fwrite(df, data_path)

    # Reference: MatchIt PSM
    m <- MatchIt::matchit(treat ~ age + female, data = df, method = "nearest")
    md <- MatchIt::match.data(m)
    fit <- lm(y ~ treat, data = md)
    smry <- summary(fit)
    ref <- data.frame(
        att = coef(fit)["treat"],
        att_se = smry$coefficients["treat", "Std. Error"],
        n_treated = sum(md$treat == 1),
        n_obs = nrow(md)
    )
    data.table::fwrite(ref, file.path(out_dir, "causal_psm_reference.csv"))
    cat("OK causal_psm\n")
}

# ── causal (IPTW) ──────────────────────────────────────────────────────────
if (test_name == "causal_iptw") {
    n <- 300
    age <- rnorm(n, 50, 10)
    female <- rbinom(n, 1, 0.5)
    ps <- plogis(-1 + 0.05 * age + 0.5 * female)
    treat <- rbinom(n, 1, ps)
    y <- 10 + 2 * treat + 0.1 * age + 3 * female + rnorm(n, sd = 2)
    df <- data.frame(treat = treat, y = y, age = age, female = female)
    data_path <- file.path(out_dir, "causal_iptw_data.csv")
    data.table::fwrite(df, data_path)

    # Reference: IPTW
    ps_model <- glm(treat ~ age + female, data = df, family = binomial)
    ps_score <- predict(ps_model, type = "response")
    ipw <- ifelse(treat == 1, 1/ps_score, 1/(1-ps_score))
    fit <- lm(y ~ treat, data = df, weights = ipw)
    smry <- summary(fit)
    ref <- data.frame(
        ate = coef(fit)["treat"],
        ate_se = smry$coefficients["treat", "Std. Error"],
        n_treated = sum(treat == 1),
        n_obs = n
    )
    data.table::fwrite(ref, file.path(out_dir, "causal_iptw_reference.csv"))
    cat("OK causal_iptw\n")
}

# ── mediation ──────────────────────────────────────────────────────────────
if (test_name == "mediation") {
    n <- 200
    x <- rnorm(n, 10, 3)
    m <- 0.5 + 0.8 * x + rnorm(n, sd = 1)
    y <- 1.0 + 0.3 * x + 0.6 * m + rnorm(n, sd = 1)
    df <- data.frame(x = x, m = m, y = y)
    data_path <- file.path(out_dir, "mediation_data.csv")
    data.table::fwrite(df, data_path)

    model_m <- lm(m ~ x, data = df)
    model_y <- lm(y ~ x + m, data = df)
    set.seed(42)
    med <- mediation::mediate(model_m, model_y, treat = "x", mediator = "m",
                              boot = TRUE, sims = 200)
    smry <- summary(med)
    ref <- data.frame(
        nde = smry$d0,
        nde_ci_lower = smry$d0.ci[1],
        nde_ci_upper = smry$d0.ci[2],
        nie = smry$z0,
        nie_ci_lower = smry$z0.ci[1],
        nie_ci_upper = smry$z0.ci[2],
        te = smry$tau.coef,
        alpha_x = coef(model_m)["x"],
        beta_x = coef(model_y)["x"],
        beta_m = coef(model_y)["m"],
        n_obs = nobs(model_y)
    )
    data.table::fwrite(ref, file.path(out_dir, "mediation_reference.csv"))
    cat("OK mediation\n")
}

# ── epi_rcs ────────────────────────────────────────────────────────────────
if (test_name == "epi_rcs") {
    n <- 300
    x <- rnorm(n, 50, 10)
    # Nonlinear relationship: quadratic + noise
    log_odds <- -2 + 0.05 * (x - 50) + 0.002 * (x - 50)^2
    prob <- 1 / (1 + exp(-log_odds))
    y <- rbinom(n, 1, prob)
    df <- data.frame(x = x, y = y)
    data_path <- file.path(out_dir, "epi_rcs_data.csv")
    data.table::fwrite(df, data_path)

    # Reference using rms
    ddist <- rms::datadist(df)
    options(datadist = "ddist")
    fit <- rms::lrm(y ~ rms::rcs(x, 4), data = df, x = TRUE)
    ref <- data.frame(
        lr_stat = fit$stats["Model L.R."],
        p_overall = 1 - pchisq(fit$stats["Model L.R."], fit$stats["d.f."]),
        n_knots = 4,
        n_obs = fit$stats["Obs"]
    )
    options(datadist = NULL)
    data.table::fwrite(ref, file.path(out_dir, "epi_rcs_reference.csv"))
    cat("OK epi_rcs\n")
}

# ── fine_gray ────────────────────────────────────────────────────────────────
# Competing-risks data. fstatus 0 = censored, 1 = event of interest, 2 = other.
if (test_name == "fine_gray") {
    n <- 400
    time <- round(pmin(rexp(n, 1 / 3), 10), 2) # ties + bounded follow-up
    x1 <- rnorm(n)
    x2 <- rnorm(n, 5, 2)
    x3 <- runif(n)
    z <- 0.4 * x1 - 0.3 * x2 + 0.5 * x3
    # subdistribution hazard of the event of interest
    p1 <- 1 - exp(-0.05 * exp(z) * time)
    p2 <- 0.25 * (1 - exp(-0.03 * time))
    u <- runif(n)
    fstatus <- ifelse(u < p1, 1, ifelse(u < p1 + p2, 2, 0))
    cengroup <- sample(1:2, n, replace = TRUE)
    df <- data.frame(time = time, fstatus = fstatus, x1 = x1, x2 = x2, x3 = x3,
                     cengroup = cengroup)
    data_path <- file.path(out_dir, "fine_gray_data.csv")
    data.table::fwrite(df, data_path)

    # Reference — call the original R package directly.
    fit <- cmprsk::crr(df$time, df$fstatus, cov1 = as.matrix(df[, c("x1", "x2", "x3")]),
                       cengroup = df$cengroup, failcode = 1, cencode = 0,
                       gtol = 1e-6, maxiter = 10, variance = TRUE)
    se <- sqrt(diag(as.matrix(fit$var)))
    smry <- summary(fit, conf.int = 0.95)
    ref <- data.frame(
        term = names(fit$coef),
        coefficient = as.numeric(fit$coef),
        subhazard_ratio = as.numeric(exp(fit$coef)),
        std_error = as.numeric(se),
        z_stat = as.numeric(fit$coef / se),
        p_value = as.numeric(2 * (1 - pnorm(abs(fit$coef / se)))),
        shr_ci_lower = as.numeric(exp(fit$coef + qnorm(0.025) * se)),
        shr_ci_upper = as.numeric(exp(fit$coef + qnorm(0.975) * se)),
        log_likelihood = fit$loglik,
        loglik_null = fit$loglik.null,
        lr_stat = as.numeric(-2 * (fit$loglik.null - fit$loglik)),
        lr_df = as.numeric(length(fit$coef)),
        lr_p_value = as.numeric(1 - pchisq(-2 * (fit$loglik.null - fit$loglik), length(fit$coef))),
        n_obs = fit$n,
        n_missing = fit$n.missing,
        n_events = sum(df$fstatus == 1),
        converged = fit$converged
    )
    data.table::fwrite(ref, file.path(out_dir, "fine_gray_reference.csv"))
    # Port 1: baseline cumulative incidence.
    base <- data.frame(
        uftime = as.numeric(fit$uftime),
        bfitj = as.numeric(fit$bfitj),
        baseline_cif = as.numeric(1 - exp(-cumsum(fit$bfitj)))
    )
    data.table::fwrite(base, file.path(out_dir, "fine_gray_reference_1.csv"))
    cat("OK fine_gray\n")
}

# ── fine_gray_tf ─────────────────────────────────────────────────────────────
# Same data, but with a time-interacted covariate (cov2 = x1, tf = x1^2).
if (test_name == "fine_gray_tf") {
    n <- 400
    time <- round(pmin(rexp(n, 1 / 3), 10), 2)
    x1 <- rnorm(n)
    x2 <- rnorm(n, 5, 2)
    z <- 0.4 * x1 - 0.3 * x2
    p1 <- 1 - exp(-0.05 * exp(z) * time)
    p2 <- 0.25 * (1 - exp(-0.03 * time))
    u <- runif(n)
    fstatus <- ifelse(u < p1, 1, ifelse(u < p1 + p2, 2, 0))
    df <- data.frame(time = time, fstatus = fstatus, x1 = x1, x2 = x2)
    data_path <- file.path(out_dir, "fine_gray_tf_data.csv")
    data.table::fwrite(df, data_path)

    fit <- cmprsk::crr(df$time, df$fstatus,
                       cov1 = as.matrix(df[, c("x1", "x2")]),
                       cov2 = as.matrix(df[, "x1", drop = FALSE]),
                       tf = function(uft) cbind(uft^2),
                       failcode = 1, cencode = 0, variance = TRUE)
    se <- sqrt(diag(as.matrix(fit$var)))
    ref <- data.frame(
        term = names(fit$coef),
        coefficient = as.numeric(fit$coef),
        subhazard_ratio = as.numeric(exp(fit$coef)),
        std_error = as.numeric(se),
        z_stat = as.numeric(fit$coef / se),
        p_value = as.numeric(2 * (1 - pnorm(abs(fit$coef / se)))),
        shr_ci_lower = as.numeric(exp(fit$coef + qnorm(0.025) * se)),
        shr_ci_upper = as.numeric(exp(fit$coef + qnorm(0.975) * se)),
        log_likelihood = fit$loglik,
        loglik_null = fit$loglik.null,
        lr_stat = as.numeric(-2 * (fit$loglik.null - fit$loglik)),
        lr_df = as.numeric(length(fit$coef)),
        lr_p_value = as.numeric(1 - pchisq(-2 * (fit$loglik.null - fit$loglik), length(fit$coef))),
        n_obs = fit$n,
        n_missing = fit$n.missing,
        n_events = sum(df$fstatus == 1),
        converged = fit$converged
    )
    data.table::fwrite(ref, file.path(out_dir, "fine_gray_tf_reference.csv"))
    base <- data.frame(
        uftime = as.numeric(fit$uftime),
        bfitj = as.numeric(fit$bfitj),
        baseline_cif = as.numeric(1 - exp(-cumsum(fit$bfitj)))
    )
    data.table::fwrite(base, file.path(out_dir, "fine_gray_tf_reference_1.csv"))
    cat("OK fine_gray_tf\n")
}

# ── cuminc ──────────────────────────────────────────────────────────────────
if (test_name == "cuminc") {
    n <- 400
    time <- round(pmin(rexp(n, 1 / 3), 10), 2)
    x1 <- rnorm(n)
    p1 <- 1 - exp(-0.05 * exp(0.4 * x1) * time)
    p2 <- 0.25 * (1 - exp(-0.03 * time))
    u <- runif(n)
    fstatus <- ifelse(u < p1, 1, ifelse(u < p1 + p2, 2, 0))
    group <- sample(1:3, n, replace = TRUE)
    df <- data.frame(time = time, fstatus = fstatus, x1 = x1, group = group)
    data_path <- file.path(out_dir, "cuminc_data.csv")
    data.table::fwrite(df, data_path)

    ci <- cmprsk::cuminc(df$time, df$fstatus, group = df$group, cencode = 0)
    # Flatten curves to long form, matching the node's port 0 row order
    # (cause-major then group, from names(ci) = "<group> <cause>").
    test_names <- names(ci)
    if (!is.null(ci$Tests)) {
        test_names <- test_names[test_names != "Tests"]
        tests <- data.frame(
            cause = rownames(ci$Tests),
            stat = as.numeric(ci$Tests[, "stat"]),
            p_value = as.numeric(ci$Tests[, "pv"]),
            df = as.integer(ci$Tests[, "df"]),
            stringsAsFactors = FALSE
        )
    }
    rows <- do.call(rbind, lapply(test_names, function(nm) {
        parts <- strsplit(nm, " ", fixed = TRUE)[[1]]
        cur <- ci[[nm]]
        data.frame(
            group = paste(parts[-length(parts)], collapse = " "),
            cause = parts[length(parts)],
            time = as.numeric(cur$time),
            est = as.numeric(cur$est),
            var = as.numeric(cur$var),
            stringsAsFactors = FALSE
        )
    }))
    data.table::fwrite(rows, file.path(out_dir, "cuminc_reference_0.csv"))
    data.table::fwrite(tests, file.path(out_dir, "cuminc_reference_1.csv"))
    cat("OK cuminc\n")
}

# ── mvmr (multivariable MR) ────────────────────────────────────────────────
if (test_name == "mvmr") {
    set.seed(42)
    n <- 200
    # Two correlated exposure effects on instruments
    bx1 <- rnorm(n, mean = 0.1, sd = 0.05)
    bx2 <- rnorm(n, mean = 0.05, sd = 0.03) + 0.3 * bx1
    sex1 <- runif(n, 0.01, 0.03)
    sex2 <- runif(n, 0.01, 0.03)
    # Outcome effect: causal effect of bx1=0.3, bx2=0.5, plus noise
    beta_yg <- 0.3 * bx1 + 0.5 * bx2 + rnorm(n, sd = 0.01)
    se_yg <- runif(n, 0.01, 0.03)
    snp <- paste0("rs", 1:n)
    df <- data.frame(snp = snp, bx1 = bx1, bx2 = bx2, beta_yg = beta_yg,
                     sex1 = sex1, sex2 = sex2, se_yg = se_yg)
    data_path <- file.path(out_dir, "mvmr_data.csv")
    data.table::fwrite(df, data_path)

    # Reference: IVW MVMR
    r_input <- MVMR::format_mvmr(
        BXGs = df[, c("bx1", "bx2")],
        BYG = df$beta_yg,
        seBXGs = df[, c("sex1", "sex2")],
        seBYG = df$se_yg,
        RSID = df$snp
    )
    ivw_res <- MVMR::ivw_mvmr(r_input)
    ref <- data.frame(
        exposure = paste0("exposure", 1:nrow(ivw_res)),
        estimate = ivw_res[, 1],
        se = ivw_res[, 2],
        t_stat = ivw_res[, 3],
        pvalue = ivw_res[, 4]
    )
    data.table::fwrite(ref, file.path(out_dir, "mvmr_reference.csv"))
    cat("OK mvmr\n")
}


# ── mice_impute_pmm ────────────────────────────────────────────────────────
if (test_name == "mice_impute_pmm") {
    set.seed(42)
    n <- 80
    x1 <- rnorm(n, mean = 5, sd = 2)
    x2 <- rnorm(n, mean = 0, sd = 1)
    y <- 3 + 2 * x1 - 1.5 * x2 + rnorm(n, sd = 1)
    # Inject ~15% missingness into y.
    missing_idx <- sample(seq_len(n), size = 12)
    y_mis <- y
    y_mis[missing_idx] <- NA_real_
    df <- data.frame(x1 = x1, x2 = x2, y = y_mis)
    data_path <- file.path(out_dir, "mice_impute_pmm_data.csv")
    data.table::fwrite(df, data_path)

    set.seed(42)
    imp <- mice.impute.pmm(
        y = df$y,
        ry = !is.na(df$y),
        x = cbind(df$x1, df$x2),
        wy = is.na(df$y),
        donors = 5,
        matchtype = 1
    )
    # Reference: emit summary stats + donor-set membership (deterministic
    # checks independent of the random donor selection).
    obs_y <- df$y[!is.na(df$y)]
    obs_mean <- mean(obs_y)
    obs_sd <- sd(obs_y)
    ref <- data.frame(
        imputed = as.numeric(imp),
        in_observed_set = as.numeric(imp) %in% obs_y,
        observed_mean = obs_mean,
        observed_sd = obs_sd,
        observed_min = min(obs_y),
        observed_max = max(obs_y)
    )
    data.table::fwrite(ref, file.path(out_dir, "mice_impute_pmm_reference.csv"))
    cat("OK mice_impute_pmm\n")
}

# ── mice_impute_norm ───────────────────────────────────────────────────────
if (test_name == "mice_impute_norm") {
    set.seed(42)
    n <- 100
    x1 <- rnorm(n, mean = 5, sd = 2)
    x2 <- rnorm(n, mean = 0, sd = 1)
    y <- 3 + 2 * x1 - 1.5 * x2 + rnorm(n, sd = 1)
    missing_idx <- sample(seq_len(n), size = 15)
    y_mis <- y
    y_mis[missing_idx] <- NA_real_
    df <- data.frame(x1 = x1, x2 = x2, y = y_mis)
    data_path <- file.path(out_dir, "mice_impute_norm_data.csv")
    data.table::fwrite(df, data_path)

    set.seed(42)
    imp <- mice.impute.norm(
        y = df$y,
        ry = !is.na(df$y),
        x = cbind(df$x1, df$x2),
        wy = is.na(df$y),
        ridge = 1e-5
    )
    # Compute the OLS prediction at the missing rows: this is the posterior
    # mean (ignoring σ*·z term) which is deterministic.
    fit <- lm(y ~ x1 + x2, data = df)
    pred_obs <- predict(fit, newdata = df[is.na(df$y), ])
    obs_y <- df$y[!is.na(df$y)]
    ref <- data.frame(
        imputed = as.numeric(imp),
        predicted_mean = as.numeric(pred_obs),
        deviation_from_mean = as.numeric(imp) - as.numeric(pred_obs),
        observed_mean = mean(obs_y),
        observed_sd = sd(obs_y)
    )
    data.table::fwrite(ref, file.path(out_dir, "mice_impute_norm_reference.csv"))
    cat("OK mice_impute_norm\n")
}

# ── mice_impute_logreg ─────────────────────────────────────────────────────
if (test_name == "mice_impute_logreg") {
    set.seed(42)
    n <- 100
    x1 <- rnorm(n)
    x2 <- rnorm(n)
    linpred <- -0.5 + 0.8 * x1 - 0.6 * x2
    prob <- 1 / (1 + exp(-linpred))
    y <- rbinom(n, 1, prob)
    missing_idx <- sample(seq_len(n), size = 20)
    y_mis <- y
    y_mis[missing_idx] <- NA_real_
    df <- data.frame(x1 = x1, x2 = x2, y = y_mis)
    data_path <- file.path(out_dir, "mice_impute_logreg_data.csv")
    data.table::fwrite(df, data_path)

    set.seed(42)
    imp <- mice.impute.logreg(
        y = df$y,
        ry = !is.na(df$y),
        x = cbind(df$x1, df$x2),
        wy = is.na(df$y)
    )
    # Compute predicted probability at missing rows + binarised version.
    fit <- glm(y ~ x1 + x2, data = df, family = binomial)
    pred_obs <- predict(fit, newdata = df[is.na(df$y), ], type = "response")
    ref <- data.frame(
        imputed = as.numeric(imp),
        is_binary = as.numeric(imp) %in% c(0, 1),
        predicted_prob = as.numeric(pred_obs),
        binarised_pred = as.numeric(pred_obs > 0.5),
        observed_mean = mean(df$y[!is.na(df$y)])
    )
    data.table::fwrite(ref, file.path(out_dir, "mice_impute_logreg_reference.csv"))
    cat("OK mice_impute_logreg\n")
}

# ── mice_impute_mean ───────────────────────────────────────────────────────
if (test_name == "mice_impute_mean") {
    set.seed(42)
    n <- 50
    y <- rnorm(n, mean = 10, sd = 3)
    missing_idx <- sample(seq_len(n), size = 10)
    y_mis <- y
    y_mis[missing_idx] <- NA_real_
    df <- data.frame(y = y_mis)
    data_path <- file.path(out_dir, "mice_impute_mean_data.csv")
    data.table::fwrite(df, data_path)

    set.seed(42)
    imp <- mice.impute.mean(y = df$y, ry = !is.na(df$y))
    obs_y <- df$y[!is.na(df$y)]
    obs_mean <- mean(obs_y)
    ref <- data.frame(
        imputed = as.numeric(imp),
        observed_mean = obs_mean,
        deviation_from_mean = as.numeric(imp) - obs_mean
    )
    data.table::fwrite(ref, file.path(out_dir, "mice_impute_mean_reference.csv"))
    cat("OK mice_impute_mean\n")
}

# ── mice_impute_sample ─────────────────────────────────────────────────────
if (test_name == "mice_impute_sample") {
    set.seed(42)
    n <- 50
    y <- rnorm(n, mean = 10, sd = 3)
    missing_idx <- sample(seq_len(n), size = 10)
    y_mis <- y
    y_mis[missing_idx] <- NA_real_
    df <- data.frame(y = y_mis)
    data_path <- file.path(out_dir, "mice_impute_sample_data.csv")
    data.table::fwrite(df, data_path)

    set.seed(42)
    imp <- mice.impute.sample(y = df$y, ry = !is.na(df$y))
    obs_y <- df$y[!is.na(df$y)]
    obs_mean <- mean(obs_y)
    obs_sd <- sd(obs_y)
    ref <- data.frame(
        imputed = as.numeric(imp),
        in_observed_set = as.numeric(imp) %in% obs_y,
        observed_mean = obs_mean,
        observed_sd = obs_sd,
        observed_min = min(obs_y),
        observed_max = max(obs_y)
    )
    data.table::fwrite(ref, file.path(out_dir, "mice_impute_sample_reference.csv"))
    cat("OK mice_impute_sample\n")
}

# ── mice (orchestrator) ────────────────────────────────────────────────────
if (test_name == "mice") {
    set.seed(42)
    n <- 60
    x1 <- rnorm(n, mean = 5, sd = 2)
    x2 <- rnorm(n, mean = 0, sd = 1)
    y <- 3 + 2 * x1 - 1.5 * x2 + rnorm(n, sd = 1)
    z <- rnorm(n)
    # Inject ~20% missingness into both y and z.
    miss_y <- sample(seq_len(n), size = 12)
    miss_z <- sample(seq_len(n), size = 12)
    y_mis <- y
    y_mis[miss_y] <- NA_real_
    z_mis <- z
    z_mis[miss_z] <- NA_real_
    df <- data.frame(x1 = x1, x2 = x2, y = y_mis, z = z_mis)
    data_path <- file.path(out_dir, "mice_data.csv")
    data.table::fwrite(df, data_path)

    set.seed(42)
    mids <- mice(
        df,
        m = 3,
        method = c("", "", "norm", "norm"),
        maxit = 3,
        printFlag = FALSE,
        ridge = 1e-5,
        donors = 5,
        matchtype = 1
    )
    comp <- complete(mids, action = "long", include = FALSE)
    # Output the long-format completed data frame (deterministic structure
    # + per-row imputed values; we check the structural invariants downstream).
    data.table::fwrite(comp, file.path(out_dir, "mice_reference.csv"))
    # Also output per-imputation summary statistics for the y column.
    comp_y <- as.data.frame(comp)[, c(".imp", "y")]
    summ <- data.frame(
        imp_col = unique(comp_y[, ".imp"]),
        n_rows = as.numeric(table(comp_y[, ".imp"])),
        y_mean = as.numeric(tapply(comp_y[, "y"], comp_y[, ".imp"], mean)),
        y_sd = as.numeric(tapply(comp_y[, "y"], comp_y[, ".imp"], sd)),
        y_min = as.numeric(tapply(comp_y[, "y"], comp_y[, ".imp"], min)),
        y_max = as.numeric(tapply(comp_y[, "y"], comp_y[, ".imp"], max))
    )
    data.table::fwrite(summ, file.path(out_dir, "mice_reference_summary.csv"))
    cat("OK mice\n")
}
