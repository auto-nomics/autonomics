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

# Prefer the canonical host location, but retain the legacy /mnt/data source for
# existing installations. Each selected source is mounted at the same path inside
# the container so bundle-relative paths remain valid.
AUTONOMICS_MIXER_SOURCE_ROOT="${AUTONOMICS_MIXER_RESOURCE_SOURCE:-}"
if [[ -z "$AUTONOMICS_MIXER_SOURCE_ROOT" ]]; then
    if [[ -f /data/mixer/resources/g1000_eur/bundle.json ]]; then
        AUTONOMICS_MIXER_SOURCE_ROOT=/data/mixer/resources
    else
        AUTONOMICS_MIXER_SOURCE_ROOT=/mnt/data/mixer/resources
    fi
fi
add_volume "$AUTONOMICS_MIXER_SOURCE_ROOT" "$AUTONOMICS_MIXER_SOURCE_ROOT" ro

PLINK_REF_SOURCE_ROOT="${AUTONOMICS_PLINK_REF_SOURCE_ROOT:-}"
if [[ -z "$PLINK_REF_SOURCE_ROOT" ]]; then
    for candidate in \
        /data/mixer/resources/g1000_eur/stage \
        /mnt/data/mixer/resources/g1000_eur/stage \
        /mnt/disk3/mixer/reference/mixer_data/stage \
        /mnt/disk2/dataset/1000g_plink/eur; do
        if [[ -d "$candidate" ]]; then
            PLINK_REF_SOURCE_ROOT="$candidate"
            break
        fi
    done
fi
if [[ -n "$PLINK_REF_SOURCE_ROOT" ]]; then
    add_volume "$PLINK_REF_SOURCE_ROOT" "$PLINK_REF_SOURCE_ROOT" ro
fi

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
    --env MIXER_RESOURCE_ROOT="$AUTONOMICS_MIXER_SOURCE_ROOT" \
    --env MIXER_PYTHON=/usr/bin/python3 \
    --env PLINK_REF_PREFIX_TEMPLATE="${PLINK_REF_PREFIX_TEMPLATE:-}" \
    --env "TERM=${TERM:-xterm-256color}" \
    "${volumes[@]}" \
    "$IMAGE" \
    "$@"
