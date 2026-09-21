#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

need() {
  command -v "$1" >/dev/null 2>&1 || { echo "missing required command: $1" >&2; exit 1; }
}

need podman

image_prefix=${AUTONOMICS_IMAGE_PREFIX:-ghcr.io/auto-nomics/autonomics}
IMAGE_NAME=${IMAGE_NAME:-"$image_prefix/bulk-rnaseq:1.0.0"}
LOCAL_TAG=${LOCAL_TAG:-"localhost/autonomics/bulk-rnaseq:1.0.0"}
BUILD_FLAGS=${BUILD_FLAGS:---no-cache}

# Build the layered image (the Dockerfile is built on top of the published
# autonomics/deseq2 image, which already contains R 4.5.3 and the closure of
# CRAN packages used by the autonomics pipeline).
echo ">>> podman build $LOCAL_TAG"
podman build $BUILD_FLAGS -f containers/bulk-rnaseq/Dockerfile -t "$LOCAL_TAG" containers/bulk-rnaseq

# Smoke: verify the R runtime and exact statistical package versions before
# publishing the image.
podman run --rm --network=none --entrypoint Rscript "$LOCAL_TAG" --vanilla -e '
  stopifnot(
    getRversion() == "4.5.3",
    packageVersion("limma") == "3.66.0",
    packageVersion("edgeR") == "4.8.2",
    packageVersion("WGCNA") == "1.74",
    packageVersion("dynamicTreeCut") >= "1.63"
  )
'

# Publish to the configured registry, then resolve the immutable manifest digest
# from the local image's repository-digest reference.
echo ">>> podman push $IMAGE_NAME"
podman tag "$LOCAL_TAG" "$IMAGE_NAME"
podman push "$IMAGE_NAME"
REMOTE_REF=$(podman image inspect --format '{{index .RepoDigests 0}}' "$IMAGE_NAME")
REMOTE_DIGEST=${REMOTE_REF#*@}
echo "published digest: $REMOTE_DIGEST"
