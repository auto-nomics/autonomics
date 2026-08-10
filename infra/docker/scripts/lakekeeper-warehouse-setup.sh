#!/bin/bash
# =============================================================================
# lakekeeper-warehouse-setup.sh — idempotent Lakekeeper → Garage warehouse link
# =============================================================================
# Ensures Lakekeeper has a Warehouse whose storage points at the Garage S3
# bucket from .env. Safe to re-run: it lists existing warehouses and only
# creates the missing one.
#
# Why this exists:
#   The Iceberg REST catalog URI (ICEBERG_REST_URI, e.g. http://…:8181/catalog)
#   routes /catalog/{namespace}/… to a Lakekeeper *Warehouse* named "catalog".
#   Without that warehouse, Iceberg clients get 404s. This script creates it
#   once, wired to the Garage warehouse the garage-warehouse-setup.sh script
#   guarantees (bucket + access key).
#
# Source of truth = .env:
#   LAKEKEEPER_WAREHOUSE_NAME  explicit warehouse name (if unset, derived from
#                              the last path segment of ICEBERG_REST_URI)
#   ICEBERG_REST_URI           falls back to deriving the warehouse name
#   ICEBERG_S3_BUCKET          the Garage bucket (datalake)
#   GARAGE_ACCESS_KEY_ID       Garage access key
#   GARAGE_SECRET_ACCESS_KEY   Garage secret
#   LAKEKEEPER_MANAGEMENT_TOKEN  optional bearer token (empty under allow-all)
#
# Transport:
#   curl against http://localhost:8181/management/v1 — the running Lakekeeper's
#   management API (spec at /api-docs/management/v1/openapi.json). With the
#   `allowall` authz backend configured in docker-compose.yml, warehouse CRUD
#   needs no token.
#
# Usage:  lakekeeper-warehouse-setup.sh [env_file]
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

API="http://localhost:8181/management/v1"
TOKEN="${LAKEKEEPER_MANAGEMENT_TOKEN:-}"

# Warehouse name: explicit var wins, else derive from the Iceberg REST URI's
# last path segment. Must match the REST URI prefix so clients route correctly.
REST_URI="${ICEBERG_REST_URI:-http://localhost:8181/catalog}"
WAREHOUSE_NAME="${LAKEKEEPER_WAREHOUSE_NAME:-$(basename "${REST_URI%/}")}"
WAREHOUSE_NAME="${WAREHOUSE_NAME:-catalog}"

BUCKET="${ICEBERG_S3_BUCKET:-datalake}"
# Lakekeeper talks to garage over the docker network, not localhost.
S3_ENDPOINT_INTERNAL="${ICEBERG_S3_ENDPOINT_INTERNAL:-http://garage:3900}"
REGION="${ICEBERG_S3_REGION:-garage}"
KEY_ID="${GARAGE_ACCESS_KEY_ID:-}"
KEY_SECRET="${GARAGE_SECRET_ACCESS_KEY:-}"

if [ -z "${KEY_ID}" ] || [ -z "${KEY_SECRET}" ]; then
  echo "ERROR: GARAGE_ACCESS_KEY_ID / GARAGE_SECRET_ACCESS_KEY must be set in ${ENV_FILE}" >&2
  exit 1
fi

# --- helpers ------------------------------------------------------------------
step() { echo ">>> $*"; }
curl_mgmt() {
  # curl_mgmt [METHOD] [path] [json_body]
  local method="${1:-GET}" path="$2" body="${3:-}"
  if [ -n "${body}" ]; then
    curl -sf -X "${method}" -H "Content-Type: application/json" ${TOKEN:+-H "Authorization: Bearer ${TOKEN}"} \
      -d "${body}" "${API}${path}"
  else
    curl -sf -X "${method}" ${TOKEN:+-H "Authorization: Bearer ${TOKEN}"} "${API}${path}"
  fi
}

# --- step 1: wait for Lakekeeper to be up and bootstrapped ---------------------
wait_lakekeeper() {
  step "Waiting for Lakekeeper to be ready..."
  for i in $(seq 1 30); do
    if info="$(curl -sf "${API}/info" 2>/dev/null)"; then
      if printf '%s' "${info}" | jq -e '.bootstrapped == true' >/dev/null 2>&1; then
        step "  Lakekeeper ready (v$(printf '%s' "${info}" | jq -r '.version // "?"'))."
        return 0
      fi
    fi
    if [ $((i % 5)) -eq 0 ]; then echo "  ...still waiting (${i}/30)"; fi
    sleep 2
  done
  echo "ERROR: Lakekeeper not ready after 60s" >&2
  exit 1
}

# --- step 2: ensure the warehouse exists (check-then-create) -------------------
ensure_warehouse() {
  local list exists
  step "Checking for warehouse '${WAREHOUSE_NAME}'..."
  list="$(curl_mgmt GET /warehouse)"
  exists="$(printf '%s' "${list}" | jq -r --arg n "${WAREHOUSE_NAME}" \
    '[.warehouses[] | select(.name == $n)] | length')"

  if [ "${exists}" != "0" ]; then
    step "  Warehouse '${WAREHOUSE_NAME}' already exists."
    return 0
  fi

  step "  Creating warehouse '${WAREHOUSE_NAME}' → garage bucket '${BUCKET}' at ${S3_ENDPOINT_INTERNAL}..."
  local body
  body="$(jq -nc \
    --arg name "${WAREHOUSE_NAME}" \
    --arg bucket "${BUCKET}" \
    --arg region "${REGION}" \
    --arg endpoint "${S3_ENDPOINT_INTERNAL}" \
    --arg key "${KEY_ID}" \
    --arg secret "${KEY_SECRET}" \
    '{
      "warehouse-name": $name,
      "storage-profile": {
        "type": "s3",
        "bucket": $bucket,
        "region": $region,
        "endpoint": $endpoint,
        "path-style-access": true,
        "sts-enabled": false,
        "flavor": "s3-compat"
      },
      "storage-credential": {
        "type": "s3",
        "credential-type": "access-key",
        "access-key-id": $key,
        "secret-access-key": $secret
      }
    }')"
  curl_mgmt POST /warehouse "${body}" >/dev/null
  step "  Created."
}

# --- report --------------------------------------------------------------------
report() {
  local list
  list="$(curl_mgmt GET /warehouse)"
  echo ""
  echo "=== Lakekeeper warehouse(s) ==="
  printf '%s' "${list}" | jq -r '.warehouses[] | "  • " + .name + "  status=" + .status + "  profile=" + (."storage-profile".type // "?")' || true
  echo "  REST catalog URI: ${REST_URI}"
}

# --- main ----------------------------------------------------------------------
wait_lakekeeper
ensure_warehouse
report
