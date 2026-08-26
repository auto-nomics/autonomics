# Containerized FUSION TWAS association testing.
#
# The image carries the unmodified official association script at the pinned
# source commit. GTEx v8 weight archives and the 1000G EUR LDREF stay in the
# immutable data catalog and are mounted by the DAG wrapper.

FROM docker.io/rocker/r-ver:4.5.1

LABEL org.opencontainers.image.title="autonomics-fusion-original" \
      org.opencontainers.image.version="1.0.0" \
      org.opencontainers.image.source="https://github.com/gusevlab/fusion_twas" \
      org.opencontainers.image.revision="9346c1222bffbb34499fa7a8e23c1b701b55cb05" \
      org.opencontainers.image.licenses="GPL-3.0-or-later"

RUN apt-get update \
 && apt-get install -y --no-install-recommends tar \
 && rm -rf /var/lib/apt/lists/* \
 && Rscript -e 'install.packages(c("here", "optparse"), repos = "https://cloud.r-project.org")'

COPY FUSION.assoc_test.R /opt/fusion/FUSION.assoc_test.R
COPY utils/plink_utils.R /opt/fusion/utils/plink_utils.R
COPY run_fusion_twas.sh /opt/fusion/bin/run_fusion_twas.sh
COPY LICENSE /opt/fusion/LICENSE

RUN chmod 0755 /opt/fusion/bin/run_fusion_twas.sh \
 && Rscript -e 'stopifnot(requireNamespace("here", quietly = TRUE), requireNamespace("optparse", quietly = TRUE))'

WORKDIR /opt/fusion
ENTRYPOINT ["/opt/fusion/bin/run_fusion_twas.sh"]
