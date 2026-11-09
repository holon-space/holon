#!/usr/bin/env bash
# ops-refuse-unknown-unit.sh : every op that answers about a unit exits non-zero for a name that is no cargo package of the workspace.
set -uo pipefail
hook=$(cd "${0%/*}/.." && pwd)/hook
fail=0
unknown_units() { printf '%s\n' no-such-crate holon-gpui-x "$(cargo metadata --no-deps --format-version 1 --offline | jq -r '.packages[0].name')-x"; }
for u in $(unknown_units); do
  for op in prompt-facts layout dependents; do
    "$hook" $op "$u" >/dev/null 2>&1 && { echo "FAIL $op $u: exit 0 for a unit that is no package"; fail=1; }
  done
done
exit $fail
