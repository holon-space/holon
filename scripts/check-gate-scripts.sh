#!/usr/bin/env bash
# Exercises gate-combine.sh and gate-run.sh --dry-run against throwaway jj repos;
# no cargo, no build slot. Exit 0 = every case behaves as specified.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
combine="$here/gate-combine.sh"
gate_run="$here/gate-run.sh"
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
  jj commit --quiet -m base
  jj bookmark create --quiet main -r @-
  jj workspace add --quiet --name _sw_integ -r main "$integ"
  jj new --quiet main -m clean;  printf 'a\n' > a
  jj new --quiet main -m left;   printf 'left\n' > f
  jj new --quiet main -m right;  printf 'right\n' > f
  jj new --quiet main
}

combine_in_repo() { (cd "$repo" && FLOW_PR_HEADS="$1" "$combine" 2>"$scratch/combine.err"); }
dry_run() { FLOW_PR_HEADS="$heads" "$gate_run" --dry-run; }

echo "== a clean weave"
make_repo clean
heads="$(lane clean) $(lane left)"
rc=0; out=$(combine_in_repo "$heads") || rc=$?
expect "clean weave exits 0 (got $rc)" [ "$rc" = 0 ]
expect "the last stdout line is the integration dir" [ "$(tail -n1 <<<"$out")" = "$integ" ]
cd "$integ"
expect "the integration checkout holds both lanes" [ "$(cat a f)" = $'a\nleft' ]
expect "target/gate-tip names @-" [ "$(cid @-)" = "$(cat target/gate-tip)" ]
rc=0; steps=$(dry_run) || rc=$?
expect "the dry run passes the guards on the tip (got $rc)" [ "$rc" = 0 ]
expect "the dry run lists the steps, fmt first" grep -q '^step 1 fmt: cargo fmt' <<<"$steps"
expect "the dry run lists the extra nextest" grep -q '^step 20 extra-nextest:' <<<"$steps"

echo "== a head rewritten locally since its push"
rc=0; combine_in_repo "${heads##* }" >/dev/null || rc=$?
expect "is refused with exit 1 (got $rc)" [ "$rc" = 1 ]

echo "== the tree guard"
cd "$integ"
jj new --quiet main
rc=0; dry_run >/dev/null || rc=$?
expect "@- off the tip exits 9 (got $rc)" [ "$rc" = 9 ]
jj new --quiet "$(cat target/gate-tip)"; printf 'stray\n' > stray
rc=0; dry_run >/dev/null || rc=$?
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
