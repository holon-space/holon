#!/usr/bin/env bash
# Guard: every naive twin of `crates/holon-net/tests/uft_lower.rs` is red for
# the property IT drops, not for another twin's.
#
# A twin that is red for someone else's reason proves nothing about its own
# rule, and it hides a dead assertion: the `no-answer-inhibitor` twin hit a
# Guard before P3, so "two Answered for one ask" was unreachable and nothing
# flagged it (holon-work#236). Here each twin NAMES its property message; the
# run must be red AND must print that message.
#
# Usage: scripts/twin-own-red.sh [twin ...]   (default: every twin)
set -uo pipefail

cd "$(dirname "$0")/.." || exit 2

# twin|the message its red must print (the property it must break)
TABLE='
no-ask-hash|P1 the work fires on a changed binding
no-answer-inhibitor|got the wrong reply
double-answer|was accepted: two Answered for one ask
no-expire|P-quiescent: Answered.
ask-while-in-flight|P5 an offered ask
ask-while-retry-pending|the ask shows while a retry is pending
ask-while-outcome-unknown|P5 an offered ask
no-quiescence|P-quiescent: Answered.
keep-work-failed|P-quiescent: WorkFailed.
'

fail=0

# Self-maintaining: a twin the test accepts but the table does not name would
# be skipped silently — the rot this guard exists to prevent.
names=crates/holon-net/tests/exec_support/twin_names.rs
# rustfmt wraps long table rows, so match on the file without whitespace.
named=$(tr -d '[:space:]' <"$names" | grep -oE '\("[a-z-]+",LowerTwin::' | grep -oE '"[a-z-]+"' | tr -d '"' | sort -u)
[ -n "$named" ] || { echo "FAIL $names: no lowering twin name found"; exit 2; }
listed=$(printf '%s\n' "$TABLE" | sed -n 's/|.*//p' | grep . | sort -u)
missing=$(comm -23 <(echo "$named") <(echo "$listed"))
if [ -n "$missing" ]; then
    echo "FAIL $names names lowering twins the table does not name: $(echo "$missing" | tr '\n' ' ')"
    fail=1
fi

want=("$@")
while IFS='|' read -r twin marker; do
    [ -n "$twin" ] || continue
    if [ "${#want[@]}" -gt 0 ]; then
        printf '%s\n' "${want[@]}" | grep -qxF "$twin" || continue
    fi
    log="target/twin-own-red-$twin.log"
    UFT_LOWER_TWIN="$twin" cargo nextest run -p holon-net --features naive-twins \
        --no-fail-fast --test uft_lower >"$log" 2>&1
    if [ $? -eq 0 ]; then
        echo "FAIL $twin: green — the twin no longer breaks anything ($log)"
        fail=1
    elif grep -qF "$marker" "$log"; then
        echo "ok   $twin: red for its own property ('$marker')"
    else
        echo "FAIL $twin: red for another reason, not '$marker' ($log):"
        grep -A3 'panicked at' "$log" | head -4 | sed 's/^/       /'
        fail=1
    fi
done <<<"$TABLE"

[ "$fail" -eq 0 ] || echo "twin-own-red: a twin is red for a property it does not own."
exit "$fail"
