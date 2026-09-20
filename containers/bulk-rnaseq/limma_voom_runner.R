#!/usr/bin/env Rscript
options(stringsAsFactors = FALSE, digits = 15)

required_env <- function(name) {
  value <- Sys.getenv(name)
  if (!nzchar(value)) {
    stop(sprintf("missing required environment variable: %s", name), call. = FALSE)
  }
  value
}

split_csv <- function(value) {
  if (!nzchar(value)) return(character())
  values <- trimws(strsplit(value, ",", fixed = TRUE)[[1]])
  values[nzchar(values)]
}

required_env("AUTONOMICS_INPUT0")
required_env("AUTONOMICS_INPUT1")

count_path <- Sys.getenv("AUTONOMICS_INPUT0")
metadata_path <- Sys.getenv("AUTONOMICS_INPUT1")
results_path <- Sys.getenv("AUTONOMICS_OUTPUT0")
weights_path <- Sys.getenv("AUTONOMICS_OUTPUT1")
normalized_path <- Sys.getenv("AUTONOMICS_OUTPUT2")
contrast_summary_path <- Sys.getenv("AUTONOMICS_OUTPUT3")
report_path <- Sys.getenv("AUTONOMICS_OUTPUT4")

outcome_var <- required_env("AUTONOMICS_LIMMA_OUTCOME")
covariates <- split_csv(Sys.getenv("AUTONOMICS_LIMMA_COVARIATES"))
contrast_var <- required_env("AUTONOMICS_LIMMA_CONTRAST_VAR")
contrast_levels <- split_csv(Sys.getenv("AUTONOMICS_LIMMA_CONTRAST_LEVELS"))
outcome_mode <- Sys.getenv("AUTONOMICS_LIMMA_OUTCOME_MODE", "categorical")
normalization <- Sys.getenv("AUTONOMICS_LIMMA_NORMALIZATION", "tmm")
alpha <- as.numeric(Sys.getenv("AUTONOMICS_LIMMA_ALPHA", "0.05"))
threads <- as.integer(Sys.getenv("AUTONOMICS_LIMMA_THREADS", "1"))

if (!outcome_mode %in% c("categorical", "continuous")) {
  stop("outcome_mode must be 'categorical' or 'continuous'", call. = FALSE)
}
if (!normalization %in% c("tmm", "quantile", "none")) {
  stop("normalization must be tmm, quantile, or none", call. = FALSE)
}
if (is.na(alpha) || !is.finite(alpha) || alpha <= 0 || alpha >= 1) {
  stop("alpha must lie strictly between 0 and 1", call. = FALSE)
}
if (is.na(threads) || threads < 1) {
  stop("threads must be a positive integer", call. = FALSE)
}

counts_frame <- read.delim(
  count_path,
  check.names = FALSE,
  stringsAsFactors = FALSE,
  na.strings = "NA"
)
metadata_frame <- read.delim(
  metadata_path,
  check.names = FALSE,
  stringsAsFactors = FALSE,
  na.strings = "NA"
)

if (ncol(counts_frame) < 2 || !identical(colnames(counts_frame)[[1]], "gene_id")) {
  stop("count matrix must start with gene_id followed by sample columns", call. = FALSE)
}
if (anyDuplicated(colnames(counts_frame))) {
  stop("count matrix column names must be unique", call. = FALSE)
}
gene_ids <- counts_frame[[1]]
if (anyNA(gene_ids) || !all(nzchar(gene_ids))) {
  stop("gene_id cannot be empty", call. = FALSE)
}
if (anyDuplicated(gene_ids)) {
  stop("gene_id values must be unique", call. = FALSE)
}
count_samples <- colnames(counts_frame)[-1]
counts <- suppressWarnings(matrix(
  as.numeric(as.matrix(counts_frame[, -1])),
  nrow = nrow(counts_frame)
))
rownames(counts) <- gene_ids
colnames(counts) <- count_samples
if (anyNA(counts) || any(!is.finite(counts)) || any(counts < 0)) {
  stop("all count values must be finite nonnegatives", call. = FALSE)
}

