#!/bin/sh
# =============================================================================
# render-garage-config.sh — host-side garage.toml renderer
# =============================================================================
# dxflrs/garage:v2.0.0 is a static/distroless image: no /bin/sh, no bash,
# nothing the kernel can exec other than the garage binary itself. So we
# CANNOT run an entrypoint script inside the container. Instead we render
# garage.toml on the host and bind-mount the result.
#
# Usage: render-garage-config.sh <template> <output> [env_file]
#
#   template : path to garage.toml.template
#   output   : path to render to (commonly garage.toml, bind-mounted at
#              /etc/garage.toml inside the garage + webui containers)
#   env_file : source file for env vars (default: workspace ../../.env);
#              loaded with `set -a` so all KEY=VAL lines become shell vars
#
# Why a whitelist for envsubst?
#   The .env file holds secrets for many services (LLM keys, OSS tokens,
#   GARAGE_*, OPENGWAS_*, EMBASE_* ...). Passing every var to envsubst
#   would silently bake unrelated secrets into garage.toml. The whitelist
#   below restricts substitution to the keys garage actually consumes.
# =============================================================================

set -eu

if [ "${#}" -lt 2 ] || [ "${#}" -gt 3 ]; then
  echo "Usage: $(basename "$0") <template> <output> [env_file]" >&2
  exit 2
fi

TPL="$1"
OUT="$2"
ENV_FILE="${3:-../../.env}"

if [ ! -f "${TPL}" ]; then
  echo "ERROR: template not found: ${TPL}" >&2
  exit 1
fi

if [ ! -f "${ENV_FILE}" ]; then
  echo "ERROR: env file not found: ${ENV_FILE}" >&2
  exit 1
fi

# Whitelist of vars the template references. envsubst replaces only these.
WHITELIST='$GARAGE_RPC_SECRET $GARAGE_ADMIN_TOKEN $GARAGE_METRICS_TOKEN $GARAGE_RPC_PUBLIC_ADDR $GARAGE_DISK_CAPACITY'

# Load .env into the current shell. Keep `set -a` active so the defaulted
# vars below also auto-export (envsubst reads from the process environment,
# not just shell-local variables).
set -a
. "${ENV_FILE}"

# Apply sensible defaults for vars the .env may omit. `:=` is the POSIX
# way to set a default at use-site; envsubst itself only understands
# bare ${VAR}, so we have to populate values before calling it.
: "${GARAGE_RPC_PUBLIC_ADDR:=127.0.0.1:3901}"
: "${GARAGE_DISK_CAPACITY:=800G}"

set +a

envsubst "${WHITELIST}" < "${TPL}" > "${OUT}"
chmod 600 "${OUT}"
echo ">>> Rendered garage config: ${OUT}"
