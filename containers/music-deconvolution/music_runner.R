#!/usr/bin/env Rscript
# Reference-based MuSiC deconvolution contract.
#
# Required inputs:
#   AUTONOMICS_INPUT0: gene x bulk-sample count/expression TSV
#   AUTONOMICS_INPUT1: gene x cell single-cell count TSV
#   AUTONOMICS_INPUT2: cell metadata TSV beginning with cell_id
# Optional inputs:
#   AUTONOMICS_INPUT3: cell_type/cell_size TSV
#   AUTONOMICS_INPUT4: one-column marker TSV

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

read_matrix <- function(path, label, allow_non_integer = FALSE) {
  frame <- read.delim(path, check.names = FALSE, stringsAsFactors = FALSE, na.strings = "NA")
  if (ncol(frame) < 2 || !identical(colnames(frame)[[1]], "gene_id")) {
    stop(sprintf("%s must start with gene_id followed by sample columns", label), call. = FALSE)
  }
  if (anyDuplicated(colnames(frame))) {
    stop(sprintf("%s column names must be unique", label), call. = FALSE)
  }
  gene_ids <- frame[[1]]
  if (anyNA(gene_ids) || !all(nzchar(gene_ids))) {
    stop(sprintf("%s gene_id cannot be empty", label), call. = FALSE)
  }
  if (anyDuplicated(gene_ids)) {
    stop(sprintf("%s gene_id values must be unique", label), call. = FALSE)
  }
  values <- suppressWarnings(as.matrix(frame[, -1, drop = FALSE]))
  mode(values) <- "numeric"
  rownames(values) <- gene_ids
  if (any(!is.finite(values)) || any(values < 0)) {
    stop(sprintf("%s values must be finite and nonnegative", label), call. = FALSE)
  }
  if (!allow_non_integer && any(values != floor(values))) {
    stop(sprintf("%s values must be integers", label), call. = FALSE)
  }
  values
}

wide_output <- function(matrix_value, path, id_name) {
  frame <- data.frame(
    id = rownames(matrix_value),
    matrix_value,
    check.names = FALSE,
    stringsAsFactors = FALSE
  )
  names(frame)[[1]] <- id_name
  write.table(frame, path, sep = "\t", quote = FALSE, row.names = FALSE)
}

file_checksum <- function(path) unname(tools::md5sum(path))

bulk_path <- required_env("AUTONOMICS_INPUT0")
sc_path <- required_env("AUTONOMICS_INPUT1")
metadata_path <- required_env("AUTONOMICS_INPUT2")
cell_size_path <- Sys.getenv("AUTONOMICS_INPUT3")
markers_path <- Sys.getenv("AUTONOMICS_INPUT4")

proportions_path <- Sys.getenv("AUTONOMICS_OUTPUT0")
nnls_path <- Sys.getenv("AUTONOMICS_OUTPUT1")
weights_path <- Sys.getenv("AUTONOMICS_OUTPUT2")
diagnostics_path <- Sys.getenv("AUTONOMICS_OUTPUT3")
report_path <- Sys.getenv("AUTONOMICS_OUTPUT4")

cell_type_col <- required_env("AUTONOMICS_MUSIC_CELL_TYPE_COL")
subject_col <- required_env("AUTONOMICS_MUSIC_SUBJECT_COL")
select_cell_types <- split_csv(Sys.getenv("AUTONOMICS_MUSIC_SELECT_CELL_TYPES"))
iter_max <- as.integer(Sys.getenv("AUTONOMICS_MUSIC_ITER_MAX", "1000"))
nu <- as.numeric(Sys.getenv("AUTONOMICS_MUSIC_NU", "0.0001"))
epsilon <- as.numeric(Sys.getenv("AUTONOMICS_MUSIC_EPSILON", "0.01"))
centered <- identical(toupper(Sys.getenv("AUTONOMICS_MUSIC_CENTERED", "false")), "TRUE")
normalize <- identical(toupper(Sys.getenv("AUTONOMICS_MUSIC_NORMALIZE", "false")), "TRUE")
ct_cov <- identical(toupper(Sys.getenv("AUTONOMICS_MUSIC_CT_COV", "false")), "TRUE")

if (is.na(iter_max) || iter_max < 1) {
  stop("iter_max must be a positive integer", call. = FALSE)
}
if (is.na(nu) || !is.finite(nu) || nu <= 0) {
  stop("nu must be finite and greater than zero", call. = FALSE)
}
if (is.na(epsilon) || !is.finite(epsilon) || epsilon <= 0) {
  stop("epsilon must be finite and greater than zero", call. = FALSE)
}

