#!/usr/bin/env bash
# The duration-assertion ratchet (#475): every duration comparison in assertion
# position carries a reviewed class, and a `timing` one is selected by the
# timing profile.
#
# Baseline: scripts/timing-assertions-baseline.txt, one reviewable line per
# site — `<path>\t<class>\t<trimmed line>`, the line NUMBER dropped so moving
# code is not a violation (the `defensive-ratchet.sh` convention). The class is
# the author's one-time judgement; see docs/Testing/TimingCritical.md.
#
# It fails on:
#   - a duration assertion the baseline does not classify (a NEW timing site),
#   - a baseline row still saying UNCLASSIFIED,
#   - a site classed `timing` that no row of docs/Testing/TimingCritical.md
#     names, so the timing profile never selects it,
#   - a timing row whose named `file:line` holds no duration assertion — which
#     is what keeps a content race out of the timing group,
#   - a committed `.config/nextest.toml` block that is not the generated one.
#
# Usage: scripts/check-timing-assertions.sh            # gate
#        scripts/check-timing-assertions.sh --update   # regenerate the baseline
#
# Costs no build: both halves read the tree. `timing-split.py resolve` is the
# half that DOES need the test binaries and runs in `just load-tolerant-nextest`.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SPLIT="/usr/bin/python3 $REPO_ROOT/scripts/timing-split.py"

if [ "${1-}" = "--update" ]; then
    $SPLIT assertions --update
    $SPLIT generate
    echo "Review scripts/timing-assertions-baseline.txt: every UNCLASSIFIED row"
    echo "needs its class before the gate passes."
    exit 0
fi

rc=0
$SPLIT check || rc=1
$SPLIT assertions || rc=1
exit $rc
