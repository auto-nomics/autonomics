#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: test_deseq2_data_boundary.sh

Confirms that the DESeq2 image carries no pasilla fixture and declares no need
for a reference-panel mount.

Environment:
  DESEQ2_IMAGE  Image tag (default localhost/atc/deseq2:1.50.2)
EOF
}

image=${DESEQ2_IMAGE:-localhost/atc/deseq2:1.50.2}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

command -v podman >/dev/null || {
  echo "missing required command: podman" >&2
  exit 1
}

podman run \
  --rm \
  --network=none \
  --read-only \
  --security-opt=no-new-privileges \
  --userns=keep-id \
  "--user=1000:1000" \
  "--tmpfs=/tmp:rw,nosuid,nodev" \
  --entrypoint sh \
  "$image" -c '
set -eu
test ! -d /panels
if find /work /opt /tmp -type f \
  \( -name "pasilla_gene_counts.tsv" \
  -o -name "pasilla_sample_metadata.tsv" \
  -o -name "pasilla_baseline.json" \) \
  | grep -q .; then
  echo "DESeq2 image unexpectedly contains pasilla fixture data" >&2
  exit 1
fi
'

echo "DESeq2 data boundary validated successfully."
