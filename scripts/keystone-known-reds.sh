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

# Strip volatile payloads that make otherwise-identical panics look distinct
# (per-run UUIDs, span Ids, resolver-map contents, split-block indices) so
# dedup and registry matching operate on the stable shape of the signature.
normalize_sig() {
    printf '%s' "$1" | sed -E \
        -e 's/^Test failed: (.*)\.$/\1/' \
        -e 's/[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/<uuid>/g' \
        -e 's/\bId\([0-9]+\)/Id(N)/g' \
        -e 's/Mapped: \[[^]]*\]/Mapped: [...]/g' \
        -e 's/split-[0-9]+/split-N/g'
}

# A panic is collateral (unwind noise from a thread tearing down after some
# other panic) when its payload is an opaque `Any { .. }` or it originates in
# tracing-subscriber's span teardown, not in workspace code. Collateral is
# reported but never counts toward novel/matched or the PASS/FAIL verdict.
is_collateral() {
    local loc="$1" sig="$2"
    [[ "$sig" == *'Any { .. }'* ]] && return 0
    [[ "$loc" == *"tracing-subscriber-"*"sharded.rs"* ]] && return 0
    return 1
}

novel=0
matched=0
collateral=0
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
    echo ""
    echo "=== $base ==="

    # The FIRST panic is what actually failed the run; proptest then re-runs
    # the same case (or a genuinely different one) while shrinking, so every
    # panic after it is a shrink-tail re-panic — printed separately so it
    # can't be mistaken for the run's real failure.
    line_no=0
    tail_file=$(mktemp)
    while IFS=$'\t' read -r loc sig; do
        line_no=$((line_no + 1))
        norm=$(normalize_sig "$sig")

        if is_collateral "$loc" "$sig"; then
            class="collateral"
            collateral=$((collateral + 1))
        else
            hit=-1
            for i in "${!keys[@]}"; do
                if printf '%s\n' "$norm" | grep -qE -- "${patterns[$i]}"; then
                    hit=$i
                    break
                fi
            done
            if [ "$hit" -ge 0 ]; then
                class="known-red:${keys[$hit]}"
                counts[$hit]=$(( ${counts[$hit]} + 1 ))
                [ -z "${examples[$hit]}" ] && examples[$hit]="$base @ $loc: $(printf '%.240s' "$norm")"
                matched=$((matched + 1))
            else
                class="novel"
                # Grouped by normalized message only, not location: the same
                # bug's panic site and proptest's own terminal re-report of it
                # are two locations for one signature (see normalize_sig's
                # Test-failed-wrapper strip) and must count as one novel hit,
                # the same way known-red counts already ignore location.
                printf '%.240s\n' "$norm" >>"$novel_file"
                novel=$((novel + 1))
            fi
        fi

        entry="[$class] $base @ $loc: $(printf '%.240s' "$norm")"
        if [ "$line_no" -eq 1 ]; then
            echo "PRIMARY: $entry"
        else
            printf '%s\n' "$entry" >>"$tail_file"
        fi
    done <"$sigs_file"
    rm -f "$sigs_file"

    if [ -s "$tail_file" ]; then
        tail_count=$(wc -l <"$tail_file" | tr -d ' ')
        echo "  -- shrink tail ($tail_count re-panics after the primary; may be a"
        echo "     different bug than the one that actually failed the run) --"
        sed 's/^/  /' "$tail_file"
    fi
    rm -f "$tail_file"

    successes=$(grep -m1 -oE 'successes: [0-9]+' "$log" | grep -oE '[0-9]+' || true)
    if [ -n "$successes" ]; then
        echo "successes: $successes"
        if [ "$successes" -lt 32 ]; then
            echo "WARN: low-power run ($successes draws)"
        fi
    fi
done
echo ""

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
    echo "[known-reds] FAIL: $novel novel panic(s), $matched known-red panic(s), $collateral collateral (ignored)."
    echo "             A novel signature is a regression to triage (bug-gap-triage),"
    echo "             not a row to add to $registry."
    exit 1
fi
echo "[known-reds] PASS-WITH-NOTE: $matched known-red panic(s), 0 novel, $collateral collateral (ignored)."
echo "             Registry: docs/Testing/KeystoneKnownReds.md"
