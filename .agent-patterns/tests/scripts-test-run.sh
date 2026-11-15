#!/usr/bin/env bash
# scripts-test-run.sh : the run that `test-command` prints for the unit `scripts` runs, in the tree it starts in, every `test-*` file under scripts/ (a file added after the command was printed too) once, skips bytecode caches under `__pycache__`, and exits non-zero exactly when one of them fails or has no runner.
set -uo pipefail
hook=$(cd "${0%/*}/.." && pwd)/hook
mapfile -t words < <("$hook" test-command /nonexistent-build-ws scripts)
envs=(); while [[ ${words[0]} =~ ^[A-Za-z_][A-Za-z0-9_]*= ]]; do envs+=("${words[0]}"); words=("${words[@]:1}"); done
t=$(mktemp -d); mkdir -p "$t/scripts/flow-checks"
mark() { printf 'echo ran >> "%s/ran-%s"\n' "$t" "$1"; }
printf 'import pathlib\npathlib.Path("%s/ran-a").open("a").write("ran\\n")\n' "$t" > "$t/scripts/test-a.py"
mark b > "$t/scripts/flow-checks/test-b.sh"
mark helper > "$t/scripts/helper.sh"
fail=0
run() { # run <label> <want exit 0|non-zero> <files that must have run once>...
  local label=$1 want=$2; shift 2; rm -f "$t"/ran-*
  (cd "$t" && env "${envs[@]}" "${words[@]}") > "$t/log" 2>&1; local ex=$?
  if [ "$want" = 0 ] && (( ex != 0 )); then echo "FAIL $label: exit $ex"; cat "$t/log"; fail=1; fi
  if [ "$want" = non-zero ] && (( ex == 0 )); then echo "FAIL $label: exit 0"; cat "$t/log"; fail=1; fi
  local f; for f in "$@"; do [ "$(cat "$t/ran-$f" 2>/dev/null)" = ran ] || { echo "FAIL $label: test-$f did not run exactly once"; fail=1; }; done
  [ ! -e "$t/ran-helper" ] || { echo "FAIL $label: a file not named test-* ran"; fail=1; }
}
run "all pass" 0 a b
mkdir -p "$t/scripts/__pycache__" "$t/scripts/flow-checks/__pycache__"
echo pyc > "$t/scripts/__pycache__/test-a.cpython-312.pyc"; echo pyc > "$t/scripts/flow-checks/__pycache__/test-b.cpython-312.pyc"
run "bytecode caches of test modules" 0 a b
echo pyc > "$t/scripts/test-d.pyc"
run "a file outside __pycache__ with no runner" non-zero a b
rm "$t/scripts/test-d.pyc"
mark c > "$t/scripts/test-c.sh"; echo 'exit 3' >> "$t/scripts/test-c.sh"
run "a new failing file" non-zero a b c
printf 'raise SystemExit(1)\n' >> "$t/scripts/test-a.py"; rm "$t/scripts/test-c.sh"
run "a failing Python file first" non-zero a b
rm -rf "$t"
exit $fail
