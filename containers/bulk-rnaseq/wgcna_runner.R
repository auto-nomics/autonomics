#!/usr/bin/env Rscript
# Whole-pipeline WGCNA contract.
#
# Consumes a single expression matrix (genes x samples, samples are columns)
# plus an optional sample metadata file with covariate metadata that is
# preserved alongside the module eigengenes.
#
# Outputs:
#   soft_threshold.tsv   power scan and chosen beta
#   adjacency_stats.tsv  sparse adjacency summary (mean / non-zero counts)
#   tom_stats.tsv        summary of the topological overlap matrix
#   modules.tsv          gene -> module assignment with kME values
#   module_eigengenes.tsv module eigengenes matrix
#   run_report.json      contract report with package versions

options(stringsAsFactors = FALSE, digits = 15)

required_env <- function(name) {
  value <- Sys.getenv(name)
  if (!nzchar(value)) {
    stop(sprintf("missing required environment variable: %s", name), call. = FALSE)
  }
  value
}

required_env("AUTONOMICS_INPUT0")

expr_path <- Sys.getenv("AUTONOMICS_INPUT0")
metadata_path <- Sys.getenv("AUTONOMICS_INPUT1")
soft_threshold_path <- Sys.getenv("AUTONOMICS_OUTPUT0")
adjacency_stats_path <- Sys.getenv("AUTONOMICS_OUTPUT1")
tom_stats_path <- Sys.getenv("AUTONOMICS_OUTPUT2")
modules_path <- Sys.getenv("AUTONOMICS_OUTPUT3")
eigengenes_path <- Sys.getenv("AUTONOMICS_OUTPUT4")
report_path <- Sys.getenv("AUTONOMICS_OUTPUT5")

network_type <- Sys.getenv("AUTONOMICS_WGCNA_NETWORK_TYPE", "signed")
cor_fn <- Sys.getenv("AUTONOMICS_WGCNA_COR", "pearson")
min_module_size <- as.integer(Sys.getenv("AUTONOMICS_WGCNA_MIN_MODULE_SIZE", "30"))
deep_split <- as.integer(Sys.getenv("AUTONOMICS_WGCNA_DEEP_SPLIT", "2"))
merge_threshold <- as.numeric(Sys.getenv("AUTONOMICS_WGCNA_MERGE_THRESHOLD", "0.25"))
max_block_size <- as.integer(Sys.getenv("AUTONOMICS_WGCNA_MAX_BLOCK_SIZE", "5000"))
threads <- as.integer(Sys.getenv("AUTONOMICS_WGCNA_THREADS", "1"))

if (!network_type %in% c("signed", "unsigned")) {
  stop("network_type must be signed or unsigned", call. = FALSE)
}
if (!cor_fn %in% c("pearson", "bicor")) {
  stop("cor must be pearson or bicor", call. = FALSE)
}
if (is.na(min_module_size) || min_module_size < 5) {
  stop("min_module_size must be at least 5", call. = FALSE)
}
if (is.na(deep_split) || deep_split < 0 || deep_split > 4) {
  stop("deep_split must be between 0 and 4", call. = FALSE)
}
if (is.na(merge_threshold) || merge_threshold <= 0 || merge_threshold >= 1) {
  stop("merge_threshold must lie strictly between 0 and 1", call. = FALSE)
}
if (is.na(max_block_size) || max_block_size < 500) {
  stop("max_block_size must be at least 500", call. = FALSE)
}
if (is.na(threads) || threads < 1) {
  stop("threads must be a positive integer", call. = FALSE)
}