if (ncol(metadata_frame) < 2 || !identical(colnames(metadata_frame)[[1]], "sample_id")) {
  stop("sample metadata must start with sample_id", call. = FALSE)
}
if (anyDuplicated(colnames(metadata_frame))) {
  stop("sample metadata column names must be unique", call. = FALSE)
}
if (anyDuplicated(metadata_frame[[1]])) {
  stop("sample_id values must be unique", call. = FALSE)
}
rownames(metadata_frame) <- metadata_frame[[1]]
if (!setequal(count_samples, metadata_frame[[1]])) {
  stop("count matrix and metadata sample sets differ", call. = FALSE)
}
metadata_frame <- metadata_frame[count_samples, , drop = FALSE]

required_columns <- unique(c(outcome_var, covariates, contrast_var))
missing <- setdiff(required_columns, colnames(metadata_frame))
if (length(missing) > 0) {
  stop(sprintf("metadata is missing required columns: %s", paste(missing, collapse = ", ")), call. = FALSE)
}

# Convert categorical outcome / contrast columns to factors with explicit
# reference level ordering when supplied via CONTRAST_LEVELS. Limma's
# makeContrasts consumes the resulting model matrix deterministically.
design_terms <- character()
for (column in unique(c(outcome_var, covariates))) {
  original_values <- metadata_frame[[column]]
  if (anyNA(original_values)) {
    stop(sprintf("metadata column `%s` contains NA", column), call. = FALSE)
  }
  if (column == outcome_var && outcome_mode == "categorical") {
    values <- as.character(original_values)
    if (length(contrast_levels) >= 2) {
      values <- factor(values, levels = contrast_levels)
    } else {
      values <- factor(values)
    }
    metadata_frame[[column]] <- values
    design_terms <- c(design_terms, column)
  } else {
    # Preserve numeric covariates and the continuous outcome. Character or
    # factor covariates still enter model.matrix as categorical terms.
    design_terms <- c(design_terms, column)
  }
}

if (!identical(contrast_var, outcome_var)) {
  stop("contrast_var must equal outcome_var in the first contract", call. = FALSE)
}
if (identical(outcome_mode, "categorical")) {
  formula <- paste("~ 0 +", paste(design_terms, collapse = " + "))
} else {
  formula <- paste("~", paste(design_terms, collapse = " + "))
}
design_matrix <- model.matrix(as.formula(formula), data = metadata_frame)
# Limma contrasts require syntactically valid coefficient names even when a
# metadata factor contains values such as "single-read".
colnames(design_matrix) <- make.names(colnames(design_matrix))

if (identical(outcome_mode, "categorical")) {
  # contrast_levels is [reference, test]; report test-versus-reference logFC.
  test_coefficient <- make.names(paste0(contrast_var, contrast_levels[2]))
  reference_coefficient <- make.names(paste0(contrast_var, contrast_levels[1]))
  contrast <- limma::makeContrasts(
    contrasts = sprintf("%s - %s", test_coefficient, reference_coefficient),
    levels = design_matrix
  )
  contrast_label <- sprintf("%s_%s_vs_%s", contrast_var, contrast_levels[2], contrast_levels[1])
} else {
  if (length(contrast_levels) > 0) {
    stop("CONTRAST_LEVELS only applies to categorical outcome", call. = FALSE)
  }
  if (!nzchar(contrast_var)) {
    contrast_var <- outcome_var
  }
  contrast <- NULL
  contrast_label <- sprintf("%s_continuous", contrast_var)
}

dge <- edgeR::DGEList(counts = counts)
keep <- edgeR::filterByExpr(dge, design = design_matrix)
dge <- dge[keep, , keep.lib.sizes = FALSE]
if (identical(normalization, "tmm")) {
  dge <- edgeR::calcNormFactors(dge, method = "TMM")
}

if (identical(normalization, "quantile")) {
  logcpm <- log2(edgeR::cpm(dge, log = FALSE) + 1)
  normalized_expr <- limma::normalizeBetweenArrays(logcpm, method = "quantile")
  dge_voom <- edgeR::DGEList(counts = dge$counts, norm.factors = rep(1, ncol(dge)))
  v <- limma::voom(dge_voom, design = design_matrix, normalize.method = "none", plot = FALSE)
  exprs_matrix <- normalized_expr[rownames(v$E), , drop = FALSE]
  v$E <- exprs_matrix
} else if (identical(normalization, "none")) {
  v <- limma::voom(dge, design = design_matrix, normalize.method = "none", plot = FALSE)
} else {
  v <- limma::voom(dge, design = design_matrix, plot = FALSE)
}

