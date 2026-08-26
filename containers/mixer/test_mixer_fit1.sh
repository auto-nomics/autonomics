#!/usr/bin/env sh
set -eu

TOOL_ROOT=${TOOL_ROOT:-"$(dirname "$0")/gsa-mixer"}
PODMAN=${PODMAN:-podman}
IMAGE=${IMAGE:-localhost/atc/mixer:2.2.1}

"$PODMAN" build -f "$TOOL_ROOT/../Dockerfile" -t "$IMAGE" "$TOOL_ROOT"
"$PODMAN" run --rm "$IMAGE" --version
"$PODMAN" run --rm "$IMAGE" fit1 --help >/dev/null

if "$PODMAN" run --rm --entrypoint sh "$IMAGE" -c 'test -d /panels || find / -type f -name "*.ld" | grep -q .'; then
  echo "mixer image unexpectedly contains reference data" >&2
  exit 1
fi
