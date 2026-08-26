FROM docker.io/rocker/r-ver:4.5.1

LABEL org.opencontainers.image.title="autonomics-mrpresso-original" \
  org.opencontainers.image.version="1.0.0" \
  org.opencontainers.image.source="https://github.com/rondolab/MR-PRESSO" \
  org.opencontainers.image.revision="3e3c92d7eda6dce0d1d66077373ec0f7ff4f7e87"

RUN Rscript -e ' \
  url <- "https://github.com/rondolab/MR-PRESSO/archive/3e3c92d7eda6dce0d1d66077373ec0f7ff4f7e87.tar.gz"; \
  archive <- tempfile(fileext = ".tar.gz"); \
  source_dir <- tempfile(); \
  download.file(url, archive, mode = "wb"); \
  dir.create(source_dir); \
  untar(archive, exdir = source_dir); \
  package_dir <- list.files(source_dir, full.names = TRUE, pattern = "MR-PRESSO")[[1]]; \
  install.packages(package_dir, repos = NULL, type = "source"); \
  stopifnot(requireNamespace("MRPRESSO", quietly = TRUE)); \
'

WORKDIR /work

ENTRYPOINT ["Rscript"]
