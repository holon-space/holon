#!/usr/bin/env bash
# classify-run.sh : the hook says `classify` in `capabilities`, and `classify-run` prints the class a generated log was built to have: no failure -> green, only registered known reds -> known-red-only, a novel failure, a truncated, an empty or a missing log -> red. A log of the unit scripts' test run, made by that run on drawn test files, is green exactly when no file failed; cut before its summary it is red. Without `--class`, the classifier's output is byte-equal to the fork-point script's.
set -uo pipefail
here=$(cd "${0%/*}/.." && pwd)
hook=$here/hook
root=$(cd "$here/.." && pwd)
registry=$root/docs/Testing/KeystoneKnownReds.md
RANDOM=${SEED:-1}; draws=${DRAWS:-60}
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
fail=0

"$hook" capabilities | grep -qx classify || { echo "FAIL capabilities: no classify"; exit 1; }

# Registered messages: patterns of known-red rows that are literal text (no regex syntax) and unpinned to a site, read from the registry now.
mapfile -t registered < <(python3 "$root/scripts/check-known-red-evidence.py" --rows "$registry" | python3 -c '
import re, sys
for line in sys.stdin:
    _, pattern, site, *_ = line.rstrip("\n").split("\x1f")
    if not re.search(r"[][\\.*+?(){}|^$]", pattern) and not pattern.startswith("not classifier-armed") and not site:
        print(pattern)')
[ ${#registered[@]} -gt 0 ] || { echo "FAIL generator: the registry has no literal unpinned known-red pattern"; exit 1; }

# mklog <file> <panic message>... : a nextest run in which each message is one failing test's panic; no message makes a green run.
mklog() {
  local f=$1; shift; local n=$# i=0 m
  if [ "$n" -eq 0 ]; then
    printf '    Starting 2 tests across 1 binary\ntest result: ok. 2 passed; 0 failed; 0 ignored\n     Summary [   1.000s] 2 tests run: 2 passed, 0 skipped\n' > "$f"; return
  fi
  {
    printf '%s\n    Starting %s tests across 1 binary\n' 'warning: ignoring --test-threads because --no-capture is specified' "$n"
    for m in "$@"; do
      i=$((i + 1))
      printf '       START [   0.010s] (%s/%s) u::b t%s\n\nrunning 1 test\nthread '"'t%s'"' (3) panicked at some/file.rs:1:1:\n%s\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\ntest t%s ... FAILED\n        FAIL [   0.010s] (%s/%s) u::b t%s\n' "$i" "$n" "$i" "$i" "$m" "$i" "$i" "$n" "$i"
    done
    printf '     Summary [   1.000s] %s tests run: 0 passed, %s failed, 0 skipped\n' "$n" "$n"
    i=0; for m in "$@"; do i=$((i + 1)); printf '        FAIL [   0.010s] (%s/%s) u::b t%s\n' "$i" "$n" "$i"; done
    echo 'error: test run failed'
  } > "$f"
}

declare -A seen
check() { # check <label> <want> <log> [any] : the keystone classifier exits 1 or 3 on red; `any` takes exit 0 for red too
  local got; got=$("$hook" classify-run "$3" 2>"$work/err"); local rc=$?
  seen[$2]=$((${seen[$2]:-0} + 1))
  local ok=0
  case $2 in red) [ "$rc" -eq 1 ] || [ "$rc" -eq 3 ] || { [ "${4:-}" = any ] && [ "$rc" -eq 0 ]; } || ok=1 ;; *) [ "$rc" -eq 0 ] || ok=1 ;; esac
  [ "$got" = "$2" ] && [ $ok -eq 0 ] || { echo "FAIL $1: want '$2', got '$got' (exit $rc)"; fail=1; return; }
  echo "ok $1"
}
novel() { echo "novel failure $RANDOM$RANDOM never registered"; }
known() { echo "${registered[RANDOM % ${#registered[@]}]}"; }

mklog "$work/g" ; check "static green" green "$work/g"
mklog "$work/k" "${registered[0]}"; check "static one registered red" known-red-only "$work/k"
mklog "$work/n" "$(novel)"; check "static new red" red "$work/n"
check "static missing path" red "$work/absent.log"

for ((i = 0; i < draws; i++)); do
  f=$work/d$i
  case $((RANDOM % 6)) in
    0) mklog "$f"; want=green ;;
    1) msgs=(); for ((j = RANDOM % 3; j >= 0; j--)); do msgs+=("$(known)"); done; mklog "$f" "${msgs[@]}"; want=known-red-only ;;
    2) msgs=("$(novel)"); for ((j = RANDOM % 3; j > 0; j--)); do msgs+=("$(known)"); done; mklog "$f" "${msgs[@]}"; want=red ;;
    3) printf '%s\n    Starting 3 tests across 1 binary\n' 'warning: ignoring --test-threads because --no-capture is specified' > "$f"; want=red ;;
    4) : > "$f"; want=red ;;
    5) f=$work/absent$i; want=red ;;
  esac
  check "draw $i ($want)" "$want" "$f"
