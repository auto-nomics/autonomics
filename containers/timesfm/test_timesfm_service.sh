#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_timesfm_service.sh

Builds the TimesFM service image and checks its HTTP health contract.
The test intentionally does not download the 1.3GB checkpoint.

Environment:
  TIMESFM_IMAGE  Image tag (default localhost/atc/timesfm:3.0.2)
  BUILD_IMAGE=0  Skip the Podman build
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
image=${TIMESFM_IMAGE:-localhost/atc/timesfm:3.0.2}
build_image=${BUILD_IMAGE:-1}

[[ "${1:-}" == "-h" || "${1:-}" == "--help" ]] && {
  usage
  exit 0
}

command -v podman >/dev/null || {
  echo "missing required command: podman" >&2
  exit 1
}
command -v curl >/dev/null || {
  echo "missing required command: curl" >&2
  exit 1
}

if [[ "$build_image" == 1 ]]; then
  podman build -f "$root/containers/timesfm/Dockerfile" -t "$image" "$root/containers/timesfm"
fi

cid=$(podman run --rm -d -p 18080:8000 \
  -e TIMESFM_EAGER_LOAD=false "$image")
trap 'podman rm -f "$cid" >/dev/null 2>&1 || true' EXIT

for _ in $(seq 1 30); do
  if curl --silent --fail http://127.0.0.1:18080/healthz >/tmp/timesfm-health.json; then
    cat /tmp/timesfm-health.json
    echo
    echo "TimesFM service smoke test completed successfully."
    exit 0
  fi
  sleep 1
done

echo "TimesFM service did not become healthy in 30s" >&2
exit 1
