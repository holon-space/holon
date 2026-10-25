#!/usr/bin/env bash
# Exercises gate-combine.sh and gate-run.sh against throwaway jj repos, with
# `just`, `cargo` and `flow` stubbed; nothing is built. Exit 0 = every case
# behaves as specified.
set -euo pipefail
export GATE_DISK_FLOOR_GIB=0  # the disk guard is the host's, not under test

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
  mkdir scripts && : > scripts/featuremap.py
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

# `just` and `cargo` stubs append each call and the quiet marker's content to
# $GATE_PROBE. HOME is a scratch dir, so the hold's marker, lock, log and slot
# walk are scratch files and no real build is lowered.
home="$scratch/home" stubs="$scratch/stubs" probe="$scratch/probe.log"
mkdir -p "$home/.claude/skills/orchestrator" "$home/.cache/holon-build" "$stubs"
ln -s "$HOME/.claude/skills/orchestrator/scripts" "$home/.claude/skills/orchestrator/scripts"
for tool in just cargo; do
  cat > "$stubs/$tool" <<'EOF'
#!/usr/bin/env bash
echo "$(basename "$0") $* | quiet=$(cat "$HOME/.cache/holon-build/quiet" 2>/dev/null)" >> "$GATE_PROBE"
EOF
done
cat > "$stubs/flow" <<'EOF'
#!/usr/bin/env bash
echo "flow $*" >> "$GATE_PROBE"
mkdir -p target/jobs
: > target/jobs/gate.gate.T.log
echo '{"exit": 7}' > target/jobs/gate.gate.T.status
echo "OUTCOME: waiting-for gate.gate.T"
EOF
chmod +x "$stubs"/*
in_stubs() { env -u HOLON_BUILD_SLOT_HOLDER HOME="$home" PATH="$stubs:$PATH" GATE_PROBE="$probe" FLOW_PR_HEADS="$heads" "$@"; }

echo "== the gate run inside a build slot"
rc=0; in_stubs HOLON_BUILD_SLOT_HOLDER=$$ GATE_STEP_POLL_S=0.1 "$gate_run" >"$scratch/gate.out" 2>&1 || rc=$?
expect "exits 0 with every step green (got $rc)" [ "$rc" = 0 ]
holder=$(sed -n 's/.*"event":"quiet-take".*"holder":\([0-9]*\).*/\1/p' "$home/.cache/holon-build/holds.jsonl" 2>/dev/null || true)
expect "takes the quiet hold once" [ "$(grep -c '"event":"quiet-take"' "$home/.cache/holon-build/holds.jsonl" || true)" = 1 ]
expect "the hold is the gate's own pid, not its caller's" [ "${holder:-$$}" != $$ ]
timing='just (hand-authored|projector-lag-lock|windowed-gate|latency-slo-gate|org-deep-text-timing)|cargo nextest run --no-fail-fast --workspace'
wrong=$(awk -v t="$timing" -v h="$holder" -F' [|] quiet=' '{ want = ($1 ~ "^(" t ")") ? h : ""; if ($2 != want) print }' "$probe")
expect "the marker holds the gate's pid in the timing steps and is empty in every other step" [ -z "$wrong" ]
[ -z "$wrong" ] || echo "$wrong"
expect "six timing calls among the calls" [ "$(grep -cE "^($timing)" "$probe")" = 6 ]
expect "the org_suite information run comes after the release" grep -q 'org_suite.* | quiet=$' "$probe"
expect "the marker is gone after the gate" [ ! -s "$home/.cache/holon-build/quiet" ]
expect "marks the target for the sweep" [ -f "$integ/target/.gc-mark" ]
expect "sweeps the build hash dirs last" [ "$(grep -E '^(== information only|build hash dirs)' "$scratch/gate.out" | tail -1 | cut -c1-15)" = "build hash dirs" ]
expect "skips the incremental sweep above the disk floor" grep -q '^incremental sweep: skipped' "$scratch/gate.out"

echo "== the gate run outside a build slot"
: > "$probe"
rc=0; in_stubs GATE_CARD=7 GATE_WAIT_POLL_S=0.1 "$gate_run" >/dev/null 2>&1 || rc=$?
expect "exits with the job's exit code (got $rc)" [ "$rc" = 7 ]
expect "starts itself as a --quiet-steps gate job of GATE_CARD" \
  grep -q -- '^flow job run --queue --card 7 --lane gate --kind gate --quiet-steps -- ' "$probe"

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
