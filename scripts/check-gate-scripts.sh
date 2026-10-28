#!/usr/bin/env bash
# Exercises gate-combine.sh, gate-integrate.sh and gate-timing.sh against
# throwaway jj repos, with `just` and `cargo` stubbed; nothing is built. Exit 0 = every case
# behaves as specified.
set -euo pipefail
export GATE_DISK_FLOOR_GIB=0  # the disk guard is the host's, not under test

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
combine="$here/gate-combine.sh"
integrate="$here/gate-integrate.sh"
timing="$here/gate-timing.sh"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
fails=0

expect() {
  local what=$1; shift
  if "$@"; then echo "ok:   $what"; else echo "FAIL: $what"; fails=$((fails + 1)); fi
}
cid()  { jj log -r "$1" --no-graph -T 'commit_id'; }
lane() { jj log -r "description(substring:\"$1\")" --no-graph -T 'commit_id'; }
all_commits() { jj log -r 'all()' --no-graph -T 'commit_id ++ "\n"' | sort; }

# A repo at $scratch/$1 with main = {f} and three lanes off main: `clean` adds
# a, `left` and `right` both rewrite f. Leaves the shell in the repo.
make_repo() {
  repo="$scratch/$1"
  integ="$repo/.claude/worktrees/_sw_integ"
  mkdir -p "$repo/.claude/worktrees" && cd "$repo"
  jj git init --quiet
  printf 'base\n' > f
  printf '.claude/worktrees/\ntarget/\n' > .gitignore
  mkdir scripts && : > scripts/featuremap.py
  printf '#!/bin/sh\n' > scripts/check-target-leftovers.sh && chmod +x scripts/check-target-leftovers.sh
  jj commit --quiet -m base
  jj bookmark create --quiet main -r @-
  jj workspace add --quiet --name _sw_integ -r main "$integ"
  jj new --quiet main -m clean;  printf 'a\n' > a
  jj new --quiet main -m left;   printf 'left\n' > f
  jj new --quiet main -m right;  printf 'right\n' > f
  jj new --quiet main
}

combine_in_repo() { (cd "$repo" && FLOW_PR_HEADS="$1" "$combine" 2>"$scratch/combine.err"); }

echo "== a clean weave"
make_repo clean
heads="$(lane clean) $(lane left)"
rc=0; out=$(combine_in_repo "$heads") || rc=$?
expect "clean weave exits 0 (got $rc)" [ "$rc" = 0 ]
expect "the last stdout line is the integration dir" [ "$(tail -n1 <<<"$out")" = "$integ" ]
cd "$integ"
expect "the integration checkout holds both lanes" [ "$(cat a f)" = $'a\nleft' ]
expect "target/gate-tip names @-" [ "$(cid @-)" = "$(cat target/gate-tip)" ]

echo "== a head rewritten locally since its push"
rc=0; combine_in_repo "${heads##* }" >/dev/null || rc=$?
expect "is refused with exit 1 (got $rc)" [ "$rc" = 1 ]

# `just` and `cargo` stubs append each call and the quiet marker's content to
# $GATE_PROBE; `just timing-gate-build` prints $BUILD_OUT and `just $STUB_RED`
# exits 1. HOME is a scratch
# dir, so the hold's marker, lock, log and slot walk are scratch files and no
# real build is lowered.
home="$scratch/home" stubs="$scratch/stubs" probe="$scratch/probe.log"
orch="$home/.claude/skills/orchestrator/scripts"
mkdir -p "$orch" "$home/.cache/holon-build" "$stubs"
ln -s "$HOME"/.claude/skills/orchestrator/scripts/* "$orch/"
# A track runs in this test's slot; the quiet hold is the real one.
rm "$orch/with-build-slot.sh"
cat > "$orch/with-build-slot.sh" <<EOF
#!/usr/bin/env bash
case "\$1" in --quiet-*) exec "$HOME/.claude/skills/orchestrator/scripts/with-build-slot.sh" "\$@" ;; esac
HOLON_BUILD_SLOT_HOLDER=\$\$ exec "\$@"
EOF
chmod +x "$orch/with-build-slot.sh"
for tool in just cargo; do
  cat > "$stubs/$tool" <<'EOF'
#!/usr/bin/env bash
echo "$(basename "$0") $* | quiet=$(cat "$HOME/.cache/holon-build/quiet" 2>/dev/null)" >> "$GATE_PROBE"
[ "$*" != timing-gate-build ] || printf '%s\n' "${BUILD_OUT:-}"
[ "$*" != "${STUB_RED:-}" ] || exit 1
EOF
done
chmod +x "$stubs"/*
tip=$(cat target/gate-tip)
# "<n> <name> <exit>" per gate-steps.jsonl row of gate $1, by step number.
rows_of() {
  /usr/bin/python3 -c 'import json, sys
for l in open(sys.argv[1]):
    r = json.loads(l)
    if r["gate"] == sys.argv[2]: print(r["step"], r["name"], r["exit"])' "$home/.cache/holon-build/gate-steps.jsonl" "$1" | sort -n
}
steps="1 20 2 13 11 19 12 14 15 17 7 3 4 5 6 8 9 10 18 22"
sorted_steps=$(tr ' ' '\n' <<<"$steps" | sort -n | tr '\n' ' ')
in_stubs() { env HOME="$home" PATH="$stubs:$PATH" GATE_PROBE="$probe" HOLON_BUILD_SLOT_HOLDER=$$ GATE_STEP_POLL_S=0.1 "$@"; }

echo "== the integrate run"
rc=0; in_stubs "$integrate" g1 "1 2" "$tip" >"$scratch/integrate.out" 2>&1 || rc=$?
expect "exits 0 with every step green (got $rc)" [ "$rc" = 0 ]
[ "$rc" = 0 ] || tail -n 20 "$scratch/integrate.out"
expect "its last line names the candidate tip" [ "$(tail -n1 "$scratch/integrate.out")" = "candidate tip: $tip" ]
order=$(sed -n 's/^== landing \[\([0-9]*\)\].*/\1/p' "$scratch/integrate.out" | tr '\n' ' ')
expect "logs the non-timing steps in order (got $order)" [ "$order" = "$steps " ]
expect "writes one gate-steps row per step" [ "$(rows_of g1 | cut -d' ' -f1 | tr '\n' ' ')" = "$sorted_steps" ]
expect "never calls timing-gate" [ "$(grep -c '^just timing-gate |' "$probe")" = 0 ]
expect "never takes the quiet hold" [ -z "$(grep -v ' | quiet=$' "$probe")" ]
expect "marks the target for the sweep" [ -f "$integ/target/.gc-mark" ]

