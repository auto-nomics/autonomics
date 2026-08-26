FROM docker.io/rocker/r-ver:4.5.1

LABEL org.opencontainers.image.title="autonomics-coloc-original" \
  org.opencontainers.image.version="5.2.3" \
  org.opencontainers.image.source="https://github.com/chr1swallace/coloc" \
  org.opencontainers.image.license="GPL-3.0-or-later"

# Install the official `coloc` R package from the CRAN archive at a pinned
# version (Wallace, GPL-3). No reference panels, no GWAS inputs, no user data
# are baked into the image.
#
# coloc::coloc.abf / finemap.abf operate purely on GWAS summary statistics;
# no LD matrix, no reference population panel is required.
RUN Rscript -e ' \
  install.packages(c("data.table", "ggplot2", "viridis", "susieR"), repos = "https://cran.r-project.org"); \
  pkg <- "coloc"; \
  ver <- "5.2.3"; \
  url <- sprintf("https://cran.r-project.org/src/contrib/%s_%s.tar.gz", pkg, ver); \
  archive <- tempfile(fileext = ".tar.gz"); \
  download.file(url, archive, mode = "wb"); \
  install.packages(archive, repos = NULL, type = "source"); \
  unlink(archive); \
  stopifnot(requireNamespace("coloc", quietly = TRUE)); \
  stopifnot(packageVersion("coloc") == "5.2.3"); \
'

WORKDIR /work

ENTRYPOINT ["Rscript"]
