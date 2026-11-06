#!/usr/bin/env bash
# Builds dist/gentle-shell-gpui-<version>-linux-<arch>.tar.gz containing
# gentle-shell-gpui-<version>/{gentle-shell-gpui,gentle-shell-gpui.desktop,gentle-shell-gpui.png}.
#
#   packaging/linux/package.sh x86_64-unknown-linux-gnu 0.1.0
#
# Set GENTLE_BIN=<path> to package an already-built binary instead of running
# `cargo build --profile dist --target <target-triple>` (the shipping profile).
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <target-triple> <version>" >&2
  exit 2
fi
target="$1"
version="$2"
case "$target" in
  x86_64-unknown-linux-gnu) arch=x86_64 ;;
  aarch64-unknown-linux-gnu) arch=aarch64 ;;
  *) echo "unsupported Linux target: $target" >&2; exit 2 ;;
esac

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

if [[ -n "${GENTLE_BIN:-}" ]]; then
  binary="$GENTLE_BIN"
else
  cargo build --profile dist --locked -p gentle-shell-gpui --target "$target"
  binary="target/$target/dist/gentle-shell-gpui"
fi
[[ -x "$binary" ]] || { echo "missing binary: $binary" >&2; exit 1; }

name="gentle-shell-gpui-$version"
stage="dist/$name"
rm -rf "$stage"
mkdir -p "$stage"
install -m 755 "$binary" "$stage/gentle-shell-gpui"
install -m 644 packaging/linux/gentle-shell-gpui.desktop "$stage/gentle-shell-gpui.desktop"
install -m 644 packaging/icons/icon-512.png "$stage/gentle-shell-gpui.png"

tarball="dist/$name-linux-$arch.tar.gz"
rm -f "$tarball"
tar -C dist -czf "$tarball" "$name"
echo "$tarball"