bulk_matrix <- read_matrix(bulk_path, "bulk matrix", allow_non_integer = TRUE)
sc_matrix <- read_matrix(sc_path, "single-cell matrix")

metadata <- read.delim(
  metadata_path,
  check.names = FALSE,
  stringsAsFactors = FALSE,
  na.strings = "NA"
)
if (ncol(metadata) < 3 || !identical(colnames(metadata)[[1]], "cell_id")) {
  stop("cell metadata must start with cell_id", call. = FALSE)
}
missing_metadata_columns <- setdiff(c(cell_type_col, subject_col), colnames(metadata))
if (length(missing_metadata_columns) > 0) {
  stop(
    sprintf("cell metadata is missing columns: %s", paste(missing_metadata_columns, collapse = ", ")),
    call. = FALSE
  )
}
cell_ids <- metadata[[1]]
if (anyNA(cell_ids) || !all(nzchar(cell_ids)) || anyDuplicated(cell_ids)) {
  stop("cell_id values must be nonempty and unique", call. = FALSE)
}
if (!setequal(cell_ids, colnames(sc_matrix))) {
  stop("single-cell matrix and metadata cell sets differ", call. = FALSE)
}
metadata <- metadata[match(colnames(sc_matrix), cell_ids), , drop = FALSE]
rownames(metadata) <- metadata$cell_id
metadata[[cell_type_col]] <- trimws(as.character(metadata[[cell_type_col]]))
metadata[[subject_col]] <- trimws(as.character(metadata[[subject_col]]))
if (any(!nzchar(metadata[[cell_type_col]])) || any(!nzchar(metadata[[subject_col]]))) {
  stop("cell type and subject IDs cannot be empty", call. = FALSE)
}

observed_cell_types <- unique(metadata[[cell_type_col]])
if (length(select_cell_types)) {
  missing_cell_types <- setdiff(select_cell_types, observed_cell_types)
  if (length(missing_cell_types) > 0) {
    stop(
      sprintf("selected cell types absent from reference: %s", paste(missing_cell_types, collapse = ", ")),
      call. = FALSE
    )
  }
} else {
  select_cell_types <- observed_cell_types
}
if (length(select_cell_types) < 2) {
  stop("MuSiC requires at least two selected cell types", call. = FALSE)
}
subject_count_per_type <- tapply(metadata[[subject_col]], metadata[[cell_type_col]], function(x) length(unique(x)))
if (any(subject_count_per_type[select_cell_types] < 2)) {
  stop("each selected cell type must appear in at least two reference subjects", call. = FALSE)
}

cell_size <- NULL
if (nzchar(cell_size_path)) {
  cell_size <- read.delim(
    cell_size_path,
    check.names = FALSE,
    stringsAsFactors = FALSE,
    na.strings = "NA"
  )
  if (ncol(cell_size) < 2 || !identical(colnames(cell_size)[[1]], "cell_type") || !identical(colnames(cell_size)[[2]], "cell_size")) {
    stop("cell size table must start with cell_type and cell_size", call. = FALSE)
  }
  cell_size[[2]] <- as.numeric(cell_size[[2]])
  if (anyNA(cell_size[[2]]) || any(!is.finite(cell_size[[2]])) || any(cell_size[[2]] <= 0)) {
    stop("cell_size values must be finite and greater than zero", call. = FALSE)
  }
  missing_sizes <- setdiff(select_cell_types, cell_size[[1]])
  if (length(missing_sizes) > 0) {
    stop(
      sprintf("cell size table is missing types: %s", paste(missing_sizes, collapse = ", ")),
      call. = FALSE
    )
  }
}

markers <- NULL
if (nzchar(markers_path)) {
  marker_frame <- read.delim(
    markers_path,
    check.names = FALSE,
    stringsAsFactors = FALSE,
    na.strings = "NA"
  )
  if (ncol(marker_frame) < 1) {
    stop("marker table has no columns", call. = FALSE)
  }
  marker_column <- if ("gene_id" %in% colnames(marker_frame)) "gene_id" else colnames(marker_frame)[[1]]
  markers <- unique(trimws(as.character(marker_frame[[marker_column]])))
  markers <- markers[nzchar(markers)]
  if (!length(markers)) {
    stop("marker table contains no gene IDs", call. = FALSE)
  }
}

common_input_genes <- intersect(rownames(bulk_matrix), rownames(sc_matrix))
if (length(common_input_genes) < 10) {
  stop("bulk and single-cell matrices have fewer than 10 common genes", call. = FALSE)
}