fit <- limma::lmFit(v, design_matrix)
fit <- limma::eBayes(fit)

if (is.null(contrast)) {
  coef_idx <- which(colnames(design_matrix) == contrast_var)
  if (length(coef_idx) != 1L) {
    stop(sprintf("unable to locate coefficient for `%s`", contrast_var), call. = FALSE)
  }
  top <- limma::topTable(
    fit,
    coef = coef_idx,
    number = Inf,
    sort.by = "none",
    confint = FALSE,
    adjust.method = "BH"
  )
} else {
  fit <- limma::contrasts.fit(fit, contrast)
  fit <- limma::eBayes(fit)
  top <- limma::topTable(
    fit,
    number = Inf,
    sort.by = "none",
    confint = FALSE,
    adjust.method = "BH"
  )
}

results_frame <- data.frame(
  gene_id = rownames(top),
  top,
  check.names = FALSE,
  stringsAsFactors = FALSE
)
write.table(results_frame, results_path, sep = "\t", quote = FALSE, row.names = FALSE)

weights_frame <- data.frame(
  gene_id = rownames(v$E),
  v$weights,
  check.names = FALSE,
  stringsAsFactors = FALSE
)
write.table(weights_frame, weights_path, sep = "\t", quote = FALSE, row.names = FALSE)

normalized_frame <- data.frame(
  gene_id = rownames(v$E),
  v$E,
  check.names = FALSE,
  stringsAsFactors = FALSE
)
write.table(normalized_frame, normalized_path, sep = "\t", quote = FALSE, row.names = FALSE)

contrast_summary <- data.frame(
  outcome_mode = outcome_mode,
  outcome_var = outcome_var,
  contrast_var = contrast_var,
  contrast_label = contrast_label,
  normalization = normalization,
  covariates = if (length(covariates)) paste(covariates, collapse = ",") else "",
  design = paste(design_terms, collapse = " + "),
  genes_kept = nrow(v$E),
  samples = ncol(v$E),
  alpha = alpha,
  threads = threads,
  stringsAsFactors = FALSE
)
write.table(contrast_summary, contrast_summary_path, sep = "\t", quote = FALSE, row.names = FALSE)

file_checksum <- function(path) unname(tools::md5sum(path))

report <- list(
  schema_version = "1.0",
  node = "limma_voom_container",
  analysis = list(
    outcome_mode = outcome_mode,
    outcome_var = outcome_var,
    contrast_var = contrast_var,
    contrast_levels = contrast_levels,
    normalization = normalization,
    covariates = covariates,
    design = paste(design_terms, collapse = " + "),
    alpha = alpha,
    threads = threads,
    formula = formula
  ),
  engine = list(
    package = c("limma", "edgeR"),
    versions = list(
      limma = as.character(packageVersion("limma")),
      edgeR = as.character(packageVersion("edgeR"))
    ),
    bioconductor_release = Sys.getenv("BIOCONDUCTOR_RELEASE"),
    r_version = paste(R.version$major, R.version$minor, sep = ".")
  ),
  inputs = list(
    count_matrix = list(path = basename(count_path), md5 = file_checksum(count_path)),
    sample_metadata = list(path = basename(metadata_path), md5 = file_checksum(metadata_path))
  ),
  dimensions = list(
    genes_total = nrow(counts),
    genes_after_filter = nrow(v$E),
    samples = ncol(counts),
    design_columns = ncol(design_matrix),
    design_colnames = colnames(design_matrix)
  ),
  outputs = list(
    results = list(path = basename(results_path), md5 = file_checksum(results_path)),
    weights = list(path = basename(weights_path), md5 = file_checksum(weights_path)),
    normalized = list(path = basename(normalized_path), md5 = file_checksum(normalized_path)),
    contrast_summary = list(path = basename(contrast_summary_path), md5 = file_checksum(contrast_summary_path))
  )
)
jsonlite::write_json(
  report,
  report_path,
  pretty = TRUE,
  auto_unbox = TRUE,
  digits = 15,
  na = "null"
)

cat("limma_voom complete: ", nrow(v$E), " genes x ", ncol(v$E), " samples\n", sep = "")
