#!/usr/bin/env bash
# Builds a signed release APK of the Android app.
#
#   scripts/build-android-apk.sh --keystore release.jks --alias homespace [--out app.apk]
#
# Every option can come from an environment variable instead (options win):
#
#   --keystore <path>       ANDROID_KEYSTORE           keystore file (.jks / .p12)
#   --alias <name>          ANDROID_KEY_ALIAS          key alias in the keystore
#   --store-password <pw>   ANDROID_KEYSTORE_PASSWORD  keystore password
#   --key-password <pw>     ANDROID_KEY_PASSWORD       key password (default: keystore password)
#   --out <path>            ANDROID_APK_OUT            output APK
#                                                      (default: dist/homespace-<version>.apk)
#
# If no password is given, the script prompts for it. Prefer the environment variables (or the
# prompt) over the password options: options are visible in the process list and shell history.
#
# Gradle builds the unsigned release APK; it is then zipaligned and signed with apksigner from the
# newest installed Android SDK build-tools. The passwords reach apksigner through the environment,
# never its command line.
set -euo pipefail

usage() {
    sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit "${1:-1}"
}

keystore="${ANDROID_KEYSTORE:-}"
alias="${ANDROID_KEY_ALIAS:-}"
store_password="${ANDROID_KEYSTORE_PASSWORD:-}"
key_password="${ANDROID_KEY_PASSWORD:-}"
out="${ANDROID_APK_OUT:-}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --keystore) keystore="${2:?--keystore needs a value}"; shift 2 ;;
        --alias) alias="${2:?--alias needs a value}"; shift 2 ;;
        --store-password) store_password="${2:?--store-password needs a value}"; shift 2 ;;
        --key-password) key_password="${2:?--key-password needs a value}"; shift 2 ;;
        --out) out="${2:?--out needs a value}"; shift 2 ;;
        -h|--help) usage 0 ;;
        *) echo "error: unknown option '$1'" >&2; usage ;;
    esac
done

[[ -n "$keystore" ]] || { echo "error: no keystore (--keystore or ANDROID_KEYSTORE)" >&2; usage; }
[[ -n "$alias" ]] || { echo "error: no key alias (--alias or ANDROID_KEY_ALIAS)" >&2; usage; }
[[ -f "$keystore" ]] || { echo "error: keystore '$keystore' not found" >&2; exit 1; }
keystore="$(cd "$(dirname "$keystore")" && pwd)/$(basename "$keystore")"

if [[ -z "$store_password" ]]; then
    [[ -t 0 ]] || { echo "error: no keystore password (ANDROID_KEYSTORE_PASSWORD)" >&2; exit 1; }
    read -rsp "Keystore password: " store_password; echo >&2
fi
key_password="${key_password:-$store_password}"

repo="$(cd "$(dirname "$0")/.." && pwd)"
mobile="$repo/clients/mobile"

# The SDK: ANDROID_HOME, ANDROID_SDK_ROOT, local.properties, or the default install location.
sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [[ -z "$sdk" && -f "$mobile/local.properties" ]]; then
    sdk="$(sed -n 's/^sdk\.dir=//p' "$mobile/local.properties" | tail -1)"
fi
if [[ -z "$sdk" ]]; then
    case "$(uname -s)" in
        Darwin) sdk="$HOME/Library/Android/sdk" ;;
        *) sdk="$HOME/Android/Sdk" ;;
    esac
fi
build_tools="$(ls -d "$sdk"/build-tools/*/ 2>/dev/null | sort -V | tail -1)"
build_tools="${build_tools%/}"
if [[ -z "$build_tools" || ! -x "$build_tools/apksigner" || ! -x "$build_tools/zipalign" ]]; then
    echo "error: no Android SDK build-tools with apksigner and zipalign under '$sdk'" >&2
    exit 1
fi

version="$(sed -n 's/.*homespace.versionName").orNull ?: "\(.*\)".*/\1/p' \
    "$mobile/androidApp/build.gradle.kts")"
out="${out:-$repo/dist/homespace-${version:-release}.apk}"
mkdir -p "$(dirname "$out")"

echo "==> Building the release APK"
(cd "$mobile" && ./gradlew :androidApp:assembleRelease)

unsigned="$mobile/androidApp/build/outputs/apk/release/androidApp-release-unsigned.apk"
[[ -f "$unsigned" ]] || unsigned="$(ls "$mobile"/androidApp/build/outputs/apk/release/*.apk | head -1)"
[[ -f "$unsigned" ]] || { echo "error: Gradle produced no release APK" >&2; exit 1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "==> Aligning"
"$build_tools/zipalign" -f -p 4 "$unsigned" "$tmp/aligned.apk"

echo "==> Signing with '$alias' from $keystore"
HS_STORE_PASS="$store_password" HS_KEY_PASS="$key_password" "$build_tools/apksigner" -J-enable-native-access=ALL-UNNAMED sign \
    --ks "$keystore" \
    --ks-key-alias "$alias" \
    --ks-pass env:HS_STORE_PASS \
    --key-pass env:HS_KEY_PASS \
    --out "$out" \
    "$tmp/aligned.apk"
rm -f "$out.idsig"

"$build_tools/apksigner" -J-enable-native-access=ALL-UNNAMED verify "$out"
echo "==> Signed APK: $out"
