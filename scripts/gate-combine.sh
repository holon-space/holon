#!/usr/bin/env bash
# The `combine` step of the landing-gate batch (see gh-kanban's `batch NAME`).
#
# Usage, from the repo root: FLOW_PR_HEADS="<sha> <sha> ..." scripts/gate-combine.sh
#
# Rebases each head's lane, in order, onto main (the first) or onto the previous
# head (the rest), then starts a fresh empty change on the result in the
# integration workspace `.claude/worktrees/_sw_integ`, so its @- is the tip.
# The tip's commit id goes to target/gate-tip there: the tip argument of
# gate-integrate.sh and gate-timing.sh.
#
# Stdout's last line: the integration workspace's directory.
# Exit 3: a head conflicts; every rebase of this run is reverted first.
# Any other failure exits non-zero with a message on stderr.
set -euo pipefail

: "${FLOW_PR_HEADS:?gate-combine.sh: FLOW_PR_HEADS names no head}"
integ="$PWD/.claude/worktrees/_sw_integ"
[ -d "$integ/.jj" ] || { echo "gate-combine.sh: no integration workspace at $integ" >&2; exit 1; }
cd "$integ"

[ -z "$(jj diff -r @ --summary)" ] || {
  echo "gate-combine.sh: $integ has uncommitted edits in @; move them away first" >&2; exit 1; }

last_op() { jj op log -n1 --no-graph --ignore-working-copy -T 'id'; }
our_ops=()
revert_ours() {
  local i
  for ((i = ${#our_ops[@]} - 1; i >= 0; i--)); do
    jj op revert "${our_ops[i]}" >&2
  done
}

base=$(jj log -r main --no-graph -T 'commit_id')
tip=$base
for head in $FLOW_PR_HEADS; do
  # A hidden head is a pushed commit that was rewritten locally: the PR head is
  # not the tree this run would test.
  [ "$(jj log -r "$head" --no-graph -T 'hidden')" = false ] || {
    echo "gate-combine.sh: head $head is not the visible version of its change; push the lane or fetch" >&2
    revert_ours; exit 1; }
  change=$(jj log -r "$head" --no-graph -T 'change_id')
  before=$(last_op)
  jj rebase -b "$head" -d "$tip" >&2
  after=$(last_op)
  [ "$after" = "$before" ] || our_ops+=("$after")
  tip=$(jj log -r "$change" --no-graph -T 'commit_id')
  if [ -n "$(jj log -r "conflicts() & ($base..$tip)" --no-graph -T 'commit_id')" ]; then
    echo "gate-combine.sh: CONFLICT weaving head $head (change $change) onto $base; reverting" >&2
    revert_ours
    exit 3
  fi
done

jj new "$tip" >&2
mkdir -p target
echo "$tip" > target/gate-tip
echo "gate-combine.sh: tip $tip on main $base" >&2
echo "$integ"
