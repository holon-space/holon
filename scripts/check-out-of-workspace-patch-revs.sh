#!/usr/bin/env bash
# Guard: out-of-workspace members must resolve every shared git dependency to
# the same rev as the root workspace.
#
# `frontends/holon-worker` declares its own `[workspace]`,
# so they get their own lockfile and their own `[patch]` table — cargo reads
# neither from the root. Their fork pins are hand-mirrored and their
# branch-tracking deps are locked independently, so both drift silently: the
# root workspace keeps building, and the lag only surfaces as an opaque
# API-drift compile error deep inside a CI log (`turso_sdk_kit` moved,
# `MappedNodeResolver` lost a field, …) — or, worse, as two different Turso /
# Loro engines writing the same vault.
#
# Comparing resolved revs rather than manifest lines catches both causes at
# once and needs no hand-maintained crate inventory: any git package present in
# both a member's lockfile and the root's must be at the same rev.
#
# To fix a reported drift, from the member's directory:
#   cargo update --package <name>          # branch-tracking dep: take the tip
#   # or edit the member's Cargo.toml rev to match the root, then cargo update
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MEMBERS=(frontends/holon-worker)

# "<package>\t<git-source>" for every git-sourced package in a lockfile.
git_pins() {
  awk '
    /^name = / { n = $3; gsub(/"/, "", n) }
    /^source = "git\+/ { s = $3; gsub(/"/, "", s); print n "\t" s }
  ' "$1" | sort -u
}

root_pins="$(mktemp)"
trap 'rm -f "$root_pins"' EXIT
git_pins "$ROOT/Cargo.lock" > "$root_pins"

failed=0
for member in "${MEMBERS[@]}"; do
  drift="$(join -t"$(printf '\t')" "$root_pins" <(git_pins "$ROOT/$member/Cargo.lock") \
    | awk -F"\t" '$2 != $3 { print "  " $1 "\n    root:   " $2 "\n    member: " $3 }')"
  if [ -z "$drift" ]; then
    echo "ok: $member — all shared git deps match the root"
  else
    echo "DRIFT: $member/Cargo.lock disagrees with the root lockfile:"
    echo "$drift"
    failed=1
  fi
done

exit "$failed"
