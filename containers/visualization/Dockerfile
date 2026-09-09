FROM docker.io/rocker/r-ver:4.5.3

LABEL org.opencontainers.image.title="autonomics-visualization" \
      org.opencontainers.image.version="0.1.0"

RUN Rscript -e ' \
  options(repos = c(CRAN = "https://p3m.dev/cran/__linux__/noble/2026-04-23")); \
  install.packages(c("arrow", "ggplot2")); \
  stopifnot(packageVersion("arrow") == "23.0.1.2"); \
  stopifnot(packageVersion("ggplot2") == "4.0.3"); \
'

COPY render.R /opt/autonomics/render.R

RUN chmod 0755 /opt/autonomics/render.R

WORKDIR /work
ENTRYPOINT ["Rscript"]
