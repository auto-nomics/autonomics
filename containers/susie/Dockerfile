# Containerized susieR 0.16.6 for official susie_rss() fine-mapping.
#
# The image contains only the official R package plus the official gsa-mixer
# libbgmg helper used to query signed LD pairs from a mounted catalog panel.
# Reference BIM/LD matrices, GWAS inputs, and user data stay in the data
# catalog.
#
# Pinned sources:
#   susieR 0.16.6  (tag 0.16.6, commit ef213feed2cb82419677661a8c986e1504df2c73)
#   gsa-mixer 2.2.1 (commit ea2a445912f83e5767d67372b6075912ed5655d8)

FROM docker.io/library/python:3.10-slim-bookworm AS mixer-build

ARG MIXER_COMMIT=ea2a445912f83e5767d67372b6075912ed5655d8

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
      ca-certificates \
      wget \
      build-essential \
      cmake \
      libboost-date-time-dev \
      libboost-filesystem-dev \
      libboost-program-options-dev \
      libboost-system-dev && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build
RUN wget -q "https://github.com/precimed/gsa-mixer/archive/${MIXER_COMMIT}.tar.gz" \
      -O mixer.tar.gz \
 && mkdir /build/src \
 && tar -xzf mixer.tar.gz -C /build/src --strip-components=1 \
 && rm -f mixer.tar.gz

WORKDIR /build/src/src
RUN cmake -S . -B build \
      -DCMAKE_BUILD_TYPE=Release \
      -DBoost_NO_BOOST_CMAKE=ON && \
    cmake --build build --target bgmg --parallel 2

FROM docker.io/rocker/r-ver:4.5.1

LABEL org.opencontainers.image.title="autonomics-susie-original" \
      org.opencontainers.image.version="0.16.6" \
      org.opencontainers.image.source="https://github.com/stephenslab/susieR" \
      org.opencontainers.image.revision="ef213feed2cb82419677661a8c986e1504df2c73" \
      org.opencontainers.image.licenses="BSD-3-Clause"

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
      python3 \
      python3-numpy \
      python3-six \
      ca-certificates \
      wget \
      cmake \
      libgomp1 \
      libboost-date-time1.74.0 \
      libboost-filesystem1.74.0 \
      libboost-program-options1.74.0 \
      libboost-system1.74.0 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=mixer-build /build/src/src/build/lib/libbgmg.so /opt/mixer/lib/libbgmg.so
COPY --from=mixer-build /build/src/precimed/common/libbgmg.py /opt/mixer/precimed/common/libbgmg.py
COPY --from=mixer-build /build/src/precimed/common/__init__.py /opt/mixer/precimed/common/__init__.py

ENV BGMG_SHARED_LIBRARY=/opt/mixer/lib/libbgmg.so

RUN Rscript -e ' \
  install.packages(c("cpp11", "cpp11armadillo", "Rfast", "Matrix", "matrixStats", "mixsqp", "reshape", "crayon", "ggplot2"), repos = "https://cloud.r-project.org"); \
  url <- "https://github.com/stephenslab/susieR/archive/ef213feed2cb82419677661a8c986e1504df2c73.tar.gz"; \
  archive <- tempfile(fileext = ".tar.gz"); \
  source_dir <- tempfile(); \
  download.file(url, archive, mode = "wb"); \
  dir.create(source_dir); \
  untar(archive, exdir = source_dir); \
  package_dir <- list.files(source_dir, full.names = TRUE, pattern = "^susieR")[[1]]; \
  install.packages(package_dir, repos = NULL, type = "source"); \
  stopifnot(requireNamespace("susieR", quietly = TRUE)); \
  stopifnot(packageVersion("susieR") == "0.16.6"); \
'

COPY susie_ld_query.py /opt/susie/bin/susie_ld_query.py
COPY run_susie_rss.R /opt/susie/bin/run_susie_rss.R

WORKDIR /work

ENTRYPOINT ["Rscript"]
