#!/usr/bin/env Rscript

options(width = 200L)
sink(Sys.getenv("AUTONOMICS_OUTPUT2"), split = TRUE)
on.exit(sink(), add = TRUE)

env_number <- function(name, default) {
  value <- Sys.getenv(name, unset = NA)
  if (is.na(value) || !nzchar(value)) default else as.numeric(value)
}

env_logical <- function(name, default) {
  value <- Sys.getenv(name, unset = NA)
  if (is.na(value) || !nzchar(value)) {
    default
  } else {
    tolower(value) %in% c("1", "true", "yes")
  }
}

env_string <- function(name, default) {
  value <- Sys.getenv(name, unset = NA)
  if (is.na(value) || !nzchar(value)) default else value
}

input_path <- Sys.getenv("AUTONOMICS_INPUT0")
sumstats <- read.delim(input_path, check.names = FALSE, stringsAsFactors = FALSE)
required <- c("snp", "chrom", "z")
missing <- setdiff(required, names(sumstats))
if (length(missing) > 0) {
  stop("input is missing required columns: ", paste(missing, collapse = ", "))
}

chroms <- unique(sumstats$chrom)
if (length(chroms) != 1L) {
  stop("susie_rss container requires exactly one chromosome per run")
}
chrom <- as.integer(chroms[1])
snps <- as.character(sumstats$snp)
z <- as.numeric(sumstats$z)
if (anyNA(z)) {
  stop("z column contains missing values")
}

n_value <- NULL
if ("n" %in% names(sumstats) && any(!is.na(sumstats$n))) {
  n_value <- as.numeric(sumstats$n[1])
}
if (nzchar(Sys.getenv("SUSIE_N", unset = ""))) {
  n_value <- as.numeric(Sys.getenv("SUSIE_N"))
}

allele_from_id <- function(snp, allele_index) {
  parts <- strsplit(snp, ":", fixed = TRUE)[[1]]
  if (length(parts) < 4L) {
    NA_character_
  } else if (allele_index == 1L) {
    parts[length(parts) - 1L]
  } else {
    parts[length(parts)]
  }
}

a1 <- if ("a1" %in% names(sumstats)) {
  as.character(sumstats$a1)
} else {
  vapply(snps, allele_from_id, character(1), allele_index = 1L)
}
a2 <- if ("a2" %in% names(sumstats)) {
  as.character(sumstats$a2)
} else {
  vapply(snps, allele_from_id, character(1), allele_index = 2L)
}
if (anyNA(a1) || anyNA(a2) || any(!nzchar(a1)) || any(!nzchar(a2))) {
  stop("missing A1/A2 for one or more SNPs; provide a1/a2 columns or chr:pos:a1:a2 IDs")
}

workdir <- Sys.getenv("AUTONOMICS_WORKDIR")
query_path <- file.path(workdir, ".autonomics", "susie_query.tsv")
dir.create(dirname(query_path), recursive = TRUE, showWarnings = FALSE)
write.table(
  data.frame(
    SNP = snps,
    A1 = a1,
    A2 = a2,
    N = if (is.null(n_value)) 500 else n_value,
    Z = z
  ),
  query_path,
  sep = "\t",
  row.names = FALSE,
  quote = FALSE
)

pairs_path <- file.path(workdir, ".autonomics", "susie_ld_pairs.tsv")
stderr_path <- paste0(pairs_path, ".stderr")
query_script <- "/opt/susie/bin/susie_ld_query.py"
r2_min <- env_number("SUSIE_R2_MIN", 0.0)
status <- system2(
  "python3",
  c(
    query_script,
    "--engine-home", "/opt/mixer",
    "--lib", "/opt/mixer/lib/libbgmg.so",
    "--bim-file", "/panels/mixer_ref/stage_flat/chr@.bim",
    "--ld-file", "/panels/mixer_ref/ld_mixer/1000G.EUR.chr@",
    "--chrom", as.character(chrom),
    "--trait1-file", query_path,
    "--r2-min", format(r2_min, scientific = FALSE)
  ),
  stdout = pairs_path,
  stderr = stderr_path
)
if (status != 0) {
  stderr_text <- if (file.exists(stderr_path)) {
    paste(readLines(stderr_path, warn = FALSE), collapse = "\n")
  } else {
    ""
  }
  stop("susie LD query failed with status ", status, ": ", stderr_text)
}