# MuSiC 1.0.0 calls counts() without a namespace, so the SingleCellExperiment
# generics must be attached before music_prop() executes.
suppressPackageStartupMessages(library(SingleCellExperiment))
col_data <- S4Vectors::DataFrame(
  cell_id = metadata$cell_id,
  cell_type = metadata[[cell_type_col]],
  subject_id = metadata[[subject_col]],
  row.names = metadata$cell_id
)
names(col_data)[[2]] <- cell_type_col
names(col_data)[[3]] <- subject_col
sce <- SingleCellExperiment::SingleCellExperiment(
  assays = list(counts = Matrix::Matrix(sc_matrix, sparse = TRUE)),
  colData = col_data
)

cat("MuSiC:", as.character(packageVersion("MuSiC")), "\n")
cat("Bulk:", nrow(bulk_matrix), "genes x", ncol(bulk_matrix), "samples\n")
cat("Reference:", nrow(sc_matrix), "genes x", ncol(sc_matrix), "cells\n")
cat("Cell types:", paste(select_cell_types, collapse = ", "), "\n")

result <- MuSiC::music_prop(
  bulk.mtx = bulk_matrix,
  sc.sce = sce,
  markers = markers,
  clusters = cell_type_col,
  samples = subject_col,
  select.ct = select_cell_types,
  cell_size = cell_size,
  ct.cov = ct_cov,
  verbose = TRUE,
  iter.max = iter_max,
  nu = nu,
  eps = epsilon,
  centered = centered,
  normalize = normalize
)

weighted <- as.matrix(result$Est.prop.weighted)
nnls <- as.matrix(result$Est.prop.allgene)
weights <- as.matrix(result$Weight.gene)
variance <- as.matrix(result$Var.prop)
r_squared <- as.numeric(result$r.squared.full)
names(r_squared) <- rownames(weighted)

wide_output(weighted, proportions_path, "sample_id")
wide_output(nnls, nnls_path, "sample_id")
wide_output(weights, weights_path, "gene_id")

colnames(variance) <- paste0("variance_", colnames(variance))
diagnostics <- data.frame(
  sample_id = rownames(weighted),
  r_squared = r_squared,
  variance,
  check.names = FALSE,
  stringsAsFactors = FALSE
)
write.table(diagnostics, diagnostics_path, sep = "\t", quote = FALSE, row.names = FALSE)

report <- list(
  schema_version = "1.0",
  node = "music_deconvolution_container",
  analysis = list(
    method = "MuSiC::music_prop",
    cell_type_col = cell_type_col,
    subject_col = subject_col,
    selected_cell_types = select_cell_types,
    iter_max = iter_max,
    nu = nu,
    epsilon = epsilon,
    centered = centered,
    normalize = normalize,
    ct_cov = ct_cov,
    markers_provided = !is.null(markers),
    marker_count = if (is.null(markers)) 0L else length(markers),
    cell_sizes_provided = !is.null(cell_size)
  ),
  engine = list(
    package = "MuSiC",
    version = as.character(packageVersion("MuSiC")),
    commit = "f21fe67f5670d5e9fca0ad7550abaae3423eb59c",
    r_version = paste(R.version$major, R.version$minor, sep = ".")
  ),
  inputs = list(
    bulk_matrix = list(path = basename(bulk_path), md5 = file_checksum(bulk_path)),
    single_cell_matrix = list(path = basename(sc_path), md5 = file_checksum(sc_path)),
    cell_metadata = list(path = basename(metadata_path), md5 = file_checksum(metadata_path)),
    cell_sizes = if (nzchar(cell_size_path)) list(path = basename(cell_size_path), md5 = file_checksum(cell_size_path)) else NULL,
    markers = if (nzchar(markers_path)) list(path = basename(markers_path), md5 = file_checksum(markers_path)) else NULL
  ),
  dimensions = list(
    bulk_genes = nrow(bulk_matrix),
    bulk_samples = ncol(bulk_matrix),
    reference_genes = nrow(sc_matrix),
    reference_cells = ncol(sc_matrix),
    reference_subjects = length(unique(metadata[[subject_col]])),
    selected_cell_types = length(select_cell_types),
    common_input_genes = length(common_input_genes)
  ),
  output_qc = list(
    proportion_row_sums = rowSums(weighted),
    nnls_row_sums = rowSums(nnls),
    weighted_gene_count = nrow(weights)
  ),
  outputs = list(
    proportions = list(path = basename(proportions_path), md5 = file_checksum(proportions_path)),
    nnls_proportions = list(path = basename(nnls_path), md5 = file_checksum(nnls_path)),
    gene_weights = list(path = basename(weights_path), md5 = file_checksum(weights_path)),
    diagnostics = list(path = basename(diagnostics_path), md5 = file_checksum(diagnostics_path))
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

cat("MuSiC deconvolution complete: ", ncol(bulk_matrix), " bulk samples\n", sep = "")
