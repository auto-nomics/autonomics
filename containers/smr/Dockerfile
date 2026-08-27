# Containerized SMR image for SMR/HEIDI analysis.
#
# Stage 0 intake:
#   tool:        SMR (Summary-data-based Mendelian Randomization)
#   version:     1.4.2 Linux (released 2026-07-13)
#   upstream:    https://yanglab.westlake.edu.cn/software/smr/
#   asset:       smr-1.4.2-linux-x86_64.zip
#                sha256: c01ef6a4c5d03c7d504e24b2ccf40ab4da56eb127295c7671c867048020f893c
#   license:     MIT for the official executable (source code is GPL-2.0)
#
# The image contains only the official AppImage payload and its bundled
# runtime libraries. BESD xQTL data, LD references, and GWAS inputs stay in
# the data catalog or runtime staging.
FROM debian:bookworm-slim AS unpack

ARG SMR_VERSION=1.4.2
ARG SMR_ASSET_SHA256=c01ef6a4c5d03c7d504e24b2ccf40ab4da56eb127295c7671c867048020f893c
ARG SMR_ASSET=smr-1.4.2-linux-x86_64.zip

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        ca-certificates \
        unzip \
        wget \
 && rm -rf /var/lib/apt/lists/*

RUN set -eux \
 && cd /tmp \
 && wget -q "https://yanglab.westlake.edu.cn/software/smr/download/${SMR_ASSET}" \
        -O "${SMR_ASSET}" \
 && echo "${SMR_ASSET_SHA256}  ${SMR_ASSET}" > "${SMR_ASSET}.sha256" \
 && sha256sum -c "${SMR_ASSET}.sha256" \
 && unzip -q "${SMR_ASSET}" \
 && cd "smr-${SMR_VERSION}-linux-x86_64" \
 && ./smr --appimage-extract \
 && test -x squashfs-root/usr/bin/smr

FROM debian:bookworm-slim

LABEL org.opencontainers.image.title="autonomics-smr-original" \
      org.opencontainers.image.description="Official SMR/HEIDI 1.4.2 executable." \
      org.opencontainers.image.source="https://yanglab.westlake.edu.cn/software/smr/" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.version="1.4.2" \
      org.opencontainers.image.documentation="https://yanglab.westlake.edu.cn/software/smr/#Overview"

COPY --from=unpack /tmp/smr-1.4.2-linux-x86_64/squashfs-root/usr /opt/smr/usr
RUN chmod 0755 /opt/smr/usr/bin/smr \
 && ln -s /opt/smr/usr/bin/smr /usr/local/bin/smr

WORKDIR /work
ENTRYPOINT ["/opt/smr/usr/bin/smr"]
CMD ["--help"]
