FROM debian:bookworm-slim

LABEL org.opencontainers.image.title="autonomics-magma-original" \
  org.opencontainers.image.version="1.10" \
  org.opencontainers.image.source="https://ctg.cncr.nl/software/magma"

COPY bin/magma /usr/local/bin/magma
RUN chmod 0755 /usr/local/bin/magma

WORKDIR /work

ENTRYPOINT ["magma"]
