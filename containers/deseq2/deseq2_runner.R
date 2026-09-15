options(stringsAsFactors = FALSE, digits = 15)

required_env <- function(name) {
  value <- Sys.getenv(name)
  if (!nzchar(value)) {
    stop(sprintf("missing required environment variable: %s", name), call. = FALSE)
  }
  value
}

parse_covariates <- function(value) {
  if (!nzchar(value)) {
    return(character())
  }
  values <- trimws(strsplit(value, ",", fixed = TRUE)[[1]])
  values <- values[nzchar(values)]
  if (anyDuplicated(values)) {
    stop("covariates cannot contain duplicates", call. = FALSE)
  }
  values
}

count_path <- required_env("AUTONOMICS_INPUT0")
metadata_path <- required_env("AUTONOMICS_INPUT1")
results_path <- required_env("AUTONOMICS_OUTPUT0")
normalized_path <- required_env("AUTONOMICS_OUTPUT1")
size_factors_path <- required_env("AUTONOMICS_OUTPUT2")
dataset_path <- required_env("AUTONOMICS_OUTPUT3")
report_path <- required_env("AUTONOMICS_OUTPUT4")

condition_reference <- required_env("AUTONOMICS_DESEQ2_CONDITION_REFERENCE")
condition_test <- required_env("AUTONOMICS_DESEQ2_CONDITION_TEST")
covariates <- parse_covariates(Sys.getenv("AUTONOMICS_DESEQ2_COVARIATES"))
fit_type <- Sys.getenv("AUTONOMICS_DESEQ2_FIT_TYPE", "parametric")
alpha <- as.numeric(Sys.getenv("AUTONOMICS_DESEQ2_ALPHA", "0.1"))
threads <- as.integer(Sys.getenv("AUTONOMICS_DESEQ2_THREADS", "1"))

