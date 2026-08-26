FROM docker.io/rocker/r-ver:4.5.1

LABEL org.opencontainers.image.title="autonomics-mvmr-original" \
  org.opencontainers.image.version="0.4.8" \
  org.opencontainers.image.source="https://github.com/WSpiller/MVMR" \
  org.opencontainers.image.revision="bceaa38088d093a5d30c713afb016e7fbc7ed2be"

RUN Rscript -e 'install.packages(c("boot", "parallelly"))' && \
  Rscript -e ' \
  url <- "https://github.com/WSpiller/MVMR/archive/bceaa38088d093a5d30c713afb016e7fbc7ed2be.tar.gz"; \
  archive <- tempfile(fileext = ".tar.gz"); \
  source_dir <- tempfile(); \
  download.file(url, archive, mode = "wb"); \
  dir.create(source_dir); \
  untar(archive, exdir = source_dir); \
  package_dir <- list.files(source_dir, full.names = TRUE, pattern = "MVMR")[[1]]; \
  install.packages(package_dir, repos = NULL, type = "source"); \
  stopifnot(requireNamespace("MVMR", quietly = TRUE)); \
  '

WORKDIR /work

ENTRYPOINT ["Rscript"]
