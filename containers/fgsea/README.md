# fgsea container

This image provides a pinned [Bioconductor](https://bioconductor.org/) R
runtime with `fgsea::fgseaMultilevel`, `data.table`, and `jsonlite`. It is the
execution image for the `pathway_gsea_container` node. Gene ranks and pathway
inputs stay outside the image; the container receives staged files through the
standard `AUTONOMICS_INPUT*` and `AUTONOMICS_OUTPUT*` contract.

The digest-pinned base is `bioconductor/bioconductor:3.21-R-4.5.2`. The image
runs as UID/GID 1001 and is compatible with the node wrapper's isolated
network, read-only root filesystem, and `/work` scratch mount.

Published immutable image:

```text
ghcr.io/auto-nomics/autonomics/pathway-gsea@sha256:1eb3a32abe2f910e8a3e2c7b5cd8d9f45bf267dc7c57605103e5c30310740275
```

## Build

From the repository root:

```sh
podman build --format docker -t autonomics/pathway-gsea:draft containers/fgsea
```

Smoke-test the installed R packages:

```sh
podman run --rm autonomics/pathway-gsea:draft \
  Rscript --vanilla -e 'cat(as.character(packageVersion("fgsea")), "\n")'
```

## Publishing

Do not publish an image as a side effect of ordinary code development. After
reviewing the build, tag the immutable manifest, push it to the configured
GHCR namespace, and copy the manifest digest into
`PATHWAY_GSEA_IMAGE_DIGEST` in `crates/node-bundles/nodes-io/src/pathway_gsea_container.rs`.

```sh
AUTONOMICS_IMAGE_PREFIX=ghcr.io/auto-nomics/autonomics
podman tag autonomics/pathway-gsea:draft \
  "$AUTONOMICS_IMAGE_PREFIX/pathway-gsea:0.1.0"
podman push "$AUTONOMICS_IMAGE_PREFIX/pathway-gsea:0.1.0"
podman inspect --format '{{index .RepoDigests 0}}' \
  "$AUTONOMICS_IMAGE_PREFIX/pathway-gsea:0.1.0"
```
