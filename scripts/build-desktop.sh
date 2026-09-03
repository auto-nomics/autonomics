#!/usr/bin/env bash
# 打包 autonomics 桌面版（仅 Linux；deb + appimage）。
#
# 顺序：前端 dist 新鲜化（release 下 rust-embed 在 tui-http 编译期嵌入 dist）
#       → cargo tauri build（release 编译 + bundle）。
#
# 注意：tauri.conf.json 故意不配 beforeBuildCommand——dist 新鲜化由本脚本
# 保证；裸跑 `cargo tauri build` 会嵌入磁盘上现有的 dist（可能过期）。
# tauri CLI 会给钩子进程注入 TAURI_ENV（触发 vite base:'./'，SPA 深链刷新
# 会 404），本脚本在 tauri 之外运行，天然不受影响。
set -euo pipefail
cd "$(dirname "$0")/.."

./scripts/build-web.sh

if command -v cargo-tauri >/dev/null 2>&1; then
  cd apps/desktop && exec cargo tauri build
else
  pnpm --dir apps/desktop install
  exec pnpm --dir apps/desktop exec tauri build
fi
