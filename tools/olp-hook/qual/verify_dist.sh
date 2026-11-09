#!/usr/bin/env bash
# Last check before npm publish: the files about to be published must be the files the tag gate tested.
# The tag gate writes the release candidate's SHA256SUMS into the annotated tag message, after a line "SHA256SUMS:".
# This script compares that block with dist/SHA256SUMS byte for byte, then checks every file against dist/SHA256SUMS.
# Usage: verify_dist.sh <dist folder> <tag message file>   exit 0 = same bytes, exit 1 = stop (never publish)
set -u
dist="${1:-}"; msg="${2:-}"
fail() { echo "::error::published bytes != tag-gate bytes: $1" >&2; exit 1; }

[ -n "$dist" ] && [ -n "$msg" ] || fail "usage: verify_dist.sh <dist folder> <tag message file>"
[ -f "$dist/SHA256SUMS" ] || fail "no SHA256SUMS in $dist"
[ -s "$dist/SHA256SUMS" ] || fail "empty SHA256SUMS in $dist"
[ -f "$msg" ] || fail "no tag message file $msg"

# Block = the lines after "SHA256SUMS:" up to the first blank line or end of message.
block="$(awk 'f && /^[[:space:]]*$/ {exit} f {print} /^SHA256SUMS:[[:space:]]*$/ {f=1}' "$msg" | tr -d '\r')"
[ -n "$block" ] || fail "no SHA256SUMS block in the tag message (lightweight or hand-made tag?)"
want="$(tr -d '\r' < "$dist/SHA256SUMS" | sed -e '/^[[:space:]]*$/d')"
[ "$block" = "$want" ] || {
  diff <(printf '%s\n' "$block") <(printf '%s\n' "$want") >&2
  fail "SHA256SUMS in the tag message differs from dist/SHA256SUMS"
}

# Every line must be "<64 hex>  <file>" and every listed file must match.
while IFS= read -r line; do
  [[ "$line" =~ ^[0-9a-f]{64}\ [\ *][^/]+$ ]] || fail "bad SHA256SUMS line: $line"
done <<< "$want"
# The files in dist (except SHA256SUMS) must be exactly the files listed: an unlisted file would be published unchecked.
listed="$(printf '%s\n' "$want" | sed -E 's/^[0-9a-f]{64} [ *]//' | LC_ALL=C sort)"
present="$(cd "$dist" && find . -maxdepth 1 -type f ! -name SHA256SUMS -printf '%f\n' | LC_ALL=C sort)"
[ "$listed" = "$present" ] || {
  diff <(printf '%s\n' "$listed") <(printf '%s\n' "$present") >&2
  fail "files in $dist differ from the SHA256SUMS list"
}
(cd "$dist" && sha256sum --strict -c SHA256SUMS) >&2 || fail "a file in $dist does not match SHA256SUMS"
echo "verify_dist: $(printf '%s\n' "$want" | wc -l) files match the tag-gate SHA256SUMS"
