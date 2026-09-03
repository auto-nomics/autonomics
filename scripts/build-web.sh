#!/usr/bin/env bash
# Build the autonomics-web SPA into apps/web/dist.
#
# The dist directory is embedded into the tui-http binary at compile time
# (rust-embed in debug mode reads from disk, release embeds it), so a Rust
# rebuild is only needed after this script produces new output.
set -euo pipefail
cd "$(dirname "$0")/.."

pnpm --dir apps/web install
pnpm --dir apps/web build
