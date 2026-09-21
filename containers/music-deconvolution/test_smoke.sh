#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

command -v podman >/dev/null 2>&1 || {
  echo "missing required command: podman" >&2
  exit 1
}

image_prefix=${AUTONOMICS_IMAGE_PREFIX:-ghcr.io/auto-nomics/autonomics}
IMAGE_NAME=${IMAGE_NAME:-"$image_prefix/music-deconvolution:1.0.0"}
LOCAL_TAG=${LOCAL_TAG:-"localhost/autonomics/music-deconvolution:1.0.0"}
BUILD_FLAGS=${BUILD_FLAGS:---no-cache}

echo ">>> podman build $LOCAL_TAG"
podman build $BUILD_FLAGS \
  -f containers/music-deconvolution/Dockerfile \
  -t "$LOCAL_TAG" \
  containers/music-deconvolution

echo ">>> verify pinned statistical runtime"
podman run --rm --network=none --entrypoint Rscript "$LOCAL_TAG" --vanilla -e '
  stopifnot(
    getRversion() == "4.5.3",
    packageVersion("MuSiC") == "1.0.0",
    packageVersion("TOAST") >= "1.24.0",
    packageVersion("SingleCellExperiment") >= "1.32.0"
  )
'

echo ">>> podman push $IMAGE_NAME"
podman tag "$LOCAL_TAG" "$IMAGE_NAME"
podman push "$IMAGE_NAME"
REMOTE_REF=$(podman image inspect --format '{{index .RepoDigests 0}}' "$IMAGE_NAME")
REMOTE_DIGEST=${REMOTE_REF#*@}
echo "published digest: $REMOTE_DIGEST"
