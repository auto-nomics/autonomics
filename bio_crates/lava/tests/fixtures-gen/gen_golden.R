#!/usr/bin/env Rscript
# Golden-output generator for the Rust LAVA port.
# Runs the original R LAVA on the vignette data and dumps deterministic
# intermediates + analysis tables to bio_crates/lava/tests/fixtures/golden/.
#
# Run from the repo root, in the r45 conda env:
#   conda activate r45
#   Rscript bio_crates/lava/tests/fixtures-gen/gen_golden.R

suppressPackageStartupMessages({ library(LAVA); library(data.table) })
set.seed(1)

repo <- normalizePath(".", winslash = "/")
data_dir <- file.path(repo, "reference/LAVA")
out_dir <- file.path(repo, "bio_crates/lava/tests/fixtures/golden")
dir.create(out_dir, showWarnings = FALSE, recursive = TRUE)

info_file <- file.path(data_dir, "vignettes/data/input.info.txt")
overlap_file <- file.path(data_dir, "vignettes/data/sample.overlap.txt")
ref_prefix <- file.path(data_dir, "vignettes/data/g1000_test")
loci_file <- file.path(data_dir, "vignettes/data/test.loci")

cat("Processing input...\n")
input <- process.input(info_file, overlap_file, ref_prefix, phenos = NULL, input.dir = data_dir)
loci <- read.loci(loci_file)

phenos7 <- input$info$phenotype

dump_locus <- function(loc, tag) {
    P <- length(loc$phenos)
    # deterministic intermediate: K, n.snps, N, sigma/omega diag, h2, univ p
    up <- run.univ(loc)$p
    rows <- data.frame(
        locus = loc$id, chr = loc$chr, start = loc$start, stop = loc$stop,
        n.snps = loc$n.snps, K = loc$K,
        phen = loc$phenos, binary = loc$binary,
        N = loc$N, sigma_diag = diag(as.matrix(loc$sigma)),
        omega_diag = diag(as.matrix(loc$omega)),
        h2.obs = loc$h2.obs, h2.latent = loc$h2.latent,
        univ_p = up, stringsAsFactors = FALSE
    )
    fn <- file.path(out_dir, sprintf("locus_%s_intermediate.tsv", tag))
    write.table(rows, fn, sep = "\t", row.names = FALSE, quote = FALSE)
    # omega and sigma matrices (JSON-ish TSV)
    write.table(as.matrix(loc$omega), file.path(out_dir, sprintf("locus_%s_omega.tsv", tag)),
                sep = "\t", row.names = FALSE, col.names = FALSE, quote = FALSE)
    write.table(as.matrix(loc$sigma), file.path(out_dir, sprintf("locus_%s_sigma.tsv", tag)),
                sep = "\t", row.names = FALSE, col.names = FALSE, quote = FALSE)
    writeLines(loc$phenos, file.path(out_dir, sprintf("locus_%s_phenos.txt", tag)))
}

# ---- locus 266 (index 3): univariate + bivariate, phenos depression/neuro/bmi ----
cat("Locus 266...\n")
l266 <- loci[loci$LOC == 266, ]
loc266 <- process.locus(l266, input, phenos = c("depression", "neuro", "bmi"))
dump_locus(loc266, "266")
u266 <- run.univ(loc266)
write.table(u266, file.path(out_dir, "locus_266_univ.tsv"), sep = "\t", row.names = FALSE, quote = FALSE)
b266 <- run.bivar(loc266)
write.table(b266, file.path(out_dir, "locus_266_bivar.tsv"), sep = "\t", row.names = FALSE, quote = FALSE)

# ---- locus 964: multiple regression + partial correlation ----
cat("Locus 964...\n")
l964 <- loci[loci$LOC == 964, ]
loc964 <- process.locus(l964, input, phenos = c("hypothyroidism", "asthma", "rheuma", "diabetes"))
dump_locus(loc964, "964")
mr964 <- run.multireg(loc964, target = "hypothyroidism", only.full.model = TRUE)
# run.multireg returns list[[model]][[df]]; only.full.model -> length-1 outer, one df inside
mr_rows <- mr964[[1]][[1]]
write.table(mr_rows, file.path(out_dir, "locus_964_multireg.tsv"), sep = "\t", row.names = FALSE, quote = FALSE)
pc964 <- run.pcor(loc964, target = c("hypothyroidism", "diabetes"), phenos = "asthma")
write.table(pc964, file.path(out_dir, "locus_964_pcor.tsv"), sep = "\t", row.names = FALSE, quote = FALSE)

# ---- all-loci deterministic sweep (7 phenotypes) ----
cat("All-loci sweep...\n")
all_rows <- list()
for (i in seq_len(nrow(loci))) {
    loc <- tryCatch(process.locus(loci[i, ], input, phenos = phenos7),
                    error = function(e) NULL)
    if (is.null(loc)) next
    up <- run.univ(loc)$p
    all_rows[[length(all_rows) + 1]] <- data.frame(
        locus = loc$id, n.snps = loc$n.snps, K = loc$K,
        phen = loc$phenos, binary = loc$binary, N = loc$N,
        sigma_diag = diag(as.matrix(loc$sigma)),
        omega_diag = diag(as.matrix(loc$omega)),
        h2.obs = loc$h2.obs, h2.latent = loc$h2.latent,
        univ_p = up, stringsAsFactors = FALSE
    )
}
all_df <- do.call(rbind, all_rows)
write.table(all_df, file.path(out_dir, "all_loci_deterministic.tsv"),
            sep = "\t", row.names = FALSE, quote = FALSE)

cat("Done. Golden files in", out_dir, "\n")
