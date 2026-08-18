#!/usr/bin/env bash

set -euo pipefail

if [[ $(id -u) -eq 0 ]]; then
    echo "Run this script without sudo so Podman can use your rootless user namespace." >&2
    exit 1
fi

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
CONTAINER_NAME="${AUTONOMICS_TUI_CONTAINER:-autonomics-tui}"
IMAGE="${AUTONOMICS_TUI_IMAGE:-localhost/autonomics-tui:local}"
STATE_DIR="${AUTONOMICS_TUI_STATE_DIR:-$HOME/.autonomics}"
CACHE_DIR="${OPENGWAS_CACHE_DIR:-$HOME/.cache/opengwas}"
HOST_ROOT="${AUTONOMICS_TUI_HOST_ROOT:-$HOME/.autonomics-tui}"

command -v podman >/dev/null || {
    echo "podman not found in PATH" >&2
    exit 1
}

# The older image tag is kept as a fallback for existing local installations.
if ! podman image exists "$IMAGE"; then
    if podman image exists localhost/atc:latest; then
        IMAGE=localhost/atc:latest
    else
        echo "Image $IMAGE not found; building it from $ROOT" >&2
        podman build -t "$IMAGE" "$ROOT"
    fi
fi

mkdir -p \
    "$HOST_ROOT/home" \
    "$HOST_ROOT/files" \
    "$HOST_ROOT/logs" \
    "$STATE_DIR" \
    "$CACHE_DIR"

declare -a volumes=()
declare -A seen_sources=()

add_volume() {
    local source="$1"
    local destination="$2"
    local mode="${3:-rw}"

    if [[ -n "${seen_sources[$source]+x}" ]]; then
        return
    fi
    if [[ ! -e "$source" ]]; then
        echo "WARNING: VFS source does not exist on the host, skipping: $source" >&2
        return
    fi

    seen_sources[$source]=1
    volumes+=("--volume" "${source}:${destination}:${mode}")
}

add_volume "$STATE_DIR" /data/state rw
add_volume "$HOST_ROOT/home" /data/home rw
add_volume "$CACHE_DIR" /data/home/.cache/opengwas rw
add_volume "$HOST_ROOT/files" /data/files rw
add_volume "$HOST_ROOT/logs" /app/logs rw

# These paths match the local sources in $STATE_DIR/vfs.toml. Keep the host
# and container paths identical so the manifest works without rewriting.
add_volume /mnt/disk3/test /mnt/disk3/test rw
add_volume \
    /mnt/projects/autonomics_projects/autonomics/reference/ldsc_data/parquet \
    /mnt/projects/autonomics_projects/autonomics/reference/ldsc_data/parquet ro
add_volume /mnt/disk3/ld_score /mnt/disk3/ld_score ro
add_volume \
    /mnt/disk3/autonomics_tree/port_sldsc/reference/ldsc_data \
    /mnt/disk3/autonomics_tree/port_sldsc/reference/ldsc_data ro
add_volume /mnt/projects/1000g_genotype_data /mnt/projects/1000g_genotype_data ro
add_volume /mnt/disk3/kegg_scraper/data/kegg_data /mnt/disk3/kegg_scraper/data/kegg_data ro
add_volume /mnt/data/go_data /mnt/data/go_data ro
add_volume /mnt/data/magma/data /mnt/data/magma/data ro
add_volume /mnt/data/magma/resources/genes /mnt/data/magma/resources/genes ro
add_volume \
    /mnt/data/magma/resources/references \
    /mnt/data/magma/resources/references ro
add_volume \
    /mnt/data/mixer/resources \
    /mnt/data/mixer/resources ro

if podman container exists "$CONTAINER_NAME"; then
    echo "Removing stale container: $CONTAINER_NAME" >&2
    podman rm --force "$CONTAINER_NAME" >/dev/null
fi

exec podman run \
    --rm \
    --interactive \
    --tty \
    --name "$CONTAINER_NAME" \
    --detach-keys=ctrl-] \
    --user "$(id -u):$(id -g)" \
    --userns=keep-id \
    --env HOME=/data/home \
    --env MIXER_RESOURCE_ROOT=/mnt/data/mixer/resources \
    --env MIXER_PYTHON=/usr/bin/python3 \
    --env "TERM=${TERM:-xterm-256color}" \
    "${volumes[@]}" \
    "$IMAGE" \
    "$@"
