#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_visualization.sh

Builds the visualization image and renders a small PNG.

Environment:
  VISUALIZATION_IMAGE  Image tag (default localhost/atc/visualization:0.1.0)
  BUILD_IMAGE=0        Skip podman build
EOF
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
image=${VISUALIZATION_IMAGE:-localhost/atc/visualization:0.1.0}
build_image=${BUILD_IMAGE:-1}

[[ "${1:-}" == "-h" || "${1:-}" == "--help" ]] && {
  usage
  exit 0
}

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required command: $1" >&2
    exit 1
  }
}

need podman

if [[ "$build_image" == 1 ]]; then
  podman build --network host -f "$root/containers/visualization/Dockerfile" \
    -t "$image" "$root/containers/visualization"
fi

work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
cat >"$work/data.csv" <<'EOF'
x,y
1,1
2,4
3,9
EOF
cat >"$work/plot.R" <<'EOF'
p <- ggplot2::ggplot(df, ggplot2::aes(x = x, y = y)) +
  ggplot2::geom_point() +
  ggplot2::geom_line()
EOF

podman run --rm \
  --network none \
  --read-only \
  --security-opt no-new-privileges \
  --tmpfs /tmp:rw,nosuid,nodev \
  --userns keep-id \
  --mount "type=bind,source=$work,destination=/work,rw=true" \
  --env AUTONOMICS_INPUT0=/work/data.csv \
  --env AUTONOMICS_INPUT1=/work/plot.R \
  --env AUTONOMICS_OUTPUT0=/work/plot.png \
  --env AUTONOMICS_VISUALIZATION_DATA_FORMAT=csv \
  --env AUTONOMICS_VISUALIZATION_WIDTH=5 \
  --env AUTONOMICS_VISUALIZATION_HEIGHT=4 \
  --env AUTONOMICS_VISUALIZATION_DPI=100 \
  "$image" /opt/autonomics/render.R >/dev/null

signature=$(od -An -tx1 -N4 "$work/plot.png" | tr -d ' \n')
[[ "$signature" == "89504e47" ]] || {
  echo "output is not a PNG (signature: $signature)" >&2
  exit 1
}

echo "Visualization container test completed successfully."
