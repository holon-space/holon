#!/usr/bin/env bash
# check-gate-scripts.sh must fail for each way the extra nextest run can drop
# the org_suite tests; each case plants one narrowing into a copy of scripts/.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fails=0
for plant in "--exclude=holon-integration-tests" "--filterset='not test(/^org_suite::/)'" "-E'not test(/^org_suite::/)'" "--lib" "views_law"; do
  copy=$(mktemp -d)
  cp -R "$here" "$copy/scripts" && cp "$here/../justfile" "$copy/justfile"
  /usr/bin/python3 - "$copy/scripts/gate-integrate.sh" "$plant" <<'PY'
import sys; p, plant = sys.argv[1:]; s = open(p).read()
old = '--workspace --features "$PBT_FEATURES" 2>&1'
assert s.count(old) == 1
open(p, 'w').write(s.replace(old, '--workspace --features "$PBT_FEATURES" ' + plant + ' 2>&1'))
PY
  if bash "$copy/scripts/check-gate-scripts.sh" >/dev/null 2>&1; then echo "FAIL: stays green with $plant"; fails=$((fails + 1)); else echo "ok:   red with $plant"; fi
  rm -rf "$copy"
done
echo "check-gate-scripts-narrowing: $fails failure(s)"
[ "$fails" = 0 ]
