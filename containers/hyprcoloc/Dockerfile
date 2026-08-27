FROM docker.io/rocker/r-ver:4.5.1

LABEL org.opencontainers.image.title="autonomics-hyprcoloc-original" \
      org.opencontainers.image.version="0.0.2" \
      org.opencontainers.image.source="https://github.com/jrs95/hyprcoloc" \
      org.opencontainers.image.revision="0348bbdd977be731d82e4efda115a9bed63cd44f" \
      org.opencontainers.image.licenses="GPL-3.0-only"

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
      libgmp-dev \
      libmpfr-dev && \
    rm -rf /var/lib/apt/lists/*

RUN Rscript -e ' \
  snapshot <- "https://packagemanager.posit.co/cran/2025-06-15"; \
  install.packages( \
    c("iterpc", "Rcpp", "Rmpfr", "knitr", "rmarkdown", "pheatmap", "RColorBrewer"), \
    repos = snapshot \
  ); \
  install.packages( \
    "https://cran.r-project.org/src/contrib/Archive/RcppEigen/RcppEigen_0.3.3.9.3.tar.gz", \
    repos = NULL, type = "source" \
  ); \
  stopifnot(packageVersion("RcppEigen") == "0.3.3.9.3"); \
  stopifnot(packageVersion("iterpc") == "0.4.2"); \
  stopifnot(packageVersion("Rcpp") == "1.0.14"); \
  stopifnot(packageVersion("Rmpfr") == "1.1.0"); \
  stopifnot(packageVersion("knitr") == "1.50"); \
  stopifnot(packageVersion("rmarkdown") == "2.29"); \
  stopifnot(packageVersion("pheatmap") == "1.0.13"); \
  stopifnot(packageVersion("RColorBrewer") == "1.1.3"); \
'

RUN Rscript -e ' \
  url <- "https://github.com/jrs95/hyprcoloc/archive/0348bbdd977be731d82e4efda115a9bed63cd44f.tar.gz"; \
  archive <- tempfile(fileext = ".tar.gz"); \
  source_dir <- tempfile(); \
  download.file(url, archive, mode = "wb"); \
  dir.create(source_dir); \
  untar(archive, exdir = source_dir); \
  package_dir <- list.files(source_dir, full.names = TRUE, pattern = "hyprcoloc")[[1]]; \
  install.packages(package_dir, repos = NULL, type = "source"); \
  stopifnot(requireNamespace("hyprcoloc", quietly = TRUE)); \
  stopifnot(packageVersion("hyprcoloc") == "0.0.2"); \
'

WORKDIR /work

ENTRYPOINT ["Rscript"]
