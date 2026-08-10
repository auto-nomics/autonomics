#!/bin/bash
# =============================================================================
# garage-warehouse-setup.sh — idempotent Garage S3 warehouse bootstrap
# =============================================================================
# Ensures the object-storage warehouse (bucket + access key) matches .env
# before the data lake (Iceberg / Lakekeeper) relies on it. Safe to re-run:
# every step is check-then-act and no-ops when the state is already correct.
#
# Driver:  docker exec garage /garage <subcommand>
#   The dxflrs/garage image is shell-less, but `docker exec` execs the garage
#   binary directly — no shell inside the container, no auth-header juggling,
#   and the CLI matches the running binary's version exactly.
#
# Source of truth = .env:
#   GARAGE_ACCESS_KEY_ID      the access key ID Garage must expose
#   GARAGE_SECRET_ACCESS_KEY  the corresponding secret
#   ICEBERG_S3_BUCKET         the warehouse bucket (default: datalake)
#
# LIMITATION on key secrets:
#   The admin API cannot read a key's secret back, so an existing key ID is
#   assumed to match .env (we only import when the ID is absent). If you
#   rotate the secret in .env, first delete the key (garage key delete <id>)
#   and re-run this script.
#
# Usage:  garage-warehouse-setup.sh [env_file]
#   env_file : .env to source (default: ../../.env relative to this script)
# =============================================================================

set -euo pipefail

# --- locate default env file relative to this script -------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="${1:-${SCRIPT_DIR}/../../.env}"

if [ ! -f "${ENV_FILE}" ]; then
  echo "ERROR: env file not found: ${ENV_FILE}" >&2
  exit 1
fi

# --- load .env (source of truth) ---------------------------------------------
set -a
# shellcheck source=/dev/null
. "${ENV_FILE}"
set +a

BUCKET="${ICEBERG_S3_BUCKET:-datalake}"
KEY_ID="${GARAGE_ACCESS_KEY_ID:-}"
KEY_SECRET="${GARAGE_SECRET_ACCESS_KEY:-}"

if [ -z "${KEY_ID}" ] || [ -z "${KEY_SECRET}" ]; then
  echo "ERROR: GARAGE_ACCESS_KEY_ID / GARAGE_SECRET_ACCESS_KEY must be set in ${ENV_FILE}" >&2
  exit 1
fi

# --- helpers ------------------------------------------------------------------
# garage logs INFO/RPC lines to stderr; strip them so stdout stays parseable.
g()   { docker exec garage /garage "$@"; }
gout() { g "$@" 2>/dev/null; }
step() { echo ">>> $*"; }

# --- step 1: wait for a healthy cluster --------------------------------------
wait_garage_healthy() {
  step "Waiting for garage cluster to be healthy..."
  for i in $(seq 1 30); do
    if h="$(gout json-api GetClusterHealth 2>/dev/null)"; then
      case "$h" in
        *'"status":"healthy"'* | *'"status": "healthy"'*) return 0 ;;
      esac
    fi
    if [ $((i % 5)) -eq 0 ]; then echo "  ...still waiting (${i}/30)"; fi
    sleep 2
  done
  echo "ERROR: garage cluster not healthy after 60s" >&2
  exit 1
}

# --- step 2: ensure the warehouse bucket exists -------------------------------
ensure_bucket() {
  if gout bucket info "${BUCKET}" >/dev/null 2>&1; then
    step "Bucket '${BUCKET}' already exists."
  else
    step "Creating bucket '${BUCKET}'..."
    g bucket create "${BUCKET}"
  fi
}

# --- step 3: ensure the access key matches .env -------------------------------
ensure_key() {
  if gout key info "${KEY_ID}" >/dev/null 2>&1; then
    step "Access key '${KEY_ID}' already exists (secret assumed to match .env)."
  else
    step "Importing access key '${KEY_ID}' (id + secret from .env)..."
    # import (not create): pins the ID+secret to exactly what .env declares.
    g key import --yes "${KEY_ID}" "${KEY_SECRET}"
  fi
}

# --- step 4: grant read/write/owner on the bucket -----------------------------
# bucket allow is additive/idempotent, so it can run unconditionally.
ensure_key_bucket_perms() {
  step "Granting read/write/owner on '${BUCKET}' to '${KEY_ID}'..."
  g bucket allow --read --write --owner "${BUCKET}" --key "${KEY_ID}"
}

# --- report --------------------------------------------------------------------
report() {
  echo ""
  echo "=== Warehouse ready ==="
  echo "  Bucket  : ${BUCKET}"
  echo "  Key     : ${KEY_ID}"
  echo "  S3 API  : http://localhost:3900   (garage:3900 on the docker network)"
  echo ""
  echo "--- Bucket info ---"
  gout bucket info "${BUCKET}" || true
  echo ""
  echo "--- Key info ---"
  gout key info "${KEY_ID}" || true
}

# --- main ----------------------------------------------------------------------
wait_garage_healthy
ensure_bucket
ensure_key
ensure_key_bucket_perms
report
