#!/usr/bin/env bash
# The teeth of a measure-mode gate step: prove the mode reached the oracles,
# and that every breach it saw was DISCLOSED rather than judged.
#
# Usage: scripts/assert-measure-mode.sh <log> [<log>...]
#
# Two things are checkable from a log, and this asserts both:
#
#   1. The run really was in `measure`. The recipe echoes its
#      `HOLON_PBT_TIMING=` line into the log before running; a step that
#      silently fell back to `enforce` would otherwise look identical to a
#      green measure run.
#   2. No ENFORCE-shaped timing verdict appears, anywhere in a line: the
#      composed harness reports a failed invariant Debug-quoted inside its
#      `diverged from the oracle: [...]` panic. The WEDGE and the SQL-count
#      ceilings fail in both modes, so they are not leaks.
#
# It does NOT require a MEASURE-ONLY line to be present: a step in which every
# transition stayed inside its budget emits none, and that is the honest green.
# The count is reported either way, because zero over a whole gate is worth
# looking at.

set -euo pipefail

[ $# -ge 1 ] || { echo "usage: $0 <log> [<log>...]" >&2; exit 2; }

# The three `inv-settle-budget` Fail shapes (boot, maintenance, interaction).
SETTLE_FAIL="\\[inv-settle-budget\\] ((boot|maintenance) )?'[^']*' took [0-9]+ms"
# An `inv-sql-budget` Fail that names a ceiling load moves; the Debug-quoted
# form keeps it on the Fail's line, the plain form indents it below.
NONDET='\.(single_query|wall_time|settle|rss_delta|rss_cumulative): '
SQL_TIMING_FAIL="\\[inv-sql-budget\\] [0-9]+ budget violation.*$NONDET|^  [^ ]+$NONDET"

rc=0
for L in "$@"; do
    [ -f "$L" ] || { echo "assert-measure-mode: no log at $L" >&2; exit 2; }
    if ! grep -qE '^HOLON_PBT_TIMING=measure$' "$L"; then
        echo "FAIL: $L carries no 'HOLON_PBT_TIMING=measure' line, so nothing"
        echo "      proves the step ran in measure mode. The recipe must echo it."
        rc=1
    fi
    if leak=$(grep -nE "$SETTLE_FAIL|$SQL_TIMING_FAIL" "$L"); then
        echo "FAIL: $L holds an ENFORCE-shaped timing verdict, so the mode did not"
        echo "      reach that oracle. In measure a breach is a MEASURE-ONLY line:"
        printf '%s\n' "$leak" | head -5 | sed 's/^/      /'
        rc=1
    fi
    echo "$L: $(grep -c 'MEASURE-ONLY' "$L" || true) MEASURE-ONLY line(s)"
done

[ $rc = 0 ] && echo "assert-measure-mode: PASS"
exit $rc
