#!/usr/bin/env Rscript

suppressPackageStartupMessages(library(jsonlite))
suppressPackageStartupMessages(library(maftools))

fail <- function(message) {
  stop(paste0("mutation-analysis: ", message), call. = FALSE)
}

env_value <- function(name) {
  value <- Sys.getenv(name, unset = "")
  if (!nzchar(value)) fail(paste0(name, " is required"))
  value
}

read_table <- function(path) {
  if (!file.exists(path)) fail(paste0("input does not exist: ", path))
  connection <- if (endsWith(path, ".gz")) gzfile(path, "rt") else file(path, "rt")
  on.exit(close(connection), add = TRUE)
  read.delim(connection, check.names = FALSE, stringsAsFactors = FALSE,
             quote = "", comment.char = "")
}

required_character <- function(config, name) {
  value <- config[[name]]
  if (!is.character(value) || length(value) != 1 || !nzchar(value)) {
    fail(paste0("configuration value must be a nonempty string: ", name))
  }
  value
}

canonical_maf <- function(raw, config) {
  mappings <- c(
    Hugo_Symbol = required_character(config, "gene_col"),
    Chromosome = required_character(config, "chromosome_col"),
    Start_Position = required_character(config, "start_position_col"),
    End_Position = required_character(config, "end_position_col"),
    Reference_Allele = required_character(config, "reference_allele_col"),
    Tumor_Seq_Allele2 = required_character(config, "tumor_seq_allele_col"),
    Variant_Classification = required_character(config, "variant_col"),
    Variant_Type = required_character(config, "variant_type_col"),
    Tumor_Sample_Barcode = required_character(config, "tumor_sample_col")
  )
  missing <- mappings[!mappings %in% colnames(raw)]
  if (length(missing) > 0) {
    fail(paste0("MAF is missing mapped columns: ",
                paste(mappings[names(missing)], collapse = ", ")))
  }
  canonical <- raw[, unname(mappings), drop = FALSE]
  colnames(canonical) <- names(mappings)
  numeric_columns <- c("Start_Position", "End_Position")
  for (column in numeric_columns) {
    canonical[[column]] <- suppressWarnings(as.numeric(canonical[[column]]))
    if (anyNA(canonical[[column]])) fail(paste0(column, " must be numeric"))
  }
  canonical$Tumor_Sample_Barcode <- as.character(canonical$Tumor_Sample_Barcode)
  if (anyNA(canonical$Tumor_Sample_Barcode) ||
      any(!nzchar(canonical$Tumor_Sample_Barcode))) {
    fail("Tumor_Sample_Barcode must be nonmissing and nonempty")
  }
  canonical
}

clinical_for_groups <- function(config, samples) {
  path <- Sys.getenv("AUTONOMICS_INPUT1", unset = "")
  if (!nzchar(path)) fail("tmb groups require a clinical input")
  clinical <- read_table(path)
  group_column <- required_character(config, "tmb_group_col")
  if (!group_column %in% colnames(clinical)) {
    fail(paste0("clinical file is missing group column ", group_column))
  }
  id_column <- if ("Tumor_Sample_Barcode" %in% colnames(clinical)) {
    "Tumor_Sample_Barcode"
  } else if ("sample_id" %in% colnames(clinical)) {
    "sample_id"
  } else {
    colnames(clinical)[[1]]
  }
  clinical$mutation_sample_id <- as.character(clinical[[id_column]])
  clinical$group <- as.character(clinical[[group_column]])
  expected_groups <- as.character(config$tmb_groups)
  if (length(expected_groups) != 2 || anyDuplicated(expected_groups) > 0 ||
      any(!nzchar(expected_groups))) {
    fail("tmb_groups must contain two distinct nonempty labels")
  }
  selected <- clinical[clinical$group %in% expected_groups,
                       c("mutation_sample_id", "group"), drop = FALSE]
  selected <- unique(selected[!is.na(selected$mutation_sample_id) &
                                nzchar(selected$mutation_sample_id), , drop = FALSE])
  if (anyDuplicated(selected$mutation_sample_id) > 0) {
    fail("clinical sample IDs must be unique after group filtering")
  }
  if (!all(selected$mutation_sample_id %in% samples)) {
    fail("clinical sample IDs must match MAF Tumor_Sample_Barcode values")
  }
  selected
}

count_table <- function(values) {
  counts <- as.data.frame(table(values), stringsAsFactors = FALSE)
  colnames(counts) <- c("category", "count")
  counts$count <- as.integer(counts$count)
  counts[order(-counts$count, counts$category), , drop = FALSE]
}

