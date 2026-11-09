#!/usr/bin/env bash
# Regenerates every checked-in icon from crates/app/src/shared/banner.rs.
# macOS only (AppKit renders the SVG; iconutil builds the .icns).
# CI never runs this: the outputs are committed.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

python3 "$here/generate.py"

# One render pass for every size anything needs.
sizes=(16 24 32 48 64 128 256 512 1024)
args=()
for size in "${sizes[@]}"; do
  args+=("$work/$size.png" "$size")
done
swift "$here/render.swift" "$here/icon.svg" "${args[@]}"

cp "$work/1024.png" "$here/icon-1024.png"
cp "$work/512.png" "$here/icon-512.png"

iconset="$work/AppIcon.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
  cp "$work/$size.png" "$iconset/icon_${size}x${size}.png"
  cp "$work/$((size * 2)).png" "$iconset/icon_${size}x${size}@2x.png"
done
iconutil --convert icns --output "$here/AppIcon.icns" "$iconset"

python3 "$here/ico.py" "$here/icon.ico" \
  "$work/16.png" "$work/24.png" "$work/32.png" "$work/48.png" \
  "$work/64.png" "$work/128.png" "$work/256.png"

echo "icons written to ${here#"$PWD"/}"
