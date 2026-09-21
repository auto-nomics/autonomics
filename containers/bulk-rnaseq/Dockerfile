# Layered on top of the pinned autonomics/deseq2 image: that image already
# ships R 4.5.3, the DESeq2 closure and the pinned CRAN snapshot closure
# used by the autonomics build pipeline. This stage adds limma (voom/eBayes),
# edgeR (TMM/GLM) and WGCNA (signed adjacency + TOM + dynamic tree cut +
# module eigengenes), copies the two runner scripts and verifies the
# resulting package set before publishing.

FROM ghcr.io/auto-nomics/autonomics/deseq2@sha256:8b2e2a78d87293e6cae6dbed2e283dff1dd8461a7f993cb347ca9810698b2b3e

LABEL org.opencontainers.image.title="autonomics-bulk-rnaseq" \
      org.opencontainers.image.description="limma + edgeR + voom/eBayes + WGCNA runtime for the autonomics bulk-rnaseq wrapper nodes" \
      org.opencontainers.image.version="1.0.0" \
      org.opencontainers.image.source="https://github.com/autonomics/bulk-rnaseq" \
      org.opencontainers.image.licenses="Artistic-2.0 OR GPL-2.0-or-later"

ARG LIMMA_VERSION=3.66.0
ARG LIMMA_SHA256=d8ba41bf7030738208a1c5d937f6c91b461d785cfe21dc5c54f22c4e66b901a4
ARG EDGER_VERSION=4.8.2
ARG EDGER_SHA256=dc1451f32fc5793001b14e0cae1b0fafcadc3b7081637f2f8e8dba5ee4760da4
ARG WGCNA_VERSION=1.74
ARG WGCNA_SHA256=4f61e117b82287f01b61bf936d5c5ea3814c9c4b0d0b90d5016dda77f1f955d6
ARG BIOCONDUCTOR_RELEASE=3.22
ARG CRAN_SNAPSHOT=2026-09-13

USER root
ENV OMP_NUM_THREADS=1 \
    OPENBLAS_NUM_THREADS=1 \
    MKL_NUM_THREADS=1

RUN apt-get update \
    && apt-get install -y --no-install-recommends libxml2-dev libpcre2-dev libbz2-dev liblzma-dev \
    && rm -rf /var/lib/apt/lists/*

RUN Rscript --vanilla -e ' \
  cran <- sprintf("https://packagemanager.posit.co/cran/__linux__/noble/%s", Sys.getenv("CRAN_SNAPSHOT")); \
  bioc <- BiocManager::repositories(version = Sys.getenv("BIOCONDUCTOR_RELEASE")); \
  bioc <- bioc[names(bioc) != "CRAN"]; \
  options( \
    repos = c(CRAN = cran, bioc), \
    HTTPUserAgent = sprintf("R/%s R (%s)", getRversion(), paste(getRversion(), R.version$platform, R.version$arch, R.version$os)) \
  ); \
  install.packages(c("statmod", "locfit")); \
  verify_archive <- function(package, version, sha, repo_url) { \
    url <- sprintf("%s/%s_%s.tar.gz", repo_url, package, version); \
    archive <- tempfile(fileext = ".tar.gz"); \
    download.file(url, archive, mode = "wb"); \
    stopifnot(identical(digest::digest(archive, algo = "sha256", file = TRUE), sha)); \
    install.packages(archive, repos = NULL, type = "source"); \
    unlink(archive); \
  }; \
  verify_archive("limma", Sys.getenv("LIMMA_VERSION"), Sys.getenv("LIMMA_SHA256"), \
                 sprintf("https://bioconductor.org/packages/%s/bioc/src/contrib", Sys.getenv("BIOCONDUCTOR_RELEASE"))); \
  verify_archive("edgeR", Sys.getenv("EDGER_VERSION"), Sys.getenv("EDGER_SHA256"), \
                 sprintf("https://bioconductor.org/packages/%s/bioc/src/contrib", Sys.getenv("BIOCONDUCTOR_RELEASE"))); \
' \
  && Rscript --vanilla -e ' \
  cran <- sprintf("https://packagemanager.posit.co/cran/__linux__/noble/%s", Sys.getenv("CRAN_SNAPSHOT")); \
  bioc <- BiocManager::repositories(version = Sys.getenv("BIOCONDUCTOR_RELEASE")); \
  bioc <- bioc[names(bioc) != "CRAN"]; \
  options( \
    repos = c(CRAN = cran, bioc), \
    HTTPUserAgent = sprintf("R/%s R (%s)", getRversion(), paste(getRversion(), R.version$platform, R.version$arch, R.version$os)) \
  ); \
  install.packages(c( \
    "dynamicTreeCut", "preprocessCore", "impute", "fastcluster", "matrixStats", \
    "doParallel", "foreach", "Rcpp", "Hmisc" \
  )); \
' \
  && Rscript --vanilla -e ' \
  cran <- sprintf("https://packagemanager.posit.co/cran/__linux__/noble/%s", Sys.getenv("CRAN_SNAPSHOT")); \
  options( \
    repos = c(CRAN = cran), \
    HTTPUserAgent = sprintf("R/%s R (%s)", getRversion(), paste(getRversion(), R.version$platform, R.version$arch, R.version$os)) \
  ); \
  archive <- tempfile(fileext = ".tar.gz"); \
  download.file(sprintf("https://cran.r-project.org/src/contrib/WGCNA_%s.tar.gz", Sys.getenv("WGCNA_VERSION")), archive, mode = "wb"); \
  stopifnot(identical(digest::digest(archive, algo = "sha256", file = TRUE), Sys.getenv("WGCNA_SHA256"))); \
  install.packages(archive, repos = NULL, type = "source"); \
  unlink(archive); \
' \
  && Rscript -e ' \
  stopifnot( \
    packageVersion("limma") == Sys.getenv("LIMMA_VERSION"), \
    packageVersion("edgeR") == Sys.getenv("EDGER_VERSION"), \
    packageVersion("WGCNA") == Sys.getenv("WGCNA_VERSION"), \
    packageVersion("dynamicTreeCut") >= "1.63", \
    packageVersion("preprocessCore") >= "1.65", \
    packageVersion("DESeq2") >= "1.50" \
  ); \
'

COPY limma_voom_runner.R /opt/autonomics/limma_voom_runner.R
COPY wgcna_runner.R /opt/autonomics/wgcna_runner.R
RUN chmod 0555 /opt/autonomics/limma_voom_runner.R /opt/autonomics/wgcna_runner.R

ENV HOME=/tmp

USER 1000:1000
WORKDIR /work

ENTRYPOINT ["Rscript", "--vanilla"]
