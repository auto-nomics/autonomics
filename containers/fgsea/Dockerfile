FROM docker.io/bioconductor/bioconductor:3.21-R-4.5.2@sha256:359702d482e70e343d2a436731b7c6deffd1ec184a1f1700de5d64b0c97cacba

LABEL org.opencontainers.image.title="autonomics-pathway-gsea" \
      org.opencontainers.image.description="Pinned Bioconductor fgsea runtime for preranked pathway GSEA" \
      org.opencontainers.image.version="0.1.0" \
      org.opencontainers.image.source="https://bioconductor.org/packages/fgsea/" \
      org.opencontainers.image.licenses="Artistic-2.0"

ENV R_MAX_NUM_COMPILE_THREADS=2 \
    OMP_NUM_THREADS=1 \
    OPENBLAS_NUM_THREADS=1 \
    MKL_NUM_THREADS=1

RUN Rscript -e ' \
    options(Ncpus = 2); \
    if (!requireNamespace("BiocManager", quietly = TRUE)) install.packages("BiocManager"); \
    BiocManager::install("fgsea", ask = FALSE, update = FALSE); \
    install.packages(c("data.table", "jsonlite")); \
' \
    && Rscript -e ' \
    stopifnot( \
      requireNamespace("fgsea", quietly = TRUE), \
      requireNamespace("data.table", quietly = TRUE), \
      requireNamespace("jsonlite", quietly = TRUE), \
      exists("fgseaMultilevel", where = asNamespace("fgsea"), inherits = FALSE) \
    ); \
'

RUN groupadd --gid 1001 autonomics \
    && useradd --uid 1001 --gid autonomics --no-create-home autonomics

ENV HOME=/tmp \
    R_LIBS_USER=/tmp/R/library

USER 1001:1001

WORKDIR /work

CMD ["Rscript", "--vanilla"]
