#!/usr/bin/env bash
# Tests for verify_dist.sh (the publish job's last check before npm publish).
# Each case builds a small fake dist folder and a tag message, then checks the exit code.
# Usage: bash tools/olp-hook/qual/test_verify_dist.sh   (exit 0 = all cases pass)
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
V="$HERE/verify_dist.sh"
fail=0
pass=0

make_case() {   # $1 = folder; writes dist/ with two files + SHA256SUMS and a matching tag message
  mkdir -p "$1/dist"
  printf 'zip-bytes\n' > "$1/dist/olp-hook-linux-x64.tar.gz"
  printf 'tgz-bytes\n' > "$1/dist/feedpia-olp-0.1.2.tgz"
  (cd "$1/dist" && sha256sum olp-hook-linux-x64.tar.gz feedpia-olp-0.1.2.tgz > SHA256SUMS)
  { echo "tag-gate PASS"; echo; echo "SHA256SUMS:"; cat "$1/dist/SHA256SUMS"; echo; echo "rc_run: 12345"; } > "$1/tagmsg"
}

check() {   # $1 = case name, $2 = expected exit code, $3 = folder
  bash "$V" "$3/dist" "$3/tagmsg" >"$3/out.txt" 2>&1
  got=$?
  if [ "$got" = "$2" ]; then pass=$((pass + 1)); echo "ok   $1 (exit $got)"
  else fail=$((fail + 1)); echo "FAIL $1: expected exit $2, got $got"; sed 's/^/     /' "$3/out.txt"; fi
}

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

# T1: the SHA256SUMS block in the tag message differs from dist/SHA256SUMS by one byte.
make_case "$T/t1"
first="$(sed -n '4s/^\(.\).*/\1/p' "$T/t1/tagmsg")"   # line 4 = first hash line of the block
if [ "$first" = "0" ]; then new=1; else new=0; fi
sed -i "4s/^./$new/" "$T/t1/tagmsg"
cmp -s <(sed -n '4,5p' "$T/t1/tagmsg") "$T/t1/dist/SHA256SUMS" && { echo "FAIL T1 setup: block not changed"; fail=$((fail + 1)); }
check "T1 one byte differs" 1 "$T/t1"

# T2: the tag message has no SHA256SUMS block (for example a lightweight tag or a hand-made tag).
make_case "$T/t2"
printf 'v0.1.2\n' > "$T/t2/tagmsg"
check "T2 no block in tag message" 1 "$T/t2"

# T3: a dist file no longer matches its own SHA256SUMS line (the block and the file list still agree).
make_case "$T/t3"
printf 'changed\n' > "$T/t3/dist/feedpia-olp-0.1.2.tgz"
check "T3 dist file sha wrong" 1 "$T/t3"

# T4: everything matches.
make_case "$T/t4"
check "T4 same bytes" 0 "$T/t4"

# T5: dist has no SHA256SUMS at all.
make_case "$T/t5"
rm "$T/t5/dist/SHA256SUMS"
check "T5 no SHA256SUMS in dist" 1 "$T/t5"

# T6: dist has a file that SHA256SUMS does not list (it would be published without being checked).
make_case "$T/t6"
printf 'extra\n' > "$T/t6/dist/feedpia-olp-0.1.2-extra.tgz"
check "T6 file not in SHA256SUMS" 1 "$T/t6"

# T7: SHA256SUMS lists a file that is missing from dist.
make_case "$T/t7"
rm "$T/t7/dist/olp-hook-linux-x64.tar.gz"
check "T7 listed file missing" 1 "$T/t7"

# T8: the tag message has Windows line ends (CRLF); the block is still the same bytes after removing CR.
make_case "$T/t8"
sed -i 's/$/\r/' "$T/t8/tagmsg"
check "T8 CRLF tag message" 0 "$T/t8"

# T9: a second "SHA256SUMS:" header inside the block (an extra block pasted in) does not pass.
make_case "$T/t9"
sed -i '4i SHA256SUMS:' "$T/t9/tagmsg"
check "T9 second header inside block" 1 "$T/t9"

echo "passed=$pass failed=$fail"
[ "$fail" = 0 ]
