# Containerized PLINK2 image for offline LD clumping against 1000G EUR PLINK
# reference panels. The image only carries the official PLINK2 binary and its
# runtime; reference panels, GWAS inputs, and user data stay in the data
# catalog.
#
# Pinned source (Stage 0 intake):
#   tool:        PLINK 2.0 alpha (plink2)
#   version:     v2.0.0-a.6.26 (released 2025-10-27)
#   upstream:    https://github.com/chrchang/plink-ng
#   source pin:  tag v2.0.0-a.6.26 (commit faed32c9)
#   asset:       plink2_linux_avx2.zip
#                sha256: f578a450af382d7dd6665aecf0ca1d280971c2b3d5bb5556efbf9266c4c8da0f
#   license:     GPL-3.0-only (matches repo-wide GPL tool policy)
#
# The archive is fetched at build time from GitHub with the upstream SHA256
# enforced; the build fails loudly if the upstream artifact is tampered with.
# The image contains only the binary plus minimal POSIX utilities (ca-certificates
# for TLS, unzip, wget, coreutils).
FROM debian:bookworm-slim

LABEL org.opencontainers.image.title="autonomics-plink2-original" \
      org.opencontainers.image.description="Official PLINK2 a.6.26 for offline LD clumping." \
      org.opencontainers.image.source="https://github.com/chrchang/plink-ng" \
      org.opencontainers.image.licenses="GPL-3.0-only" \
      org.opencontainers.image.version="2.0.0-a.6.26" \
      org.opencontainers.image.revision="faed32c9" \
      org.opencontainers.image.documentation="https://www.cog-genomics.org/plink/2.0/"

ARG PLINK2_VERSION=2.0.0-a.6.26
ARG PLINK2_DATE_TAG=20251027
ARG PLINK2_ASSET_SHA256=f578a450af382d7dd6665aecf0ca1d280971c2b3d5bb5556efbf9266c4c8da0f
ARG PLINK2_ASSET=plink2_linux_avx2.zip

ENV PLINK2_VERSION=${PLINK2_VERSION} \
    PLINK2_DATE_TAG=${PLINK2_DATE_TAG}

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        ca-certificates \
        unzip \
        wget \
        coreutils \
 && rm -rf /var/lib/apt/lists/*

# Fetch the upstream tarball at the pinned tag, verify SHA256, and unpack
# only the PLINK2 binary. Different releases have shipped the binary at
# different archive paths (root or under one directory), so the unpack
# tolerates both layouts.
RUN set -eux \
 && cd /tmp \
 && wget -q "https://github.com/chrchang/plink-ng/releases/download/v${PLINK2_VERSION}/${PLINK2_ASSET}" \
        -O "${PLINK2_ASSET}" \
 && echo "${PLINK2_ASSET_SHA256}  ${PLINK2_ASSET}" > expected.sha256 \
 && sha256sum -c expected.sha256 \
 && mkdir -p /opt/plink2 \
 && unzip -q "${PLINK2_ASSET}" -d /opt/plink2 \
 && if [ -f /opt/plink2/plink2 ]; then \
        : ; \
    elif [ -f /opt/plink2/*/plink2 ]; then \
        mv /opt/plink2/*/plink2 /opt/plink2/plink2 ; \
    else \
        echo "plink2 binary not found in archive" >&2 ; \
        exit 1 ; \
    fi \
 && chmod 0755 /opt/plink2/plink2 \
 && ln -s /opt/plink2/plink2 /usr/local/bin/plink2 \
 && rm -f "${PLINK2_ASSET}" expected.sha256 \
 && plink2 --version

WORKDIR /work

# The default entrypoint is the PLINK2 binary; the wrapper script emitted by
# the DAG wrapper passes argv directly. Containers run with a read-only root
# filesystem; /work remains writable.
ENTRYPOINT ["plink2"]
CMD ["--help"]
