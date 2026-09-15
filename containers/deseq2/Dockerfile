FROM docker.io/rocker/r-ver:4.5.3@sha256:35394dcbf419ac29056848522006de3cd33c33191377abed182acaecd48eba37

LABEL org.opencontainers.image.title="autonomics-deseq2-original" \
      org.opencontainers.image.description="Pinned official DESeq2 runtime for bulk RNA-seq differential expression" \
      org.opencontainers.image.version="1.50.2" \
      org.opencontainers.image.source="https://github.com/thelovelab/DESeq2" \
      org.opencontainers.image.revision="d90821a" \
      org.opencontainers.image.licenses="LGPL-3.0-or-later"

ARG CRAN_SNAPSHOT=2026-09-13
ARG BIOCONDUCTOR_RELEASE=3.22
ARG DESEQ2_VERSION=1.50.2
ARG DESEQ2_SHA256=514f23ae8d274623d80978c30bfa1c6566acd98188bf1aa563970959ea59522f

ENV CRAN_SNAPSHOT=${CRAN_SNAPSHOT} \
    BIOCONDUCTOR_RELEASE=${BIOCONDUCTOR_RELEASE} \
    R_PROFILE_USER=/dev/null \
    OMP_NUM_THREADS=1 \
    OPENBLAS_NUM_THREADS=1 \
    MKL_NUM_THREADS=1

RUN apt-get update \
    && apt-get install -y --no-install-recommends zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

RUN Rscript --vanilla -e ' \
  cran <- sprintf("https://packagemanager.posit.co/cran/__linux__/noble/%s", Sys.getenv("CRAN_SNAPSHOT")); \
  options( \
    repos = c(CRAN = cran), \
    HTTPUserAgent = sprintf("R/%s R (%s)", getRversion(), paste(getRversion(), R.version$platform, R.version$arch, R.version$os)) \
  ); \
  install.packages(c("BiocManager", "digest", "jsonlite")); \
  repositories <- BiocManager::repositories(version = Sys.getenv("BIOCONDUCTOR_RELEASE")); \
  repositories <- repositories[names(repositories) != "CRAN"]; \
  options(repos = c(CRAN = cran, repositories)); \
  url <- sprintf( \
    "https://bioconductor.org/packages/%s/bioc/src/contrib/DESeq2_%s.tar.gz", \
    Sys.getenv("BIOCONDUCTOR_RELEASE"), \
    Sys.getenv("DESEQ2_VERSION") \
  ); \
  archive <- tempfile(fileext = ".tar.gz"); \
  download.file(url, archive, mode = "wb"); \
  stopifnot( \
    identical( \
      digest::digest(archive, algo = "sha256", file = TRUE), \
      Sys.getenv("DESEQ2_SHA256") \
    ) \
  ); \
  options(repos = c(CRAN = cran)); \
  BiocManager::install( \
    "DESeq2", \
    version = Sys.getenv("BIOCONDUCTOR_RELEASE"), \
    ask = FALSE, \
    update = FALSE \
  ); \
  stopifnot(packageVersion("DESeq2") == Sys.getenv("DESEQ2_VERSION")); \
  unlink(archive); \
'

COPY deseq2_runner.R /opt/autonomics/deseq2_runner.R
RUN chmod 0555 /opt/autonomics/deseq2_runner.R \
    && groupadd --gid 1000 autonomics \
    && useradd --uid 1000 --gid autonomics --no-create-home autonomics

ENV HOME=/tmp \
    XDG_CACHE_HOME=/tmp/cache

USER 1000:1000
WORKDIR /work

ENTRYPOINT ["Rscript", "--vanilla", "/opt/autonomics/deseq2_runner.R"]
