FROM docker.io/rocker/r-ver:4.5.1

LABEL org.opencontainers.image.title="autonomics-twosamplemr-original" \
  org.opencontainers.image.version="0.7.9" \
  org.opencontainers.image.source="https://github.com/MRCIEU/TwoSampleMR" \
  org.opencontainers.image.license="MIT" \
  org.opencontainers.image.revision="3d119f20d6fc164b0c7f710f5590fee9580f2c7b"

ARG CRAN_SNAPSHOT=2026-09-13
ARG TWOSAMPLEMR_SHA256=6848c344c5eead601ff52e9a88b2c23c2b61b0ce4f848e331e7494b657be64e5
ARG MRMIX_SHA256=b8847e5f57311dc5335461e4e1fcee13c832ff03f26f53819746bf292709d8b6
ARG RADIALMR_SHA256=71216b4a1a8827a3f2dd6322c72db4f490793e861df0d626f0bf4e189a524ea5
ARG MRPRESSO_SHA256=8bee27809fdf2ab69e549d08db0ff914d1af80f16119973c56f98c07f54f8728
ARG PLINK2_VERSION=2.0.0-a.6.26
ARG PLINK2_DATE_TAG=20251027
ARG PLINK2_ASSET_SHA256=f578a450af382d7dd6665aecf0ca1d280971c2b3d5bb5556efbf9266c4c8da0f
ARG PLINK2_ASSET=plink2_linux_avx2.zip

ENV CRAN_SNAPSHOT=${CRAN_SNAPSHOT}

WORKDIR /tmp/source

RUN Rscript -e 'options(repos = c(CRAN = sprintf("https://packagemanager.posit.co/cran/__linux__/noble/%s", Sys.getenv("CRAN_SNAPSHOT"))), HTTPUserAgent = sprintf("R/%s R (%s)", getRversion(), paste(getRversion(), R.version$platform, R.version$arch, R.version$os))); install.packages(c("cli", "cowplot", "data.table", "dplyr", "ggplot2", "glmnet", "gridExtra", "gtable", "ieugwasr", "jsonlite", "knitr", "lattice", "magrittr", "MASS", "pbapply", "plotly", "psych", "rmarkdown", "tidyr"))'

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl unzip \
  && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
  curl -fsSL https://github.com/gqi/MRMix/archive/56afdb2bc96760842405396f5d3f02e60e305039.tar.gz -o MRMix.tar.gz; \
  echo "${MRMIX_SHA256}  MRMix.tar.gz" > checksums; \
  curl -fsSL https://github.com/WSpiller/RadialMR/archive/a30ff117fbfb6733ecdad0d69b8f5dd07958ed47.tar.gz -o RadialMR.tar.gz; \
  echo "${RADIALMR_SHA256}  RadialMR.tar.gz" >> checksums; \
  curl -fsSL https://github.com/rondolab/MR-PRESSO/archive/3e3c92d7eda6dce0d1d66077373ec0f7ff4f7e87.tar.gz -o MRPRESSO.tar.gz; \
  echo "${MRPRESSO_SHA256}  MRPRESSO.tar.gz" >> checksums; \
  curl -fsSL https://api.github.com/repos/MRCIEU/TwoSampleMR/tarball/v0.7.9 -o TwoSampleMR.tar.gz; \
  echo "${TWOSAMPLEMR_SHA256}  TwoSampleMR.tar.gz" >> checksums; \
  sha256sum -c checksums; \
  Rscript -e 'install.packages(c("MRMix.tar.gz", "RadialMR.tar.gz", "MRPRESSO.tar.gz", "TwoSampleMR.tar.gz"), repos = NULL, type = "source", INSTALL_opts = "--no-build-vignettes")'; \
  rm -f *.tar.gz checksums

RUN set -eux; \
  curl -fsSL "https://github.com/chrchang/plink-ng/releases/download/v${PLINK2_VERSION}/${PLINK2_ASSET}" -o "${PLINK2_ASSET}"; \
  echo "${PLINK2_ASSET_SHA256}  ${PLINK2_ASSET}" > plink2.sha256; \
  sha256sum -c plink2.sha256; \
  mkdir -p /opt/plink2; \
  unzip -q "${PLINK2_ASSET}" -d /opt/plink2; \
  if [ -f /opt/plink2/plink2 ]; then :; elif [ -f /opt/plink2/*/plink2 ]; then mv /opt/plink2/*/plink2 /opt/plink2/plink2; else echo "plink2 binary not found" >&2; exit 1; fi; \
  chmod 0755 /opt/plink2/plink2; \
  ln -s /opt/plink2/plink2 /usr/local/bin/plink2; \
  rm -f "${PLINK2_ASSET}" plink2.sha256; \
  plink2 --version

RUN Rscript -e 'stopifnot(packageVersion("TwoSampleMR") == "0.7.9"); stopifnot(requireNamespace("MRMix", quietly = TRUE)); stopifnot(requireNamespace("RadialMR", quietly = TRUE)); stopifnot(requireNamespace("MRPRESSO", quietly = TRUE)); load(system.file("extdata", "test_commondata.RData", package = "TwoSampleMR")); stopifnot(nrow(dat) == 79L)'

WORKDIR /work

ENTRYPOINT ["Rscript"]