echo "== the integrate run with one red step after gate-compile"
rc=0; in_stubs STUB_RED=gate-arch "$integrate" g2 "1 2" "$tip" >"$scratch/integrate-red.out" 2>&1 || rc=$?
expect "exits 1 (got $rc)" [ "$rc" = 1 ]
expect "names the red step" grep -qx 'RED STEP \[7\] gate-arch: exit 1, log target/gate-logs/g2-step-7.log' "$scratch/integrate-red.out"
expect "names no other step red" [ "$(grep -c '^RED ' "$scratch/integrate-red.out")" = 1 ]
expect "runs every other step to a green row" [ "$(rows_of g2 | grep -v ' 0$')" = "7 gate-arch 1" ]
expect "writes one gate-steps row per step" [ "$(rows_of g2 | cut -d' ' -f1 | tr '\n' ' ')" = "$sorted_steps" ]
expect "names no candidate tip" [ -z "$(grep '^candidate tip' "$scratch/integrate-red.out")" ]

echo "== the timing gate on the integrated tip"
: > "$probe"
rc=0; in_stubs "$timing" g1 "1 2" "$tip" >"$scratch/timing.out" 2>&1 || rc=$?
expect "exits 0 (got $rc)" [ "$rc" = 0 ]
expect "calls the build check, then timing-gate, and nothing else" \
  [ "$(cut -d' ' -f1-2 "$probe" | tr '\n' ' ')" = "just timing-gate-build just timing-gate " ]

echo "== the timing gate on a tip whose binaries are not built"
: > "$probe"
rc=0; in_stubs BUILD_OUT="   Compiling holon v0.1.0" "$timing" g1 "1 2" "$tip" >/dev/null 2>&1 || rc=$?
expect "exits 10 (got $rc)" [ "$rc" = 10 ]
expect "does not call timing-gate" [ "$(grep -c '^just timing-gate |' "$probe")" = 0 ]

echo "== the tree guards"
for script in "$integrate" "$timing"; do
  : > "$probe"
  rc=0; in_stubs "$script" g1 "1 2" "$(cid main)" >/dev/null 2>&1 || rc=$?
  expect "$(basename "$script"): a tip other than @- exits 9 (got $rc)" [ "$rc" = 9 ]
  expect "$(basename "$script"): before any cargo or just call" [ ! -s "$probe" ]
done
jj new --quiet main
rc=0; in_stubs "$timing" g1 "1 2" "$tip" >/dev/null 2>&1 || rc=$?
expect "@- off the tip exits 9 (got $rc)" [ "$rc" = 9 ]
jj new --quiet "$tip"; printf 'stray\n' > stray
rc=0; in_stubs "$timing" g1 "1 2" "$tip" >/dev/null 2>&1 || rc=$?
expect "edits on top of the tip exit 9 (got $rc)" [ "$rc" = 9 ]

echo "== a conflicting weave"
make_repo conflict
heads="$(lane clean) $(lane left) $(lane right)"
before=$(all_commits)
integ_before=$(cd "$integ" && cid @)
rc=0; combine_in_repo "$heads" >/dev/null || rc=$?
expect "exits 3 (got $rc)" [ "$rc" = 3 ]
expect "leaves every commit as before" [ "$(all_commits)" = "$before" ]
expect "leaves the integration @ where it was" [ "$(cd "$integ" && cid @)" = "$integ_before" ]
expect "writes no target/gate-tip" [ ! -e "$integ/target/gate-tip" ]

echo "check-gate-scripts: $fails failure(s)"
[ "$fails" = 0 ]
