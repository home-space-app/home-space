#!/usr/bin/env bash
# Generates packaging/flatpak/cargo-sources.json from clients/Cargo.lock with
# flatpak-cargo-generator. flatpak-builder builds without network access, so every crate the lock
# file names is listed there for it to download beforehand. The file is not committed (it would go
# stale with every dependency bump); build-flatpak.sh and CI generate it before each build.
#
#   scripts/generate-flatpak-sources.sh [output]
#
# Needs git and python3 (uses uv when it is installed).
set -euo pipefail

desktop_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
clients_dir=$(dirname "$desktop_dir")
out=${1:-$desktop_dir/packaging/flatpak/cargo-sources.json}

tools=$(mktemp -d)
trap 'rm -rf "$tools"' EXIT
git clone --quiet --depth 1 https://github.com/flatpak/flatpak-builder-tools.git "$tools/fbt"
generator=$tools/fbt/cargo/flatpak-cargo-generator.py

if command -v uv >/dev/null; then
    (cd "$tools/fbt/cargo" && uv run --quiet python "$generator" -o "$out" "$clients_dir/Cargo.lock")
else
    python3 -m venv "$tools/venv"
    "$tools/venv/bin/pip" install --quiet 'aiohttp>=3.9.5,<4' 'PyYAML>=6.0.2,<7' 'tomlkit>=0.13.3,<1'
    "$tools/venv/bin/python" -I "$generator" -o "$out" "$clients_dir/Cargo.lock"
fi
echo "Wrote $out"