tmb_result <- function(variants, config) {
  sample_counts <- count_table(variants$Tumor_Sample_Barcode)
  colnames(sample_counts) <- c("sample_id", "mutations")
  panel_size_mb <- as.numeric(config$panel_size_mb)
  sample_counts$panel_size_mb <- panel_size_mb
  sample_counts$tmb_per_mb <- sample_counts$mutations / panel_size_mb
  expected_groups <- as.character(config$tmb_groups)
  comparison <- "not_performed"
  p_value <- NA_real_

  if (length(expected_groups) > 0) {
    clinical <- clinical_for_groups(config, sample_counts$sample_id)
    merged <- merge(sample_counts, clinical, by.x = "sample_id",
                    by.y = "mutation_sample_id", all.x = TRUE, sort = FALSE)
    sample_counts <- merged[match(sample_counts$sample_id, merged$sample_id), , drop = FALSE]
    groups_present <- unique(sample_counts$group[!is.na(sample_counts$group)])
    if (length(groups_present) == 2) {
      first <- sample_counts$tmb_per_mb[sample_counts$group == expected_groups[[1]]]
      second <- sample_counts$tmb_per_mb[sample_counts$group == expected_groups[[2]]]
      test_result <- suppressWarnings(wilcox.test(first, second,
                                                  exact = FALSE, correct = FALSE))
      comparison <- paste0("wilcox:", expected_groups[[1]], "_vs_",
                           expected_groups[[2]])
      p_value <- as.numeric(test_result$p.value)
    }
  }

  sample_counts$tmb_group_comparison <- comparison
  sample_counts$wilcox_p_value <- p_value
  sample_counts
}

summary_result <- function(variants) {
  total <- nrow(variants)
  classifications <- count_table(variants$Variant_Classification)
  types <- count_table(variants$Variant_Type)
  snps <- variants[variants$Variant_Type == "SNP", , drop = FALSE]
  snv_classes <- count_table(snps$Variant_Classification)
  long <- function(values, label) {
    if (nrow(values) == 0) values <- data.frame(category = character(), count = integer())
    data.frame(metric = label, category = values$category, count = values$count,
               fraction = values$count / total, stringsAsFactors = FALSE)
  }
  rbind(long(classifications, "variant_classification"),
        long(types, "variant_type"), long(snv_classes, "snv_class"),
        stringsAsFactors = FALSE)
}

top_genes_result <- function(variants, config) {
  by_gene <- split(variants$Tumor_Sample_Barcode, variants$Hugo_Symbol)
  result <- data.frame(
    gene_id = names(by_gene),
    mutation_count = as.integer(vapply(by_gene, length, numeric(1))),
    sample_count = as.integer(vapply(by_gene, function(values) {
      length(unique(values))
    }, numeric(1))),
    stringsAsFactors = FALSE
  )
  result <- result[order(-result$mutation_count, -result$sample_count,
                         result$gene_id), , drop = FALSE]
  result$rank <- seq_len(nrow(result))
  head(result, max(1L, as.integer(config$top_n)))
}

is_transition <- function(reference, alternate) {
  transitions <- list(A = "G", G = "A", C = "T", T = "C")
  transition <- unname(transitions[reference])
  !is.na(transition) && transition == alternate
}

titv_result <- function(variants) {
  snps <- variants[toupper(variants$Variant_Type) == "SNP", , drop = FALSE]
  if (nrow(snps) == 0) {
    return(data.frame(sample_id = character(), transitions = integer(),
                      transversions = integer(), ti_tv_ratio = numeric(),
                      stringsAsFactors = FALSE))
  }
  reference <- toupper(snps$Reference_Allele)
  alternate <- toupper(snps$Tumor_Seq_Allele2)
  valid <- nchar(reference) == 1 & nchar(alternate) == 1 &
    reference %in% c("A", "C", "G", "T") & alternate %in% c("A", "C", "G", "T") &
    reference != alternate
  snps <- snps[valid, , drop = FALSE]
  classification <- ifelse(mapply(is_transition, toupper(snps$Reference_Allele),
                                  toupper(snps$Tumor_Seq_Allele2)),
                           "transition", "transversion")
  rows <- lapply(split(classification, snps$Tumor_Sample_Barcode), function(values) {
    transitions <- sum(values == "transition")
    transversions <- sum(values == "transversion")
    data.frame(sample_id = "", transitions = transitions,
               transversions = transversions,
               ti_tv_ratio = if (transversions == 0) NA_real_
                             else transitions / transversions,
               stringsAsFactors = FALSE)
  })
  result <- do.call(rbind, rows)
  result$sample_id <- names(rows)
  result[order(result$sample_id), , drop = FALSE]
  rownames(result) <- NULL
  result
}