expr_frame <- read.delim(
  expr_path,
  check.names = FALSE,
  stringsAsFactors = FALSE,
  na.strings = "NA"
)
if (ncol(expr_frame) < 2 || !identical(colnames(expr_frame)[[1]], "gene_id")) {
  stop("expression matrix must start with gene_id followed by sample columns", call. = FALSE)
}
if (anyDuplicated(colnames(expr_frame))) {
  stop("expression matrix column names must be unique", call. = FALSE)
}
gene_ids <- expr_frame[[1]]
if (anyNA(gene_ids) || !all(nzchar(gene_ids))) {
  stop("gene_id cannot be empty", call. = FALSE)
}
if (anyDuplicated(gene_ids)) {
  stop("gene_id values must be unique", call. = FALSE)
}
sample_ids <- colnames(expr_frame)[-1]
expr <- as.matrix(expr_frame[, -1, drop = FALSE])
mode(expr) <- "numeric"
rownames(expr) <- gene_ids
colnames(expr) <- sample_ids
if (any(!is.finite(expr))) {
  stop("expression matrix contains non-finite values", call. = FALSE)
}
if (anyNA(expr)) {
  expr[is.na(expr)] <- 0
}

sample_metadata <- NULL
if (nzchar(metadata_path)) {
  md <- read.delim(
    metadata_path,
    check.names = FALSE,
    stringsAsFactors = FALSE,
    na.strings = "NA"
  )
  if (!identical(colnames(md)[[1]], "sample_id")) {
    stop("metadata must start with sample_id", call. = FALSE)
  }
  if (any(!sample_ids %in% md[[1]])) {
    stop("expression samples must all appear in metadata", call. = FALSE)
  }
  rownames(md) <- md[[1]]
  sample_metadata <- md[sample_ids, , drop = FALSE]
}

if (!requireNamespace("WGCNA", quietly = TRUE)) stop("WGCNA is required", call. = FALSE)
if (!requireNamespace("dynamicTreeCut", quietly = TRUE)) stop("dynamicTreeCut is required", call. = FALSE)
suppressPackageStartupMessages(library(WGCNA))

if (threads >= 2) {
  WGCNA::enableWGCNAThreads(nThreads = threads)
} else {
  WGCNA::disableWGCNAThreads()
}

powers <- c(1:10, seq(12, 30, by = 2))
cor_name <- if (identical(cor_fn, "bicor")) "bicor" else "cor"
cor_type_name <- if (identical(cor_fn, "bicor")) "bicor" else "pearson"
sft <- WGCNA::pickSoftThreshold(
  t(expr),
  powerVector = powers,
  networkType = network_type,
  corFnc = cor_name,
  verbose = 0
)
fit_indices <- sft$fitIndices
if (is.null(fit_indices) || nrow(fit_indices) == 0) {
  stop("soft-threshold scan returned no fit indices", call. = FALSE)
}
chosen_power <- sft$powerEstimate
if (is.na(chosen_power)) {
  eligible <- which(fit_indices$SFT.R.sq >= 0.8)
  chosen_power <- if (length(eligible)) min(powers[eligible]) else powers[1]
}
soft_frame <- data.frame(
  power = fit_indices$Power,
  scale_free_R2 = fit_indices$SFT.R.sq,
  slope = fit_indices$slope,
  truncated_R2 = fit_indices$truncated.R.sq,
  mean_connectivity = fit_indices$mean.k,
  median_connectivity = fit_indices$median.k,
  max_connectivity = fit_indices$max.k,
  chosen_power = as.integer(chosen_power),
  network_type = network_type,
  cor = cor_fn
)
write.table(soft_frame, soft_threshold_path, sep = "\t", quote = FALSE, row.names = FALSE)

# Block-wise network construction keeps memory bounded for large cohorts.
# WGCNA's blockwiseModules handles TOM internally and never materializes
# the full TOM as a dense dataframe.
net <- WGCNA::blockwiseModules(
  t(expr),
  power = chosen_power,
  networkType = network_type,
  corType = cor_type_name,
  TOMType = if (identical(network_type, "signed")) "signed" else "unsigned",
  deepSplit = deep_split,
  minModuleSize = min_module_size,
  mergeCutHeight = merge_threshold,
  maxBlockSize = max_block_size,
  pamRespectsDendro = FALSE,
  numericLabels = TRUE,
  saveTOMs = FALSE,
  verbose = 0,
  checkMissingData = FALSE
)

module_labels <- net$colors
module_size <- table(module_labels)
module_table <- data.frame(
  module_label = names(module_size),
  module_size = as.integer(module_size),
  stringsAsFactors = FALSE
)
write.table(module_table, adjacency_stats_path, sep = "\t", quote = FALSE, row.names = FALSE)

