#!/usr/bin/env bash
# check-target-leftovers.sh names a file that a test copied under target/ with
# its old mtime kept, and ignores what cargo writes in a profile dir.
set -euo pipefail
cd "$(dirname "$0")/.."

root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/scripts" "$root/target/debug/build" "$root/target/planted"
cp scripts/check-target-leftovers.sh "$root/scripts/"
touch "$root/target/debug/.cargo-lock"
echo fixture >"$root/fixture"
touch -t 202001010000 "$root/fixture"
touch -t 202401010000 "$root/marker"
cp -p "$root/fixture" "$root/target/planted/copied"
echo out >"$root/target/debug/build/out"

rc=0
out=$("$root/scripts/check-target-leftovers.sh" "$root/marker") || rc=$?
if [ "$rc" != 1 ] || [ "$out" != "FAIL: the test run left these files under target/ (write them to a temp dir instead):
target/planted/copied" ]; then
    echo "test-check-target-leftovers: expected exit 1 naming only target/planted/copied, got exit $rc:" >&2
    echo "$out" >&2
    exit 1
fi
echo "test-check-target-leftovers: PASS"
