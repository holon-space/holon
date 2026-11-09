#!/usr/bin/env bash
# facts-agree-with-hook.sh : for every cargo package of the workspace, the facts that name a path or a helper command agree with the hook's own answers.
set -euo pipefail
hook=$(cd "${0%/*}/.." && pwd)/hook
fail=0
every_unit() { cargo metadata --no-deps --format-version 1 --offline | jq -r '.packages[].name'; }
for u in $(every_unit); do
  facts=$("$hook" prompt-facts "$u")
  tests=$("$hook" layout "$u" | sed -n 's/^tests: //p')
  td=$(printf '%s\n' "$facts" | awk '/^@@ TESTS_DIR$/{f=1;next} /^@@ /{f=0} f' | tr -d '`' | sed "s#<crate>#$u#")
  [ "${td%/}" = "$tests" ] || { echo "FAIL $u: TESTS_DIR fact says $td, layout says $tests"; fail=1; }
  r=$(mktemp -d); t=$(mktemp -d); mkdir -p "$t/$tests"; : > "$t/$tests/main.rs"; printf '%s mod zz_probe;\n' "$u" > "$r/tester-mods.txt"
  cmd=$(printf '%s\n' "$facts" | grep -o '`[^`]*add-mods\.sh[^`]*`' | tr -d '`' | sed -e "s#@R@#$r#g" -e "s#@T@#$t#g" -e "s#<crate>#$u#g")
  [ -n "$cmd" ] || { echo "FAIL $u: no add-mods.sh command in the facts"; fail=1; }
  if ! (cd "$t" && eval "$cmd" >/dev/null 2>&1) || ! grep -qxF 'mod zz_probe;' "$t/$tests/main.rs"; then echo "FAIL $u: the facts' registration command does not register: $cmd"; fail=1; fi
  rm -rf "$r" "$t"
done
exit $fail
