#!/usr/bin/env bash
# Classify FAILED keystone full-depth runs against the known-reds registry.
#
# Usage:
#   scripts/keystone-known-reds.sh <failed-run.log> [<failed-run.log> ...]
#
# Pass the log of every run that exited NON-ZERO (a green run has nothing to
# classify). For each log the script extracts the failure signatures — the first
# message line of every panic, which is either the composed-keystone oracle
# verdict ("reconciled composed sequence diverged from the oracle: [(...)]") or
# a harness assertion — and matches each one against the `Match pattern` column
# of docs/Testing/KeystoneKnownReds.md.
#
# Exit 0  — every extracted signature matched a `known-red` row (printed as WARN).
# Exit 1  — at least one signature matched nothing, or a failed run yielded no
#           signature at all. Both are regressions to triage, NOT rows to add.
#
# The registry is the single source of truth for the patterns; this script holds
# none of its own.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
registry="$repo_root/docs/Testing/KeystoneKnownReds.md"

if [ "$#" -lt 1 ]; then
    echo "usage: $0 <failed-run.log> [<failed-run.log> ...]" >&2
    exit 2
fi
if [ ! -f "$registry" ]; then
    echo "[known-reds] registry missing: $registry" >&2
    exit 2
fi

# Registry rows: | `key` | status | `pattern` | ... — only `known-red` rows
# classify. A pattern may not contain `|` (the table separator); use character
# classes instead of alternation.
keys=()
patterns=()
while IFS=$'\t' read -r key status pattern; do
    [ -z "$key" ] && continue
    [ "$status" = "known-red" ] || continue
    keys+=("$key")
    patterns+=("$pattern")
done < <(awk -F'|' '
    /^\| *`/ {
        gsub(/^ *`|` *$/, "", $2); gsub(/^ *| *$/, "", $3)
        gsub(/^ *`|` *$/, "", $4)
        if ($2 != "" && $4 != "") print $2 "\t" $3 "\t" $4
    }' "$registry")

if [ "${#keys[@]}" -eq 0 ]; then
    echo "[known-reds] registry has no known-red rows — every failure is novel."
fi

novel=0
matched=0
# Per-key hit counts + one example each. Parallel indexed arrays, not an
# associative array — macOS ships bash 3.2.
counts=()
examples=()
for i in "${!keys[@]}"; do
    counts[$i]=0
    examples[$i]=""
done
novel_file=$(mktemp)

for log in "$@"; do
    if [ ! -f "$log" ]; then
        echo "[known-reds] NOVEL: log not found: $log" >&2
        novel=$((novel + 1))
        continue
    fi

    # A panic prints "thread '…' panicked at <file>:<line>:" and the message on
    # the NEXT line; that message line IS the signature. Proptest re-panics on
    # every shrink step, so one failing case yields dozens of near-identical
    # lines — hence the aggregation below.
    sigs_file=$(mktemp)
    awk '/panicked at / {
            loc = $0; sub(/^.*panicked at /, "", loc); sub(/:$/, "", loc)
            if ((getline msg) > 0) print loc "\t" msg
         }' "$log" >"$sigs_file"

    if [ ! -s "$sigs_file" ]; then
        echo "[known-reds] NOVEL: $log — run failed but no panic signature was extracted"
        echo "               (compile error, timeout, or a new failure shape). Tail:"
        tail -20 "$log" | sed 's/^/               | /'
        novel=$((novel + 1))
        rm -f "$sigs_file"
        continue
    fi

    base=$(basename "$log")
    while IFS=$'\t' read -r loc sig; do
        hit=-1
        for i in "${!keys[@]}"; do
            if printf '%s\n' "$sig" | grep -qE -- "${patterns[$i]}"; then
                hit=$i
                break
            fi
        done
        if [ "$hit" -ge 0 ]; then
            counts[$hit]=$(( ${counts[$hit]} + 1 ))
            [ -z "${examples[$hit]}" ] && examples[$hit]="$base @ $loc: $(printf '%.240s' "$sig")"
            matched=$((matched + 1))
        else
            printf '%s @ %s: %.240s\n' "$base" "$loc" "$sig" >>"$novel_file"
            novel=$((novel + 1))
        fi
    done <"$sigs_file"
    rm -f "$sigs_file"
done

for i in "${!keys[@]}"; do
    if [ "${counts[$i]}" -gt 0 ]; then
        echo "WARN known-red [${keys[$i]}] x${counts[$i]}"
        echo "     ${examples[$i]}"
    fi
done
if [ -s "$novel_file" ]; then
    echo ""
    echo "NOVEL signatures (distinct, with occurrence counts):"
    sort "$novel_file" | uniq -c | sort -rn | sed 's/^/  /'
fi
rm -f "$novel_file"

echo ""
if [ "$novel" -ne 0 ]; then
    echo "[known-reds] FAIL: $novel novel panic(s), $matched known-red panic(s)."
    echo "             A novel signature is a regression to triage (bug-gap-triage),"
    echo "             not a row to add to $registry."
    exit 1
fi
echo "[known-reds] PASS-WITH-NOTE: $matched known-red panic(s), 0 novel."
echo "             Registry: docs/Testing/KeystoneKnownReds.md"
