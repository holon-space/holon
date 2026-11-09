#!/usr/bin/env bash
# facts-many-units.sh : `prompt-facts` for several units at once names the tests dir of each, in order, as `layout` answers it.
set -euo pipefail
hook=$(cd "${0%/*}/.." && pwd)/hook
mapfile -t all < <(cargo metadata --no-deps --format-version 1 --offline | jq -r '.packages[].name')
units=("${all[@]:0:3}")
want=$(for u in "${units[@]}"; do printf '`%s/`, ' "$("$hook" layout "$u" | sed -n 's/^tests: //p')"; done)
got=$("$hook" prompt-facts "${units[@]}" | awk '/^@@ TESTS_DIR$/{f=1;next} /^@@ /{f=0} f' | sed '/^$/d')
[ "$got" = "${want%, }" ] || { echo "FAIL ${units[*]}: TESTS_DIR fact is $got, layout says ${want%, }"; exit 1; }
echo "ok ${units[*]}: $got"
