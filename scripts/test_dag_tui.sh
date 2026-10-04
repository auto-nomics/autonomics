#!/usr/bin/env bash
set -euo pipefail

if [[ ! -t 0 ]]; then
  echo "This preview requires an interactive terminal." >&2
  exit 1
fi

cd "$(dirname "${BASH_SOURCE[0]}")/.."

args=(--quiet --package autonomics --bin dag_tui_preview)
if [[ "${AUTONOMICS_DAG_TUI_RELEASE:-0}" == "1" ]]; then
  args+=(--release)
fi

cargo run "${args[@]}"
