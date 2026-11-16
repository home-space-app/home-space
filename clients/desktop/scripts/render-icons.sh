#!/usr/bin/env bash
# Renders packaging/home-space.svg to the PNG sizes of the freedesktop hicolor icon theme, in
# packaging/icons/hicolor/<size>x<size>/apps/home-space.png (the same layout as
# /usr/share/icons/hicolor). Desktop environments pick the closest size they need; KDE, for one,
# shows the generic icon in its panels and menus when only 256x256 is there.
#
# Needs rsvg-convert (librsvg: brew install librsvg, apt install librsvg2-bin).
# The deb/rpm assets in desktop/Cargo.toml and the Flatpak manifest list these sizes.
set -euo pipefail

sizes=(16 22 24 32 48 64 128 256 512)

packaging=$(cd "$(dirname "${BASH_SOURCE[0]}")/../packaging" && pwd)
command -v rsvg-convert >/dev/null || { echo "rsvg-convert is not installed" >&2; exit 1; }

for size in "${sizes[@]}"; do
    dir=$packaging/icons/hicolor/${size}x${size}/apps
    mkdir -p "$dir"
    rsvg-convert -w "$size" -h "$size" "$packaging/home-space.svg" -o "$dir/home-space.png"
    echo "$dir/home-space.png"
done
