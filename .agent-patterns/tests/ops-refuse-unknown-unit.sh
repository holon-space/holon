#!/usr/bin/env bash
# ops-refuse-unknown-unit.sh : every op that answers about a unit answers for `scripts` (the scripts folder); `prompt-facts`, `layout` and `dependents` exit non-zero for a name that is neither `scripts` nor a cargo package of the workspace, and `prompt-facts` and `test-command` for a call that mixes `scripts` with cargo packages.
set -uo pipefail
hook=$(cd "${0%/*}/.." && pwd)/hook
fail=0
first=$(cargo metadata --no-deps --format-version 1 --offline | jq -r '.packages[0].name')
unknown_units() { printf '%s\n' no-such-crate holon-gpui-x "$first-x" scripts-x xscripts Scripts scripts/ script; }
for u in $(unknown_units); do
  for op in prompt-facts layout dependents; do
    "$hook" $op "$u" >/dev/null 2>&1 && { echo "FAIL $op $u: exit 0 for a unit that is no package"; fail=1; }
  done
done

lay=$("$hook" layout scripts) || { echo "FAIL layout scripts: exit non-zero"; fail=1; }
[ "$lay" = $'dir: scripts\ncode: scripts\ntests: scripts\nmanifest: -' ] || { echo "FAIL layout scripts: $lay"; fail=1; }
dep=$("$hook" dependents scripts) || { echo "FAIL dependents scripts: exit non-zero"; fail=1; }
[ -z "$dep" ] || { echo "FAIL dependents scripts: printed $dep"; fail=1; }
facts=$("$hook" prompt-facts scripts) || { echo "FAIL prompt-facts scripts: exit non-zero"; fail=1; }
block() { awk -v b="@@ $1" '$0 == b {f=1; next} /^@@ /{f=0} f' <<<"$facts" | sed '/^$/d'; }
[ "$(block TESTS_DIR)" = '`scripts/`' ] || { echo "FAIL prompt-facts scripts: TESTS_DIR is $(block TESTS_DIR)"; fail=1; }
# A scripts lane brief names no cargo command; the run it names is the hook's test-command.
grep -qE 'cargo|clippy|rustfmt|-p scripts' <<<"$(block TEST_CMD; block LINT_RULE; block FORMAT_CMD; block TEST_REGISTRATION_RULE)" \
  && { echo "FAIL prompt-facts scripts: a cargo fact for the scripts unit"; fail=1; }
cmd=$("$hook" test-command /nonexistent-build-ws scripts | grep -v '^[A-Za-z_][A-Za-z0-9_]*=' | paste -sd' ' -)
[ "$(block TEST_CMD)" = "$cmd" ] || { echo "FAIL prompt-facts scripts: TEST_CMD $(block TEST_CMD) is not the test-command $cmd"; fail=1; }

s=$(mktemp); printf 'scripts/lane.sh\n' > "$s"
for args in "/nonexistent-build-ws scripts" "--scope $s /nonexistent-build-ws scripts"; do
  # shellcheck disable=SC2086
  out=$("$hook" test-command $args) || { echo "FAIL test-command $args: exit non-zero"; fail=1; continue; }
  grep -qx run-scripts-tests <<<"$out" || { echo "FAIL test-command $args: not the scripts run: $out"; fail=1; }
done
r=$(mktemp -d); printf 'scripts mod x;\n%s mod y;\n' "$first" > "$r/tester-mods.txt"
for mods in "$r/tester-mods.txt" "$r/absent.txt"; do
  "$hook" register-tests "$mods" scripts >/dev/null 2>&1 || { echo "FAIL register-tests $mods scripts: exit non-zero"; fail=1; }
done
for op in prompt-facts test-command; do
  args=(scripts "$first"); [ $op = test-command ] && args=(/nonexistent-build-ws "${args[@]}")
  "$hook" $op "${args[@]}" >/dev/null 2>&1 && { echo "FAIL $op ${args[*]}: exit 0 for scripts mixed with a cargo package"; fail=1; }
done
rm -rf "$r" "$s"
exit $fail