raw_pairs <- readLines(pairs_path, warn = FALSE)
pair_rows <- grep("^#", raw_pairs, invert = TRUE, value = TRUE)
pair_rows <- pair_rows[nzchar(pair_rows)]
if (length(pair_rows) > 0) {
  pairs <- read.table(
    text = paste(pair_rows, collapse = "\n"),
    sep = "\t",
    header = FALSE,
    col.names = c("id_a", "id_b", "r"),
    stringsAsFactors = FALSE
  )
} else {
  pairs <- data.frame(
    id_a = character(),
    id_b = character(),
    r = numeric()
  )
}

matched <- sub("^#matched\t", "", grep("^#matched\t", raw_pairs, value = TRUE))
missing_snps <- setdiff(snps, matched)
if (length(missing_snps) > 0) {
  stop(
    "input SNP(s) did not align to the signed-LD reference: ",
    paste(head(missing_snps, 5), collapse = ", ")
  )
}

p <- length(snps)
r_matrix <- matrix(0, nrow = p, ncol = p, dimnames = list(snps, snps))
diag(r_matrix) <- 1.0
if (nrow(pairs) > 0) {
  for (i in seq_len(nrow(pairs))) {
    id_a <- pairs$id_a[i]
    id_b <- pairs$id_b[i]
    r_matrix[id_a, id_b] <- pairs$r[i]
    r_matrix[id_b, id_a] <- pairs$r[i]
  }
}

if (p > 1L && sum(r_matrix[upper.tri(r_matrix)] != 0) == 0) {
  stop("no signed LD pairs overlap the input locus")
}

l <- as.integer(env_number("SUSIE_L", 10))
estimate_prior_method <- env_string("SUSIE_ESTIMATE_PRIOR_METHOD", "optim")
estimate_residual_variance <- env_logical("SUSIE_ESTIMATE_RESIDUAL_VARIANCE", FALSE)
estimate_prior_variance <- env_logical("SUSIE_ESTIMATE_PRIOR_VARIANCE", TRUE)
coverage <- env_number("SUSIE_COVERAGE", 0.95)
min_abs_corr <- env_number("SUSIE_MIN_ABS_CORR", 0.5)
scaled_prior_variance <- env_number("SUSIE_SCALED_PRIOR_VARIANCE", 0.2)
z_method <- env_string("SUSIE_Z_METHOD", "wald")
check_null_threshold <- env_number("SUSIE_CHECK_NULL_THRESHOLD", 0.0)
max_iter <- as.integer(env_number("SUSIE_MAX_ITER", 100))

fit <- susieR::susie_rss(
  z = z,
  R = r_matrix,
  n = n_value,
  L = l,
  scaled_prior_variance = scaled_prior_variance,
  estimate_residual_variance = estimate_residual_variance,
  estimate_prior_variance = estimate_prior_variance,
  estimate_prior_method = estimate_prior_method,
  z_method = z_method,
  check_null_threshold = check_null_threshold,
  coverage = coverage,
  min_abs_corr = min_abs_corr,
  max_iter = max_iter
)

cat("## susieR susie_rss 0.16.6\n")
cat("variants:", p, "\n")
cat("niter:", fit$niter, "\n")
cat("converged:", isTRUE(fit$converged), "\n")
cs_set <- fit$sets$cs
if (!is.null(cs_set)) {
  cat("credible sets:", length(cs_set), "\n")
  print(cs_set)
} else {
  cat("credible sets: 0\n")
}

pip <- susieR::susie_get_pip(fit)
cs_membership <- integer(p)
if (!is.null(cs_set)) {
  for (k in seq_along(cs_set)) {
    cs_membership[as.integer(cs_set[[k]])] <- k
  }
}

alpha <- fit$alpha
mu <- fit$mu
mu2 <- fit$mu2
lbf <- fit$lbf_variable
best_effect <- apply(alpha, 2, which.max)
best_alpha <- apply(alpha, 2, max)
best_mu <- mu[cbind(best_effect, seq_len(p))]
best_mu2 <- mu2[cbind(best_effect, seq_len(p))]
best_lbf <- lbf[cbind(best_effect, seq_len(p))]

out <- data.frame(
  snp = snps,
  pip = pip,
  cs = cs_membership,
  alpha = best_alpha,
  mu = best_mu,
  mu2 = best_mu2,
  lbf = best_lbf,
  stringsAsFactors = FALSE
)
write.table(out, Sys.getenv("AUTONOMICS_OUTPUT0"), sep = "\t", row.names = FALSE, quote = FALSE)
saveRDS(fit, Sys.getenv("AUTONOMICS_OUTPUT1"))
