# Containerized GCTA image for summary-statistics analyses.
#
# Stage 0 intake:
#   tool:    GCTA (Genome-wide Complex Trait Analysis)
#   version: 1.95.3 Linux (released 2026-07-10)
#   asset:   gcta-1.95.3-linux-x86_64.zip
#            sha256: 441e01715bc12dabb083fe76372432139dff9bb2c073d21d218d15e92580e807
#   license: MIT for the official executable (source code is GPL-3.0-or-later)
#
# The official Linux asset is an AppImage. It is extracted during the build so
# the runtime needs neither FUSE nor privileges. Reference panels and user data
# remain in the data catalog and runtime staging.
FROM debian:bookworm-slim AS unpack

ARG GCTA_VERSION=1.95.3
ARG GCTA_ASSET_SHA256=441e01715bc12dabb083fe76372432139dff9bb2c073d21d218d15e92580e807
ARG GCTA_ASSET=gcta-1.95.3-linux-x86_64.zip

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        ca-certificates \
        unzip \
        wget \
 && rm -rf /var/lib/apt/lists/*

RUN set -eux \
 && cd /tmp \
 && wget -q "https://yanglab.westlake.edu.cn/software/gcta/bin/${GCTA_ASSET}" \
        -O "${GCTA_ASSET}" \
 && echo "${GCTA_ASSET_SHA256}  ${GCTA_ASSET}" > "${GCTA_ASSET}.sha256" \
 && sha256sum -c "${GCTA_ASSET}.sha256" \
 && unzip -q "${GCTA_ASSET}" \
 && cd "gcta-${GCTA_VERSION}-linux-x86_64" \
 && ./gcta --appimage-extract \
 && test -x squashfs-root/usr/bin/gcta64

FROM debian:bookworm-slim

LABEL org.opencontainers.image.title="autonomics-gcta-original" \
      org.opencontainers.image.description="Official GCTA 1.95.3 executable." \
      org.opencontainers.image.source="https://yanglab.westlake.edu.cn/software/gcta/" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.version="1.95.3" \
      org.opencontainers.image.documentation="https://yanglab.westlake.edu.cn/software/gcta/#Overview"

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        libstdc++6 \
        zlib1g \
 && rm -rf /var/lib/apt/lists/*

COPY --from=unpack /tmp/gcta-1.95.3-linux-x86_64/squashfs-root/usr/bin/gcta64 \
     /opt/gcta/bin/gcta64
COPY --from=unpack /tmp/gcta-1.95.3-linux-x86_64/squashfs-root/usr/lib/ \
     /opt/gcta/lib/

ENV LD_LIBRARY_PATH=/opt/gcta/lib

RUN chmod 0755 /opt/gcta/bin/gcta64 \
 && ln -s /opt/gcta/bin/gcta64 /usr/local/bin/gcta64 \
 && ldd /opt/gcta/bin/gcta64 | tee /tmp/gcta-ldd.txt \
 && ! grep -q 'not found' /tmp/gcta-ldd.txt \
 && { set +e; gcta64 >/tmp/gcta-smoke.txt 2>&1; smoke_status=$?; set -e; } \
 && grep -q 'version v1.95.3 Linux' /tmp/gcta-smoke.txt \
 && test "$smoke_status" -eq 1 \
 && rm -f /tmp/gcta-ldd.txt /tmp/gcta-smoke.txt

WORKDIR /work

ENTRYPOINT ["gcta64"]
