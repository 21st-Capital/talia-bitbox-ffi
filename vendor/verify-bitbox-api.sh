#!/bin/bash
# Re-derive vendor/bitbox-api from what crates.io published plus vendor/patches,
# and fail on any difference. Read-only: it writes only to a temporary folder.
# See vendor/README.md.
set -euo pipefail
shopt -s nullglob

VERSION=0.13.0
SHA256=d6fbc54fa856e199b598cea738c9eeb2d8a3c891617e3bef3a970c2374c29baf
URL="https://static.crates.io/crates/bitbox-api/bitbox-api-$VERSION.crate"

VENDOR="$(cd "$(dirname "$0")" && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

curl -sSfL --proto '=https' --tlsv1.2 -o "$TMP/bitbox-api.crate" "$URL"
GOT="$(shasum -a 256 "$TMP/bitbox-api.crate" | cut -d' ' -f1)"
if [ "$GOT" != "$SHA256" ]; then
  echo "✗ $URL has SHA-256 $GOT, expected $SHA256." >&2
  exit 1
fi
mkdir "$TMP/unpacked"
tar -xzf "$TMP/bitbox-api.crate" -C "$TMP/unpacked"
DERIVED="$TMP/unpacked/bitbox-api-$VERSION"

for PATCH in "$VENDOR"/patches/*.patch; do
  # The ceiling keeps git from finding a repository above the unpacked
  # package: inside one, `git apply` silently skips paths outside the
  # current folder instead of patching them.
  (cd "$DERIVED" && GIT_CEILING_DIRECTORIES="$TMP/unpacked" git apply --whitespace=nowarn "$PATCH")
  echo "applied $(basename "$PATCH")"
done

if diff -r "$DERIVED" "$VENDOR/bitbox-api"; then
  echo "✓ vendor/bitbox-api is bitbox-api $VERSION from crates.io plus vendor/patches, nothing else."
else
  echo "✗ vendor/bitbox-api differs from bitbox-api $VERSION plus vendor/patches (see above)." >&2
  exit 1
fi
