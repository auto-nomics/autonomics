#!/usr/bin/env sh
set -eu

TOOL_ROOT=${TOOL_ROOT:-"$(dirname "$0")/gsa-mixer"}
PODMAN=${PODMAN:-podman}
ACR_ENDPOINT=${ACR_ENDPOINT:-crpi-isjkczwpadlvr9i3.cn-hongkong.personal.cr.aliyuncs.com}
IMAGE=${IMAGE:-"$ACR_ENDPOINT/autonomics/mixer:2.2.1"}
EXPECTED_DIGEST=${MIXER_IMAGE_DIGEST:-sha256:3bd67cccf298bd3c9af3d2b013dd7dfacde9ad13d51bc78b2f7f1315f01bebb7}
BUILD_IMAGE=${BUILD_IMAGE:-0}
PUSH_IMAGE=${PUSH_IMAGE:-0}

if [ "$BUILD_IMAGE" = 1 ]; then
  "$PODMAN" build -f "$TOOL_ROOT/../Dockerfile" -t "$IMAGE" "$TOOL_ROOT"
elif ! "$PODMAN" image exists "$IMAGE"; then
  "$PODMAN" pull "$IMAGE"
fi

if [ "$PUSH_IMAGE" = 1 ]; then
  "$PODMAN" push "$IMAGE"
fi

ACTUAL_DIGEST=$("$PODMAN" image inspect "$IMAGE" --format '{{.Digest}}')
if [ "$ACTUAL_DIGEST" != "$EXPECTED_DIGEST" ]; then
  echo "mixer image digest mismatch: expected $EXPECTED_DIGEST, got $ACTUAL_DIGEST" >&2
  exit 1
fi

"$PODMAN" run --rm "$IMAGE" --version
"$PODMAN" run --rm "$IMAGE" fit1 --help >/dev/null

if "$PODMAN" run --rm --entrypoint sh "$IMAGE" -c 'test -d /panels || find / -type f -name "*.ld" | grep -q .'; then
  echo "mixer image unexpectedly contains reference data" >&2
  exit 1
fi
