#!/usr/bin/env bash
# Fails when the production part of a server source file (everything before its first
# `#[cfg(test)]`) is longer than the limit. Unit-test files named `tests.rs` are skipped.
# See G2 in docs/todo/server-refactoring.md.
#
# Files that are already too long are listed in `long-files.allow` with the length they may not
# grow past and the finding that splits them. Lower or remove an entry once a file shrinks.
set -euo pipefail

LIMIT=${LIMIT:-500}
cd "$(dirname "$0")/.."
ALLOW=scripts/long-files.allow

allowed() {
  awk -v file="$1" '$1 == file { print $2 }' "$ALLOW"
}

status=0
while IFS= read -r file; do
  length=$(awk '/^#\[cfg\(test\)\]/ { exit } { n++ } END { print n + 0 }' "$file")
  limit=$(allowed "$file")
  limit=${limit:-$LIMIT}
  if (( length > limit )); then
    echo "$file: $length production lines (limit $limit)" >&2
    status=1
  elif [[ -n $(allowed "$file") ]] && (( length <= LIMIT )); then
    echo "$file: $length production lines, now within $LIMIT; remove it from $ALLOW" >&2
    status=1
  fi
done < <(git ls-files --cached --others --exclude-standard 'crates/*/src/*.rs' 'crates/*/src/**/*.rs' |
  grep -v '/tests\.rs$' | sort -u)
exit $status
