#!/usr/bin/env bash
set -euo pipefail

repo=${AUTONOMICS_REPO:-auto-nomics/autonomics}
version=${AUTONOMICS_VERSION:-latest}
install_dir=${AUTONOMICS_INSTALL_DIR:-"$HOME/.local/bin"}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

case "$repo" in
  *[!A-Za-z0-9._/-]* | *[/.] | .* | *. | */ | */*/*) die "invalid AUTONOMICS_REPO: $repo" ;;
esac

case "$version" in
  latest) ;;
  *[!A-Za-z0-9._-]* | '') die "invalid AUTONOMICS_VERSION: $version" ;;
esac

os=$(uname -s)
machine=$(uname -m)

case "$os" in
  Linux) os=unknown-linux-gnu ;;
  Darwin) os=apple-darwin ;;
  *) die "unsupported operating system: $os" ;;
esac

case "$machine" in
  x86_64 | amd64) machine=x86_64 ;;
  aarch64 | arm64) machine=aarch64 ;;
  *) die "unsupported CPU architecture: $machine" ;;
esac

target="$machine-$os"
asset="autonomics-$target.tar.gz"

for command in curl tar install; do
  command -v "$command" >/dev/null 2>&1 || die "$command is required"
done

if command -v sha256sum >/dev/null 2>&1; then
  checksum_command=(sha256sum)
elif command -v shasum >/dev/null 2>&1; then
  checksum_command=(shasum -a 256)
else
  die "sha256sum or shasum is required"
fi

if [[ "$version" == latest ]]; then
  release_base="https://github.com/$repo/releases/latest/download"
else
  release_base="https://github.com/$repo/releases/download/$version"
fi

temporary_dir=$(mktemp -d "${TMPDIR:-/tmp}/autonomics-install.XXXXXX")
trap 'rm -rf "$temporary_dir"' EXIT

curl --fail --location --retry 3 --retry-delay 2 --silent --show-error \
  "$release_base/SHA256SUMS" -o "$temporary_dir/SHA256SUMS"
curl --fail --location --retry 3 --retry-delay 2 --silent --show-error \
  "$release_base/$asset" -o "$temporary_dir/$asset"

expected_checksum=$(awk -v file="$asset" '{gsub(/^\*/, "", $2)} $2 == file {print $1}' "$temporary_dir/SHA256SUMS")
actual_checksum=$("${checksum_command[@]}" "$temporary_dir/$asset" | awk '{print $1}')

[[ "$expected_checksum" =~ ^[[:xdigit:]]{64}$ ]] || die "checksum entry not found for $asset"
[[ "$actual_checksum" == "$expected_checksum" ]] || die "checksum mismatch for $asset"

tar -xzf "$temporary_dir/$asset" -C "$temporary_dir"
[[ -f "$temporary_dir/autonomics" ]] || die "archive does not contain autonomics"

mkdir -p "$install_dir"
install -m 0755 "$temporary_dir/autonomics" "$install_dir/autonomics.new"
mv -f "$install_dir/autonomics.new" "$install_dir/autonomics"

printf 'Installed autonomics %s to %s\n' "$version" "$install_dir/autonomics"
if [[ ":$PATH:" != *":$install_dir:"* ]]; then
  printf 'Add %s to PATH before running autonomics.\n' "$install_dir"
fi
