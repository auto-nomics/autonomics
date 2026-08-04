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
