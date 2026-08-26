#!/usr/bin/env bash
# Classify FAILED keystone full-depth runs against the known-reds registry.
#
# Usage:
#   scripts/keystone-known-reds.sh <run.log> [<run.log> ...]
#
# `just keystone-nightly` passes only the runs that exited non-zero, but logs
# also arrive by hand, so each log's outcome is read from the log itself. For a
# FAILED run the script extracts the failure signatures — the first message line
# of every panic, which is either the composed-keystone oracle verdict
# ("reconciled composed sequence diverged from the oracle: [(...)]") or a
# harness assertion — and matches each one against the `Match pattern` column of
# docs/Testing/KeystoneKnownReds.md. A GREEN run has nothing to classify.
#
# Exit 0  — nothing to triage: every extracted signature matched a `known-red`
#           row (printed as WARN), and/or every log was green.
# Exit 1  — at least one signature matched nothing, or a FAILED run yielded no
#           signature at all. Both are regressions to triage, NOT rows to add.
# Exit 3  — at least one log admits no verdict: INDETERMINATE (empty, or
#           truncated before the run said anything) or UNREADABLE (no such
#           file). Never silently a pass — the input is broken, so nothing can
#           be said about the run at all.
#
# The registry is the single source of truth for the patterns; this script holds
# none of its own.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
registry="$repo_root/docs/Testing/KeystoneKnownReds.md"

if [ "$#" -lt 1 ]; then
    echo "usage: $0 <run.log> [<run.log> ...]" >&2
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

# What the log says the run DID: `failed`, `green`, or `indeterminate` (it says
# nothing either way). Read from the harness's own verdict lines — cargo test's
# `test result:`, cargo's and just's failure diagnostics, nextest's `Summary` /
# `FAIL` — because the classifier is handed logs by hand as often as by
# `just keystone-nightly`, and a green run's absence of panics is not a missing
# signature.
#
# Precedence is deliberate where a log carries BOTH a green verdict and
# something error-shaped: only the harness's own failure vocabulary outranks an
# explicit `test result: ok.`, because a test that prints a line starting
# `error: ` on its own stdout has not failed, and reading it as one reproduces
# the very false alarm this classifier exists to avoid. With no green verdict to
# weigh it against, any error-shaped line still counts as failure evidence.
harness_failed_re='^test result: FAILED|^ +FAIL \[|tests run:.* [1-9][0-9]* failed|^error: (test failed|could not compile|recipe .* failed|process didn.t exit successfully|linking with|failed to run custom build command)'
green_verdict_re='^test result: ok\.|tests run: [0-9]+ passed'
any_error_re='^error(\[[A-Za-z0-9]+\])?: '

log_outcome() {
    local log="$1"
    if grep -qE "$harness_failed_re" "$log"; then
        printf 'failed'
    elif grep -qE "$green_verdict_re" "$log"; then
        printf 'green'
    elif grep -qE "$any_error_re" "$log"; then
        printf 'failed'
    else
        printf 'indeterminate'
    fi
}

novel=0
matched=0
collateral=0
green=0
indeterminate=0
unreadable=0
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
    if [ ! -f "$log" ] || [ ! -r "$log" ]; then
        echo "[known-reds] UNREADABLE: $log — not a readable file." >&2
        unreadable=$((unreadable + 1))
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

    # Panics ARE failure evidence, and outrank the verdict lines: a run killed
    # mid-shrink is truncated before cargo ever prints `test result:`.
    if [ -s "$sigs_file" ]; then
        outcome=failed
    else
        outcome=$(log_outcome "$log")
    fi

    if [ "$outcome" = green ]; then
        echo "[known-reds] GREEN: $log — run passed, nothing to classify."
        green=$((green + 1))
        rm -f "$sigs_file"
        continue
    fi

    if [ "$outcome" = indeterminate ]; then
        echo "[known-reds] INDETERMINATE: $log — the log states no pass/fail"
        echo "               outcome (empty, truncated before any verdict, or not a"
        echo "               keystone run log). Tail:"
        tail -20 "$log" | sed 's/^/               | /'
        indeterminate=$((indeterminate + 1))
        rm -f "$sigs_file"
        continue
    fi

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
no_verdict=$((indeterminate + unreadable))
if [ "$no_verdict" -ne 0 ]; then
    echo "[known-reds] $indeterminate log(s) stated no outcome, $unreadable unreadable — see above."
fi
if [ "$novel" -ne 0 ]; then
    echo "[known-reds] FAIL: $novel novel panic(s), $matched known-red panic(s), $collateral collateral (ignored)."
    echo "             A novel signature is a regression to triage (bug-gap-triage),"
    echo "             not a row to add to $registry."
    exit 1
fi
if [ "$no_verdict" -ne 0 ]; then
    echo "[known-reds] NO VERDICT: nothing can be said about $no_verdict of the $# log(s)."
    echo "             Re-run the keystone and keep the whole log."
    exit 3
fi
if [ "$matched" -eq 0 ] && [ "$collateral" -eq 0 ]; then
    echo "[known-reds] PASS: $green green run(s), nothing to classify."
    exit 0
fi
echo "[known-reds] PASS-WITH-NOTE: $matched known-red panic(s), 0 novel, $collateral collateral (ignored)."
if [ "$green" -ne 0 ]; then
    echo "             Plus $green green run(s) with nothing to classify."
fi
echo "             Registry: docs/Testing/KeystoneKnownReds.md"
