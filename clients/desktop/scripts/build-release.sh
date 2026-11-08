#!/usr/bin/env bash
# Builds the desktop client in release mode and packages it for one operating system.
#
#   scripts/build-release.sh <macos|linux|windows> [--universal]
#
#   macos    Home Space.app (ad-hoc signed) and a zip of it
#   linux    the binary with its .desktop entry and icons, as a tar.gz
#   windows  HomeSpace.exe, zipped when a zip tool is available (run from Git Bash/MSYS)
#
# --universal (macos only) builds for both Apple Silicon and Intel and merges them with lipo.
# CODESIGN_IDENTITY overrides the ad-hoc signature ("-") on macOS.
# Output: clients/target/dist/<os>/
set -euo pipefail

usage() {
    echo "usage: $(basename "$0") <macos|linux|windows> [--universal]" >&2
    exit 1
}

[[ $# -ge 1 ]] || usage
os=$1
shift
universal=false
for arg in "$@"; do
    case $arg in
        --universal) universal=true ;;
        *) usage ;;
    esac
done
case $os in
    macos | linux | windows) ;;
    *) usage ;;
esac
if $universal && [[ $os != macos ]]; then
    echo "--universal is only supported for macos" >&2
    exit 1
fi

desktop_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
clients_dir=$(dirname "$desktop_dir")
packaging=$desktop_dir/packaging
target_dir=${CARGO_TARGET_DIR:-$clients_dir/target}
dist=$target_dir/dist/$os

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$clients_dir/Cargo.toml")
[[ -n $version ]] || { echo "could not read the workspace version" >&2; exit 1; }

arch=$(uname -m)
case $arch in
    arm64 | aarch64) arch=aarch64 ;;
    x86_64 | amd64) arch=x86_64 ;;
esac

triple_for() {
    case $os in
        macos) echo "$1-apple-darwin" ;;
        linux) echo "$1-unknown-linux-gnu" ;;
        windows) echo "$1-pc-windows-msvc" ;;
    esac
}

# Builds for one target triple and prints the path of the binary.
build() {
    local triple=$1
    if command -v rustup >/dev/null && ! rustup target list --installed | grep -qx "$triple"; then
        echo "Rust target $triple is not installed: rustup target add $triple" >&2
        exit 1
    fi
    echo "Building $triple..." >&2
    (cd "$clients_dir" && cargo build --release -p desktop --features keyring --target "$triple" >&2)
    local exe=HomeSpace
    [[ $os == windows ]] && exe=HomeSpace.exe
    echo "$target_dir/$triple/release/$exe"
}

rm -rf "$dist"
mkdir -p "$dist"

case $os in
    macos)
        if $universal; then
            arm=$(build "$(triple_for aarch64)")
            intel=$(build "$(triple_for x86_64)")
            label=universal
        else
            bin=$(build "$(triple_for "$arch")")
            label=$arch
        fi

        app="$dist/Home Space.app"
        mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
        if $universal; then
            lipo -create "$arm" "$intel" -output "$app/Contents/MacOS/HomeSpace"
        else
            cp "$bin" "$app/Contents/MacOS/HomeSpace"
        fi
        cp "$packaging/home-space.icns" "$app/Contents/Resources/"

        # Mirrors [package.metadata.bundle] in desktop/Cargo.toml.
        cat >"$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>Home Space</string>
    <key>CFBundleExecutable</key>
    <string>HomeSpace</string>
    <key>CFBundleIconFile</key>
    <string>home-space</string>
    <key>CFBundleIdentifier</key>
    <string>io.github.indomitable.home-space</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>Home Space</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.productivity</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

        codesign --force --deep --sign "${CODESIGN_IDENTITY:--}" "$app"
        archive="$dist/HomeSpace-$version-macos-$label.zip"
        ditto -c -k --keepParent "$app" "$archive"
        echo "Built $app"
        ;;

    linux)
        bin=$(build "$(triple_for "$arch")")
        name=home-space-$version-linux-$arch
        stage=$dist/$name
        mkdir -p "$stage"
        cp "$bin" "$stage/HomeSpace"
        cp "$packaging/home-space.desktop" "$packaging/home-space.png" "$packaging/home-space.svg" "$stage/"
        archive=$dist/$name.tar.gz
        tar -czf "$archive" -C "$dist" "$name"
        ;;

    windows)
        bin=$(build "$(triple_for "$arch")")
        cp "$bin" "$dist/HomeSpace.exe"
        archive=$dist/HomeSpace-$version-windows-$arch.zip
        if command -v zip >/dev/null; then
            (cd "$dist" && zip -q "$(basename "$archive")" HomeSpace.exe)
        elif command -v powershell.exe >/dev/null; then
            powershell.exe -NoProfile -Command \
                "Compress-Archive -Force -Path '$(cygpath -w "$dist/HomeSpace.exe")' -DestinationPath '$(cygpath -w "$archive")'"
        else
            archive=$dist/HomeSpace.exe
        fi
        ;;
esac

echo "Wrote $archive"
