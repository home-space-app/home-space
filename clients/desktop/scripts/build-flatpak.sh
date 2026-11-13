#!/usr/bin/env bash
# Builds the desktop client as a Flatpak bundle with flatpak-builder.
#
#   scripts/build-flatpak.sh [--install]
#
# Needs flatpak and flatpak-builder; the runtime, SDK and Rust extension come from Flathub
# (added as a user remote if missing). --install also installs the app for the current user,
# so `flatpak run io.github.indomitable.home-space` starts it.
# Output: clients/target/dist/flatpak/HomeSpace-<version>-linux-<arch>.flatpak
set -euo pipefail

install=false
for arg in "$@"; do
    case $arg in
        --install) install=true ;;
        *) echo "usage: $(basename "$0") [--install]" >&2; exit 1 ;;
    esac
done

app_id=io.github.indomitable.home-space
desktop_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
clients_dir=$(dirname "$desktop_dir")
manifest=$desktop_dir/packaging/flatpak/$app_id.yml
target_dir=${CARGO_TARGET_DIR:-$clients_dir/target}
work=$target_dir/flatpak
dist=$target_dir/dist/flatpak

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$clients_dir/Cargo.toml")
arch=$(flatpak --default-arch)

flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo

sources=$desktop_dir/packaging/flatpak/cargo-sources.json
if [[ ! -f $sources || $clients_dir/Cargo.lock -nt $sources ]]; then
    "$desktop_dir/scripts/generate-flatpak-sources.sh" "$sources"
fi

mkdir -p "$work" "$dist"
# The state dir lives next to the build so the source copy can skip it (see the manifest).
flatpak-builder --user --install-deps-from=flathub --force-clean --ccache \
    --state-dir="$work/state" --repo="$work/repo" \
    $($install && echo --install) \
    "$work/build" "$manifest"

bundle=$dist/HomeSpace-$version-linux-$arch.flatpak
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
    "$work/repo" "$bundle" "$app_id"
echo "Wrote $bundle"