if (identical(condition_reference, condition_test)) {
  stop("condition_test and condition_reference must differ", call. = FALSE)
}
if (!fit_type %in% c("parametric", "local", "mean")) {
  stop("fit_type must be parametric, local, or mean", call. = FALSE)
}
if (is.na(alpha) || !is.finite(alpha) || alpha <= 0 || alpha >= 1) {
  stop("alpha must be finite and lie strictly between 0 and 1", call. = FALSE)
}
if (is.na(threads) || threads != 1) {
  stop("threads must be 1 in the deterministic first contract", call. = FALSE)
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
counts <- suppressWarnings(matrix(as.numeric(as.matrix(counts_frame[, -1])), nrow = nrow(counts_frame)))
rownames(counts) <- gene_ids
colnames(counts) <- count_samples
if (anyNA(counts) || any(!is.finite(counts)) || any(counts < 0) || any(counts != floor(counts))) {
  stop("all count values must be finite nonnegative integers", call. = FALSE)
}

if (ncol(metadata_frame) < 2 || !identical(colnames(metadata_frame)[[1]], "sample_id")) {
  stop("sample metadata must start with sample_id", call. = FALSE)
}
if (anyDuplicated(colnames(metadata_frame))) {
  stop("sample metadata column names must be unique", call. = FALSE)
}
metadata_samples <- metadata_frame[[1]]
if (anyNA(metadata_samples) || !all(nzchar(metadata_samples))) {
  stop("sample_id cannot be empty", call. = FALSE)
}
if (anyDuplicated(metadata_samples)) {
  stop("sample_id values must be unique", call. = FALSE)
}
if (!setequal(metadata_samples, count_samples)) {
  stop("count matrix and metadata sample sets must match exactly", call. = FALSE)
}

metadata_frame <- metadata_frame[match(count_samples, metadata_samples), , drop = FALSE]
rownames(metadata_frame) <- metadata_frame$sample_id

if (!"condition" %in% colnames(metadata_frame)) {
  stop("sample metadata requires a condition column", call. = FALSE)
}
condition_values <- as.character(metadata_frame$condition)
if (anyNA(condition_values) || !all(nzchar(condition_values))) {
  stop("condition cannot contain missing values", call. = FALSE)
}
observed_conditions <- unique(condition_values)
if (!setequal(observed_conditions, c(condition_reference, condition_test))) {
  stop(
    sprintf(
      "condition must contain exactly %s and %s",
      condition_reference,
      condition_test
    ),
    call. = FALSE
  )
}
metadata_frame$condition <- factor(
  condition_values,
  levels = c(condition_reference, condition_test)
)

if (any(c("sample_id", "condition") %in% covariates)) {
  stop("sample_id and condition cannot be listed as covariates", call. = FALSE)
}
missing_covariates <- setdiff(covariates, colnames(metadata_frame))
if (length(missing_covariates) > 0) {
  stop(
    sprintf("missing covariate columns: %s", paste(missing_covariates, collapse = ", ")),
    call. = FALSE
  )
}
identifier_pattern <- "^[A-Za-z.][A-Za-z0-9_.]*$"
if (!all(grepl(identifier_pattern, covariates))) {
  stop("covariate names must be simple R identifiers", call. = FALSE)
}

for (covariate in covariates) {
  values <- metadata_frame[[covariate]]
  numeric_values <- suppressWarnings(as.numeric(values))
  if (!anyNA(numeric_values) && all(is.finite(numeric_values))) {
    metadata_frame[[covariate]] <- numeric_values
  } else {
    if (anyNA(values) || !all(nzchar(as.character(values)))) {
      stop(sprintf("covariate %s cannot contain missing values", covariate), call. = FALSE)
    }
    metadata_frame[[covariate]] <- factor(as.character(values))
  }
}

design_terms <- c(covariates, "condition")
design <- as.formula(paste("~", paste(design_terms, collapse = " + ")))
design_string <- paste("~", paste(design_terms, collapse = " + "))
model_matrix <- model.matrix(design, data = metadata_frame)
if (qr(model_matrix)$rank < ncol(model_matrix)) {
  stop(
    "design matrix is rank deficient; remove collinear covariates or add samples",
    call. = FALSE
  )
}

col_data <- metadata_frame[, design_terms, drop = FALSE]
rownames(col_data) <- metadata_frame$sample_id

cat("DESeq2:", as.character(packageVersion("DESeq2")), "\n")
cat("Genes:", nrow(counts), "Samples:", ncol(counts), "\n")
cat("Design:", paste(deparse(design), collapse = " "), "\n")

dds <- DESeq2::DESeqDataSetFromMatrix(
  countData = counts,
  colData = col_data,
  design = design
)
dds <- DESeq2::DESeq(
  dds,
  fitType = fit_type,
  quiet = TRUE,
  parallel = FALSE
)
result <- DESeq2::results(
  dds,
  contrast = c("condition", condition_test, condition_reference),
  alpha = alpha,
  independentFiltering = TRUE,
  cooksCutoff = TRUE
)

result_data <- as.data.frame(result)
result_frame <- data.frame(
  gene_id = rownames(result_data),
  result_data,
  check.names = FALSE,
  stringsAsFactors = FALSE
)
write.table(
  result_frame,
  results_path,
  sep = "\t",
  quote = FALSE,
  row.names = FALSE
)

normalized_counts <- DESeq2::counts(dds, normalized = TRUE)
normalized_data <- as.data.frame(normalized_counts)
normalized_frame <- data.frame(
  gene_id = rownames(normalized_data),
  normalized_data,
  check.names = FALSE,
  stringsAsFactors = FALSE
)
write.table(
  normalized_frame,
  normalized_path,
  sep = "\t",
  quote = FALSE,
  row.names = FALSE
)

size_factors <- DESeq2::sizeFactors(dds)
size_factor_frame <- data.frame(
  sample_id = names(size_factors),
  size_factor = unname(size_factors),
  stringsAsFactors = FALSE
)
write.table(
  size_factor_frame,
  size_factors_path,
  sep = "\t",
  quote = FALSE,
  row.names = FALSE
)

saveRDS(dds, dataset_path)

file_checksum <- function(path) {
  unname(tools::md5sum(path))
}

padj <- result$padj
log2_fold_change <- result$log2FoldChange
tested <- !is.na(padj)
report <- list(
  schema_version = "1.0",
  analysis = list(
    node = "deseq2_de_container",
    contrast = list(
      variable = "condition",
      test = condition_test,
      reference = condition_reference
    ),
    design = design_string,
    fit_type = fit_type,
    alpha = alpha,
    independent_filtering = TRUE,
    cooks_cutoff = TRUE,
    threads = threads
  ),
  engine = list(
    package = "DESeq2",
    version = as.character(packageVersion("DESeq2")),
    bioconductor_release = Sys.getenv("BIOCONDUCTOR_RELEASE"),
    r_version = paste(R.version$major, R.version$minor, sep = ".")
  ),
  inputs = list(
    count_matrix = list(path = basename(count_path), md5 = file_checksum(count_path)),
    sample_metadata = list(path = basename(metadata_path), md5 = file_checksum(metadata_path))
  ),
  dimensions = list(genes = nrow(counts), samples = ncol(counts)),
  sample_ids = colnames(counts),
  factor_levels = lapply(col_data, levels),
  result = list(
    genes = nrow(result_frame),
    independent_filtering_retained = sum(tested),
    significant_at_alpha = sum(tested & padj < alpha),
    up_at_alpha = sum(tested & padj < alpha & log2_fold_change > 0),
    down_at_alpha = sum(tested & padj < alpha & log2_fold_change < 0)
  ),
  outputs = list(
    results = list(path = basename(results_path), md5 = file_checksum(results_path)),
    normalized_counts = list(path = basename(normalized_path), md5 = file_checksum(normalized_path)),
    size_factors = list(path = basename(size_factors_path), md5 = file_checksum(size_factors_path)),
    dataset = list(path = basename(dataset_path), md5 = file_checksum(dataset_path))
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

cat("Retained by independent filtering:", sum(tested), "\n")
cat("Significant at alpha", alpha, ":", sum(tested & padj < alpha), "\n")
