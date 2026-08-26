FROM docker.io/rocker/r-ver:4.5.1

LABEL org.opencontainers.image.title="autonomics-lava-original" \
  org.opencontainers.image.version="0.1.5" \
  org.opencontainers.image.source="https://github.com/josefin-werme/LAVA" \
  org.opencontainers.image.revision="4738b097bf929ec8af40225c196c57d46d3d8a22" \
  org.opencontainers.image.licenses="all-rights-reserved"

RUN Rscript -e 'install.packages("https://cran.r-project.org/src/contrib/keep_1.0.tar.gz", repos = NULL, type = "source")'
RUN Rscript -e 'install.packages(c("https://cran.r-project.org/src/contrib/Archive/data.table/data.table_1.18.4.tar.gz", "https://cran.r-project.org/src/contrib/matrixsampling_2.0.0.tar.gz", "https://cran.r-project.org/src/contrib/cpp11_0.5.5.tar.gz"), repos = NULL, type = "source")'
RUN Rscript -e 'url <- "https://github.com/josefin-werme/LAVA/archive/4738b097bf929ec8af40225c196c57d46d3d8a22.tar.gz"; archive <- tempfile(fileext = ".tar.gz"); source_dir <- tempfile(); download.file(url, archive, mode = "wb"); dir.create(source_dir); untar(archive, exdir = source_dir); package_dir <- list.files(source_dir, full.names = TRUE, pattern = "^LAVA")[[1]]; install.packages(package_dir, repos = NULL, type = "source"); stopifnot(requireNamespace("LAVA", quietly = TRUE))'

WORKDIR /work

ENTRYPOINT ["Rscript"]