done
mapfile -t words < <("$hook" test-command /nonexistent-build-ws scripts)
envs=(); while [[ ${words[0]} =~ ^[A-Za-z_][A-Za-z0-9_]*= ]]; do envs+=("${words[0]}"); words=("${words[@]:1}"); done
# scripts_log <file> <exit code>... : the log of the scripts test run on one test file per exit code, each printing a nextest-like line, some beside a bytecode cache of a test module.
scripts_log() {
  local f=$1 t; shift; t=$(mktemp -d "$work/scripts.XXXX"); mkdir -p "$t/scripts/sub"
  local k=0 c; for c in "$@"; do
    k=$((k + 1)); local d=$t/scripts/$([ $((RANDOM % 2)) = 0 ] && echo sub/)
    printf 'echo "test result: ok. 1 passed; 0 failed"\nexit %s\n' "$c" > "${d}test-$k.sh"
    if [ $((RANDOM % 2)) = 0 ]; then mkdir -p "${d}__pycache__"; echo pyc > "${d}__pycache__/test-$k.cpython-3$((RANDOM % 20)).pyc"; fi
  done
  (cd "$t" && env "${envs[@]}" "${words[@]}") > "$f" 2>&1
}
scripts_log "$work/sg" 0 0; check "static scripts run green" green "$work/sg"
scripts_log "$work/sr" 0 1; check "static scripts run with a failing file" red "$work/sr" any
for ((i = 0; i < draws / 3; i++)); do
  codes=(); want=green; for ((j = RANDOM % 4; j >= 0; j--)); do c=$(( RANDOM % 3 == 0 ? RANDOM % 3 : 0 )); codes+=("$c"); [ "$c" = 0 ] || want=red; done
  scripts_log "$work/s$i" "${codes[@]}"
  if [ $((RANDOM % 4)) = 0 ]; then sed -i '' '/^Summary: /,$d' "$work/s$i"; want=red; fi
  check "scripts draw $i (${codes[*]}, $want)" "$want" "$work/s$i" any
done

for c in green known-red-only red; do [ "${seen[$c]:-0}" -gt 0 ] || { echo "FAIL generator: class $c never drawn"; fail=1; }; echo "drawn $c: ${seen[$c]:-0}"; done

# The classifier prints byte-equal output without --class: compare the script with the one at the base (BASE, else the fork point).
base=$work/base; mkdir -p "$base/scripts"
for f in "$root"/scripts/*; do ln -s "$f" "$base/scripts/"; done
rm "$base/scripts/keystone-known-reds.sh"
python3 "$root/scripts/jj_read.py" file show -r "${BASE:-fork_point(trunk() | @)}" scripts/keystone-known-reds.sh > "$base/scripts/keystone-known-reds.sh" || { echo "FAIL base script unreadable"; exit 1; }
chmod +x "$base/scripts/keystone-known-reds.sh"; ln -s "$root/docs" "$base/docs"
mklog "$work/s" "${registered[0]}" "$(novel)"
cmp <("$root/scripts/keystone-known-reds.sh" "$work/s" 2>&1) <("$base/scripts/keystone-known-reds.sh" "$work/s" 2>&1 | sed "s#$base#$root#g") || { echo "FAIL no-flag output differs from base"; fail=1; }
exit $fail
