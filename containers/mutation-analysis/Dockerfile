FROM docker.io/bioconductor/bioconductor@sha256:359702d482e70e343d2a436731b7c6deffd1ec184a1f1700de5d64b0c97cacba

LABEL org.opencontainers.image.title="autonomics-mutation-analysis" \
      org.opencontainers.image.description="Pinned R maftools runtime for mutation analyses" \
      org.opencontainers.image.version="0.1.0" \
      org.opencontainers.image.source="https://github.com/PoisonAlien/maftools" \
      org.opencontainers.image.licenses="MIT"

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

RUN Rscript -e 'options(warn = 1, Ncpus = 1); if (!requireNamespace("BiocManager", quietly = TRUE)) install.packages("BiocManager", repos = "https://cloud.r-project.org"); BiocManager::install("maftools", ask = FALSE, update = FALSE); library(maftools); stopifnot(packageVersion("maftools") >= "2.20.0")'

COPY mutation_analysis.R /opt/autonomics/mutation_analysis.R
RUN chmod 0555 /opt/autonomics/mutation_analysis.R

USER 1000:1000
WORKDIR /work

ENTRYPOINT ["Rscript", "/opt/autonomics/mutation_analysis.R"]
