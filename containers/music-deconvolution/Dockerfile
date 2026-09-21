# Pinned MuSiC runtime for reference-based bulk RNA-seq deconvolution.
#
# The image intentionally contains no reference panels or disease datasets.
# Reference single-cell expression, cell metadata, optional cell sizes, and
# optional markers are staged by the DAG for every run.

FROM ghcr.io/auto-nomics/autonomics/bulk-rnaseq@sha256:c63fb113760b97bad1b1761d1daf4ad3241aa32e054ab20dd3a6bced4a552a6d

LABEL org.opencontainers.image.title="autonomics-music-deconvolution" \
      org.opencontainers.image.description="Pinned MuSiC bulk RNA-seq deconvolution runtime" \
      org.opencontainers.image.version="1.0.0" \
      org.opencontainers.image.source="https://github.com/xuranw/MuSiC" \
      org.opencontainers.image.revision="f21fe67f5670d5e9fca0ad7550abaae3423eb59c" \
      org.opencontainers.image.licenses="GPL-3.0-or-later"

ARG CRAN_SNAPSHOT=2026-09-13
ARG BIOCONDUCTOR_RELEASE=3.22
ARG MUSIC_VERSION=1.0.0
ARG MUSIC_COMMIT=f21fe67f5670d5e9fca0ad7550abaae3423eb59c
ARG MUSIC_SHA256=57f2cd50335ffee220317b80169a88c28a18060a9830080424f0b28391b9d9f1

USER root

RUN Rscript --vanilla -e ' \
  cran <- sprintf("https://packagemanager.posit.co/cran/__linux__/noble/%s", Sys.getenv("CRAN_SNAPSHOT")); \
  bioc <- BiocManager::repositories(version = Sys.getenv("BIOCONDUCTOR_RELEASE")); \
  bioc <- bioc[names(bioc) != "CRAN"]; \
  options( \
    repos = c(CRAN = cran, bioc), \
    HTTPUserAgent = sprintf("R/%s R (%s)", getRversion(), paste(getRversion(), R.version$platform, R.version$arch, R.version$os)) \
  ); \
  BiocManager::install( \
    c("SingleCellExperiment", "TOAST"), \
    version = Sys.getenv("BIOCONDUCTOR_RELEASE"), \
    ask = FALSE, \
    update = FALSE \
  ); \
  install.packages(c("MCMCpack", "nnls")); \
'

RUN Rscript --vanilla -e ' \
  archive <- tempfile(fileext = ".tar.gz"); \
  url <- sprintf("https://api.github.com/repos/xuranw/MuSiC/tarball/%s", Sys.getenv("MUSIC_COMMIT")); \
  download.file(url, archive, mode = "wb"); \
  stopifnot(identical(digest::digest(archive, algo = "sha256", file = TRUE), Sys.getenv("MUSIC_SHA256"))); \
  install.packages(archive, repos = NULL, type = "source"); \
  unlink(archive); \
  stopifnot(packageVersion("MuSiC") == Sys.getenv("MUSIC_VERSION")); \
'

COPY music_runner.R /opt/autonomics/music_runner.R
RUN chmod 0555 /opt/autonomics/music_runner.R

ENV HOME=/tmp

USER 1000:1000
WORKDIR /work

ENTRYPOINT ["Rscript", "--vanilla", "/opt/autonomics/music_runner.R"]
