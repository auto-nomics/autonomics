#!/usr/bin/env bash
# =============================================================================
# ensure-secret.sh — idempotent placeholder writer for .env / .env-style files
# =============================================================================
# Usage:  ensure-secret.sh <KEY> <ENV_FILE>
#
# Behavior:
#   • KEY present with non-empty value  →  no-op, silent
#   • KEY present with empty value     →  in-place sed-replace, log "patched"
#   • KEY absent (line not found)      →  append new line, log "appended"
#
# Idempotent: re-running with a healthy .env is a silent no-op.
# Side-effect safe: leaves every other line of the file byte-identical.
#
# Why not just sed once?
#   sed s/... works fine for replace; for append we need printf + >>. The
#   dispatch between those two paths is easier to read here than in a Makefile
#   recipe, so this script keeps the Makefile small.
# =============================================================================

set -euo pipefail

if [ "${#}" -lt 2 ] || [ "${#}" -gt 3 ]; then
  echo "Usage: $(basename "$0") <KEY> <ENV_FILE> [TOKEN_TYPE=hex|base64]" >&2
  exit 2
fi

KEY="$1"
ENV_FILE="$2"
TOKEN_TYPE="${3:-hex}"

# Sanity: ENV_FILE must exist. The Makefile guard (check-env) covers the
# normal flow; this is defense-in-depth.
if [ ! -f "${ENV_FILE}" ]; then
  echo "ERROR: ${ENV_FILE} not found" >&2
  exit 1
fi

# Already-set, non-empty: silent, idempotent.
VAL=$(grep "^${KEY}=" "${ENV_FILE}" | cut -d= -f2- || true)
if [ -n "${VAL}" ]; then
  exit 0
fi

# Generate a strong random value. Default 64 hex chars (32 bytes).
if [ "${TOKEN_TYPE}" = "hex" ]; then
  NEW=$(openssl rand -hex 32)
else
  NEW=$(openssl rand -base64 32)
fi

# Decide: replace in place vs. append.
if grep -q "^${KEY}=" "${ENV_FILE}"; then
  sed -i "s|^${KEY}=.*|${KEY}=${NEW}|" "${ENV_FILE}"
  echo ">>> Patched empty ${KEY} in ${ENV_FILE}"
else
  printf '\n%s=%s\n' "${KEY}" "${NEW}" >>"${ENV_FILE}"
  echo ">>> Generated ${KEY} and appended to ${ENV_FILE}"
fi