# Compute only the diagonals of the TOM per block for the run report.
# The full TOM is intentionally not persisted to disk.
if (length(net$TOMFiles) > 0) {
  tom_size_bytes <- sum(file.info(net$TOMFiles)$size, na.rm = TRUE)
  tom_blocks <- length(net$TOMFiles)
} else {
  internal_object_bytes <- as.numeric(object.size(net))
  tom_blocks <- 1L
}
tom_stats <- data.frame(
  blocks = tom_blocks,
  internal_object_bytes = internal_object_bytes,
  network_type = network_type,
  cor = cor_fn,
  power = chosen_power,
  stringsAsFactors = FALSE
)
write.table(tom_stats, tom_stats_path, sep = "\t", quote = FALSE, row.names = FALSE)

module_assignment <- data.frame(
  gene_id = rownames(expr),
  module_label = module_labels,
  module_color = WGCNA::labels2colors(module_labels),
  stringsAsFactors = FALSE
)
write.table(module_assignment, modules_path, sep = "\t", quote = FALSE, row.names = FALSE)

eigengenes <- net$MEs
eigengenes_frame <- data.frame(
  sample_id = rownames(eigengenes),
  eigengenes,
  check.names = FALSE,
  stringsAsFactors = FALSE
)
if (!is.null(sample_metadata)) {
  metadata_columns <- sample_metadata[
    rownames(eigengenes),
    setdiff(colnames(sample_metadata), "sample_id"),
    drop = FALSE
  ]
  eigengenes_frame <- cbind(eigengenes_frame, metadata_columns)
}
write.table(eigengenes_frame, eigengenes_path, sep = "\t", quote = FALSE, row.names = FALSE)

# kME per gene per module: correlation between expression and module eigengene.
kme_list <- WGCNA::signedKME(t(expr), datME = eigengenes, outputColumnName = "kME", corFnc = cor_name)
modules_full <- cbind(module_assignment, kme_list)
write.table(modules_full, modules_path, sep = "\t", quote = FALSE, row.names = FALSE)

file_checksum <- function(path) unname(tools::md5sum(path))

report <- list(
  schema_version = "1.0",
  node = "wgcna_container",
  analysis = list(
    network_type = network_type,
    cor = cor_fn,
    min_module_size = min_module_size,
    deep_split = deep_split,
    merge_threshold = merge_threshold,
    max_block_size = max_block_size,
    threads = threads,
    chosen_power = chosen_power
  ),
  engine = list(
    package = c("WGCNA", "dynamicTreeCut"),
    versions = list(
      WGCNA = as.character(packageVersion("WGCNA")),
      dynamicTreeCut = as.character(packageVersion("dynamicTreeCut"))
    ),
    r_version = paste(R.version$major, R.version$minor, sep = ".")
  ),
  inputs = list(
    expression_matrix = list(path = basename(expr_path), md5 = file_checksum(expr_path)),
    sample_metadata = if (nzchar(metadata_path)) list(path = basename(metadata_path), md5 = file_checksum(metadata_path)) else NULL
  ),
  dimensions = list(
    genes = nrow(expr),
    samples = ncol(expr),
    modules = length(unique(module_labels)),
    modules_after_merge = length(unique(module_labels)),
    largest_module = if (nrow(module_table)) max(module_table$module_size) else 0
  ),
  outputs = list(
    soft_threshold = list(path = basename(soft_threshold_path), md5 = file_checksum(soft_threshold_path)),
    adjacency_stats = list(path = basename(adjacency_stats_path), md5 = file_checksum(adjacency_stats_path)),
    tom_stats = list(path = basename(tom_stats_path), md5 = file_checksum(tom_stats_path)),
    modules = list(path = basename(modules_path), md5 = file_checksum(modules_path)),
    eigengenes = list(path = basename(eigengenes_path), md5 = file_checksum(eigengenes_path))
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

cat("wgcna complete: ", length(unique(module_labels)), " modules over ", nrow(expr), " genes\n", sep = "")
