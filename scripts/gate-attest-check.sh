#!/usr/bin/env bash
# The gate's attestation check over <root>/steps/<step>/{claimed,attested}: every step's records in one
# folder, then scripts/check-gate-attestations.py over it.
set -euo pipefail
root=$1
mkdir -p "$root/all/claimed" "$root/all/attested"
for kind in claimed attested; do
  for dir in "$root"/steps/*/"$kind"; do
    [ ! -d "$dir" ] || cp -R "$dir/." "$root/all/$kind/"
  done
done
exec python3 "$(dirname "$0")/check-gate-attestations.py" "$root/all"
