#!/usr/bin/env bash
# Prepares a release: creates the branch release/v<version> and bumps every app version.
#
#   scripts/release.sh 1.0.0
#   scripts/release.sh 1.0.0-rc.1
#
# - Cargo: [workspace.package] version in clients/ and server/, [package] version in
#   shared/protocol, and the workspace entries in each Cargo.lock
# - iOS (clients/mobile/iosApp/project.yml): MARKETING_VERSION = <version> without any
#   pre-release suffix (App Store versions must be numeric), CURRENT_PROJECT_VERSION + 1
# - Android (clients/mobile/androidApp/build.gradle.kts): versionName = <version>,
#   versionCode + 1
#
# The changes are left uncommitted on the new branch for review.
set -euo pipefail

usage() {
    echo "usage: $0 <version>   e.g. $0 1.0.0 or $0 1.0.0-rc.1" >&2
    exit 1
}

[[ $# -eq 1 ]] || usage
version="${1#v}"
if [[ ! "$version" =~ ^([0-9]+\.[0-9]+\.[0-9]+)(-[0-9A-Za-z.-]+)?$ ]]; then
    echo "error: '$1' is not a version like 1.0.0 or 1.0.0-rc.1" >&2
    exit 1
fi
numeric_version="${BASH_REMATCH[1]}"
branch="release/v$version"

root="$(git rev-parse --show-toplevel)"
cd "$root"

# if [[ -n "$(git status --porcelain)" ]]; then
#     echo "error: the working tree has uncommitted changes; commit or stash them first" >&2
#     exit 1
# fi
if git show-ref --verify --quiet "refs/heads/$branch"; then
    echo "error: branch $branch already exists" >&2
    exit 1
fi

# Sets the first `version = "..."` line inside the given TOML section.
set_toml_version() {
    local file="$1" section="$2" tmp
    tmp="$(mktemp)"
    awk -v section="[$section]" -v version="$version" '
        /^\[/ { in_section = ($0 == section) }
        in_section && !done && /^version[[:space:]]*=/ {
            print "version = \"" version "\""
            done = 1
            next
        }
        { print }
        END { if (!done) exit 1 }
    ' "$file" >"$tmp" || {
        rm -f "$tmp"
        echo "error: no version in [$section] of $file" >&2
        exit 1
    }
    cat "$tmp" >"$file"
    rm -f "$tmp"
    echo "  $file: $version"
}

# Replaces a single regex match in a file; fails if it does not match exactly once.
replace_once() {
    local file="$1" pattern="$2" replacement="$3" count tmp
    count="$(grep -cE "$pattern" "$file" || true)"
    if [[ "$count" -ne 1 ]]; then
        echo "error: expected one match of /$pattern/ in $file, found $count" >&2
        exit 1
    fi
    # No `sed -i`: BSD (macOS) and GNU sed disagree on its argument.
    tmp="$(mktemp)"
    sed -E "s/$pattern/$replacement/" "$file" >"$tmp" || {
        rm -f "$tmp"
        echo "error: could not edit $file" >&2
        exit 1
    }
    cat "$tmp" >"$file"
    rm -f "$tmp"
}

ios_spec="clients/mobile/iosApp/project.yml"
android_gradle="clients/mobile/androidApp/build.gradle.kts"

# Read the current build numbers before touching anything.
ios_build="$(sed -nE 's/^[[:space:]]*CURRENT_PROJECT_VERSION:[[:space:]]*"?([0-9]+)"?.*/\1/p' "$ios_spec")"
android_code="$(sed -nE 's/.*versionCode = .*\?: ([0-9]+).*/\1/p' "$android_gradle")"
[[ "$ios_build" =~ ^[0-9]+$ ]] || { echo "error: no CURRENT_PROJECT_VERSION in $ios_spec" >&2; exit 1; }
[[ "$android_code" =~ ^[0-9]+$ ]] || { echo "error: no versionCode in $android_gradle" >&2; exit 1; }
ios_build=$((ios_build + 1))
android_code=$((android_code + 1))

git switch -c "$branch"
echo "Created branch $branch"

echo "Cargo:"
set_toml_version clients/Cargo.toml workspace.package
set_toml_version server/Cargo.toml workspace.package
set_toml_version shared/protocol/Cargo.toml package
for dir in clients server shared/protocol; do
    # Refresh only the workspace's own entries in the lock file; dependencies stay as they are.
    (cd "$dir" && cargo update --workspace --offline --quiet) ||
        echo "  warning: could not update $dir/Cargo.lock; run 'cargo update --workspace' there" >&2
done

echo "iOS ($ios_spec):"
replace_once "$ios_spec" '^([[:space:]]*MARKETING_VERSION:[[:space:]]*).*$' "\\1\"$numeric_version\""
replace_once "$ios_spec" '^([[:space:]]*CURRENT_PROJECT_VERSION:[[:space:]]*).*$' "\\1\"$ios_build\""
echo "  MARKETING_VERSION: $numeric_version, CURRENT_PROJECT_VERSION: $ios_build"

echo "Android ($android_gradle):"
replace_once "$android_gradle" '(versionCode = .*\?: )[0-9]+' "\\1$android_code"
replace_once "$android_gradle" '(versionName = .*\?: )"[^"]*"' "\\1\"$version\""
echo "  versionName: $version, versionCode: $android_code"

echo
git --no-pager diff --stat
echo
echo "Review the changes, then:"
echo "  git commit -am \"chore: release v$version\""
echo "  git push -u origin $branch"
