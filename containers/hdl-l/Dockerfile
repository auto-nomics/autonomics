FROM docker.io/rocker/r-ver:4.5.1

LABEL org.opencontainers.image.title="autonomics-hdl-original" \
      org.opencontainers.image.version="1.4.3" \
      org.opencontainers.image.source="https://github.com/zhenin/HDL" \
      org.opencontainers.image.revision="e6b055d42fd9904c3e994a9b829626c7e5f8422b" \
      org.opencontainers.image.licenses="GPL-3.0-or-later"

RUN apt-get update && \
    apt-get install -y --no-install-recommends zlib1g-dev && \
    rm -rf /var/lib/apt/lists/*

RUN Rscript -e ' \
  options(timeout = 600); \
  install.packages( \
    c("data.table", "dplyr"), \
    repos = "https://packagemanager.posit.co/cran/2026-08-15", \
    Ncpus = 2 \
  ); \
  stopifnot(packageVersion("data.table") == "1.18.4"); \
  stopifnot(packageVersion("dplyr") == "1.2.1"); \
'

RUN Rscript -e ' \
  url <- "https://github.com/zhenin/HDL/archive/e6b055d42fd9904c3e994a9b829626c7e5f8422b.tar.gz"; \
  archive <- tempfile(fileext = ".tar.gz"); \
  source_root <- tempfile(); \
  download.file(url, archive, mode = "wb"); \
  dir.create(source_root); \
  untar(archive, exdir = source_root); \
  package_dir <- list.files(source_root, full.names = TRUE, pattern = "^HDL-")[[1]]; \
  install.packages( \
    file.path(package_dir, "HDL"), \
    repos = NULL, \
    type = "source" \
  ); \
  stopifnot(requireNamespace("HDL", quietly = TRUE)); \
  stopifnot(packageVersion("HDL") == "1.4.3"); \
'

WORKDIR /work

ENTRYPOINT ["Rscript"]