mutex_result <- function(variants, config) {
  genes <- unique(as.character(top_genes_result(variants, config)$gene_id))
  if (length(genes) < 2) {
    return(list(table = data.frame(gene_1 = character(), gene_2 = character(),
                                   samples_gene_1 = integer(), samples_gene_2 = integer(),
                                   cooccurring = integer(), neither = integer(),
                                   p_value = numeric(), relationship = character(),
                                   stringsAsFactors = FALSE),
                genes = character()))
  }
  samples <- unique(variants$Tumor_Sample_Barcode)
  altered <- lapply(genes, function(gene) {
    unique(variants$Tumor_Sample_Barcode[variants$Hugo_Symbol == gene])
  })
  names(altered) <- genes
  pairs <- list()
  for (i in seq_along(genes)) {
    for (j in seq_len(max(0L, length(genes) - i)) + i) {
      first <- altered[[i]]
      second <- altered[[j]]
      both <- length(intersect(first, second))
      first_only <- length(setdiff(first, second))
      second_only <- length(setdiff(second, first))
      neither <- length(samples) - both - first_only - second_only
      contingency <- matrix(c(both, first_only, second_only, neither), nrow = 2)
      test_result <- suppressWarnings(fisher.test(contingency))
      odds_ratio <- unname(test_result$estimate)
      relationship <- if (!is.finite(odds_ratio) || is.na(odds_ratio) || odds_ratio == 1) {
        "neutral"
      } else if (odds_ratio > 1) {
        "co_occurrence"
      } else {
        "mutual_exclusivity"
      }
      pairs[[length(pairs) + 1L]] <- data.frame(
        gene_1 = genes[[i]], gene_2 = genes[[j]],
        samples_gene_1 = length(first), samples_gene_2 = length(second),
        cooccurring = both, neither = neither,
        p_value = as.numeric(test_result$p.value), relationship = relationship,
        stringsAsFactors = FALSE
      )
    }
  }
  list(table = do.call(rbind, pairs), genes = genes)
}

main <- function() {
  config_file <- env_value("MUTATION_CONFIG")
  config <- jsonlite::fromJSON(config_file, simplifyVector = TRUE)
  raw <- read_table(env_value("AUTONOMICS_INPUT0"))
  canonical <- canonical_maf(raw, config)
  maf_object <- maftools::read.maf(
    maf = canonical, useAll = TRUE, verbose = FALSE
  )
  variants <- maf_object@data
  operation <- required_character(config, "operation")

  report <- switch(operation,
    tmb = tmb_result(variants, config),
    summary = summary_result(variants),
    top_genes = top_genes_result(variants, config),
    titv = titv_result(variants),
    mutex = mutex_result(variants, config)$table,
    fail(paste0("unsupported operation: ", operation))
  )

  details <- list(
    schema_version = "1.0",
    operation = operation,
    sample_count = length(unique(variants$Tumor_Sample_Barcode)),
    variant_count = nrow(variants),
    gene_count = length(unique(variants$Hugo_Symbol))
  )
  if (operation == "tmb") {
    details$panel_size_mb <- as.numeric(config$panel_size_mb)
    details$group_comparison <- if (nrow(report) > 0) report$tmb_group_comparison[[1]] else "not_performed"
    details$wilcox_p_value <- if (nrow(report) > 0) report$wilcox_p_value[[1]] else NA_real_
  } else if (operation == "top_genes") {
    details$top_n <- as.integer(config$top_n)
  } else if (operation == "summary") {
    details$frequencies <- report
  } else if (operation == "titv") {
    details$sample_statistics <- report
    details$total_transitions <- sum(report$transitions)
    details$total_transversions <- sum(report$transversions)
  } else if (operation == "mutex") {
    matrix_result <- mutex_result(variants, config)
    details$genes <- matrix_result$genes
    details$pairs <- matrix_result$table
  }

  write.table(report, env_value("AUTONOMICS_OUTPUT0"), sep = "\t", quote = FALSE,
              row.names = FALSE, na = "NA")
  jsonlite::write_json(details, env_value("AUTONOMICS_OUTPUT1"),
                       auto_unbox = TRUE, pretty = TRUE, na = "null", null = "null")
}

main()
