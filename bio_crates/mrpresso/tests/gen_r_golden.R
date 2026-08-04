#!/usr/bin/env Rscript
# Generate golden MR-PRESSO output from the reference R package (MRPRESSO v1.0)
# for cross-validation against the Rust port.
#
# Usage:
#   Rscript tests/gen_r_golden.R <out.json>
#
# Writes a JSON object keyed by scenario name. Each scenario captures the full
# output structure needed to cross-validate the Rust port:
#   * global: RSSobs, Pvalue (numeric + formatted)
#   * outlier: per-row RSSobs, Pvalue, and which rows are significant
#   * distortion: coefficient (per exposure), Pvalue
#   * main_mr: raw + outlier-corrected rows (Estimate, Sd, T-stat, P-value)
#   * n: number of rows after NA removal
#   * labels: 1-based row indices (the cleaned data's row positions)

suppressMessages(library(MRPRESSO))
suppressMessages(library(jsonlite))
data(SummaryStats)

# R formats a raw p-value of exactly 0 as "<prec" (the smallest representable
# step). Recover the exact count: "<..." always corresponds to raw 0.
pnum <- function(x) {
    x <- as.character(x)
    out <- suppressWarnings(as.numeric(x))
    out[grepl("^<", x)] <- 0
    out
}

run_scenario <- function(beta_exp, sd_exp, label, seed, outlier = TRUE, distortion = TRUE,
                         threshold = 0.05, nb = 1000) {
    set.seed(seed)
    res <- suppressWarnings(
        mr_presso(
            BetaOutcome = "Y_effect",
            BetaExposure = beta_exp,
            SdOutcome = "Y_se",
            SdExposure = sd_exp,
            OUTLIERtest = outlier,
            DISTORTIONtest = distortion,
            data = SummaryStats,
            NbDistribution = nb,
            SignifThreshold = threshold
        )
    )
    main <- res[["Main MR results"]]
    mrpresso_res <- res[["MR-PRESSO results"]]

    gt <- mrpresso_res[["Global Test"]]
    out <- list(
        label = label,
        global = list(
            RSSobs = as.numeric(gt$RSSobs),
            Pvalue = pnum(gt$Pvalue),
            Pvalue_str = as.character(gt$Pvalue)
        )
    )

    # Main MR results
    out$main_mr <- list(
        exposure = as.character(main$Exposure),
        analysis = as.character(main$`MR Analysis`),
        estimate = as.numeric(main$`Causal Estimate`),
        sd = as.numeric(main$Sd),
        t_stat = as.numeric(main$`T-stat`),
        pvalue = as.numeric(main$`P-value`)
    )

    # Outlier test
    if ("Outlier Test" %in% names(mrpresso_res)) {
        ot <- mrpresso_res[["Outlier Test"]]
        out$outlier <- list(
            index = as.integer(rownames(ot)),
            rss = as.numeric(ot$RSSobs),
            pvalue = pnum(ot$Pvalue),
            # R formats 0 as "<n/nb"
            pvalue_str = as.character(ot$Pvalue),
            # 1-based indices of significant outliers
            sig = which(pnum(ot$Pvalue) <= threshold)
        )
    }

    # Distortion test
    if ("Distortion Test" %in% names(mrpresso_res)) {
        dt <- mrpresso_res[["Distortion Test"]]
        out$distortion <- list(
            indices = as.integer(dt$`Outliers Indices`),
            indices_str = as.character(dt$`Outliers Indices`),
            coefficient = as.numeric(dt$`Distortion Coefficient`),
            coefficient_names = names(dt$`Distortion Coefficient`),
            pvalue = pnum(dt$Pvalue),
            pvalue_str = as.character(dt$Pvalue)
        )
    }

    out$n <- nrow(SummaryStats)  # (no NA rows in SummaryStats)
    out
}

scenarios <- list(
    single = run_scenario("E1_effect", "E1_se", "single-E1", 123),
    multi = run_scenario(c("E1_effect", "E2_effect"), c("E1_se", "E2_se"), "multi-E1E2", 123),
    single_no_outlier = run_scenario("E1_effect", "E1_se", "single-no-outlier", 123,
                                     outlier = TRUE, distortion = FALSE),
    global_only = run_scenario("E1_effect", "E1_se", "global-only", 42,
                               outlier = FALSE, distortion = FALSE),
    single_other_seed = run_scenario("E1_effect", "E1_se", "single-other-seed", 7)
)

out_file <- commandArgs(trailingOnly = TRUE)[1]
writeLines(toJSON(scenarios, auto_unbox = TRUE, digits = 15), out_file)
cat("wrote", out_file, "\n")
