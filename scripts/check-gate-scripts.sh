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
real_just=$(command -v just)
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
  mkdir scripts
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

# `just` and `cargo` stubs append each call (a `just --evaluate` then runs the real just), its HOLON_PBT_TIMING, the record file of gate-step.sh's runner
# and the quiet marker's content to $GATE_PROBE; `just latency-slo-gate` prints $SETTLE_OUT (default: a contention line
# and an admission), `just timing-gate-build` prints $BUILD_OUT
# `just $STUB_RED` exits 1 and `just $STUB_SLEEP` sleeps 2 s; with STUB_RED_NEXTEST set, every nextest run
# fails one org_suite test no known-red row lists. HOME is a scratch dir, so the hold's marker,
# lock, log and slot walk are scratch files and no real build is lowered.
home="$scratch/home" stubs="$scratch/stubs" probe="$scratch/probe.log"
orch="$home/.claude/skills/orchestrator/scripts"
mkdir -p "$orch" "$home/.cache/holon-build" "$stubs"
ln -s "$HOME"/.claude/skills/orchestrator/scripts/* "$orch/"
# A track runs in this test's slot, or with GATE_HOLD_FIRST set in the real one; the quiet hold is
# the real one.
rm "$orch/with-build-slot.sh"
cat > "$orch/with-build-slot.sh" <<EOF
#!/usr/bin/env bash
case "\$1" in --quiet-*) exec "$HOME/.claude/skills/orchestrator/scripts/with-build-slot.sh" "\$@" ;; esac
[ -z "\${GATE_HOLD_FIRST:-}" ] || exec "$HOME/.claude/skills/orchestrator/scripts/with-build-slot.sh" "\$@"
HOLON_BUILD_SLOT_HOLDER=\$\$ exec "\$@"
EOF
chmod +x "$orch/with-build-slot.sh"
for tool in just cargo; do
  cat > "$stubs/$tool" <<'EOF'
#!/usr/bin/env bash
echo "$(basename "$0") $* | timing=${HOLON_PBT_TIMING-unset} | record=${GATE_RECORD-unset} | quiet=$(cat "$HOME/.cache/holon-build/quiet" 2>/dev/null)" >> "$GATE_PROBE"
case " $* " in *" --evaluate "*) exec "$REAL_JUST" "$@" ;; esac
[ "$*" != timing-gate-build ] || printf '%s\n' "${BUILD_OUT:-}"
[ "$*" != gate-compile ] || [ -z "${GATE_HOLD_FIRST:-}" ] || "$HOME/.claude/skills/orchestrator/scripts/with-build-slot.sh" --quiet-hold "$GATE_PID"
[ "${1:-}" != latency-slo-gate ] || printf '%b' "${SETTLE_OUT-mean boot matview_ddl 1.0ms\nhost admitted\n}"
[ "$*" != "${STUB_RED:-}" ] || exit 1
[ "$*" != "${STUB_SLEEP:-}" ] || sleep 2
[ -z "${STUB_RED_NEXTEST:-}" ] || [ "${1:-} ${2:-}" != "nextest run" ] || { echo "        FAIL [   1.000s] holon-integration-tests::integration org_suite::stub_red"; exit 100; }
EOF
done
cat > "$stubs/df" <<'EOF'
#!/bin/sh
[ -n "${STUB_DF_FREE:-}" ] || exec /bin/df "$@"
printf 'Filesystem 1G-blocks Used Available\nstub 1000 0 %s\n' "$STUB_DF_FREE"
EOF
chmod +x "$stubs"/*
tip=$(cat target/gate-tip)
# "<n> <name> <exit>" per gate-steps.jsonl row of gate $1, by step number.
rows_of() {
  /usr/bin/python3 -c 'import json, sys
for l in open(sys.argv[1]):
    r = json.loads(l)
    if r["gate"] == sys.argv[2]: print(r["step"], r["name"], r.get("exit", "skipped"))' "$home/.cache/holon-build/gate-steps.jsonl" "$1" | sort -n
}
steps="1 20 2 13 17 7 14 5 6 8 9 18 19 15 11 3 4 12 10 22"
sorted_steps=$(tr ' ' '\n' <<<"$steps" | sort -n | tr '\n' ' ')
# Without the caller's HOLON_PBT_TIMING, only the gate scripts can set it.
# The real wrapper asks rustup for rustc; under the scratch HOME rustup would install a toolchain.
in_stubs() { env -u HOLON_PBT_TIMING REAL_JUST="$real_just" HOME="$home" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" PATH="$stubs:$PATH" GATE_PROBE="$probe" HOLON_BUILD_SLOT_HOLDER=$$ GATE_STEP_POLL_S=0.1 "$@"; }
marker="$home/.cache/holon-build/quiet"
# The probe lines of the steps that go red on a loaded host: the extra nextest
# and the windowed gate.
loaded_red='^(cargo nextest run --profile load-tolerant|just windowed-gate) '
# Whether every such line saw the hold of pid $1 and there is at least one.
held_by() { grep -qE "$loaded_red" "$probe" && ! grep -E "$loaded_red" "$probe" | grep -qv " | quiet=$1\$"; }
held_by_none_other() { ! grep -vE "$loaded_red" "$probe" | grep -qv ' | quiet=$'; }
# Runs every track alone, in order, as gate g4 whose pid is this shell's.
tracks_alone() {
  local t
  for t in 0 1 2; do in_stubs GATE=g4 GATE_CARDS="1 2" GATE_PID=$$ "$integrate" --track $t >>"$scratch/tracks.out" 2>&1 || return; done
}

echo "== the integrate run"
rc=0; in_stubs "$integrate" g1 "1 2" "$tip" >"$scratch/integrate.out" 2>&1 || rc=$?
expect "exits 0 with every step green (got $rc)" [ "$rc" = 0 ]
[ "$rc" = 0 ] || tail -n 20 "$scratch/integrate.out"
expect "its last line names the candidate tip" [ "$(tail -n1 "$scratch/integrate.out")" = "candidate tip: $tip" ]
order=$(sed -n 's/^== landing \[\([0-9]*\)\].*/\1/p' "$scratch/integrate.out" | tr '\n' ' ')
expect "logs the non-timing steps in order (got $order)" [ "$order" = "$steps " ]
expect "writes one gate-steps row per step" [ "$(rows_of g1 | cut -d' ' -f1 | tr '\n' ' ')" = "$sorted_steps" ]
expect "never calls timing-gate" [ "$(grep -c '^just timing-gate |' "$probe")" = 0 ]
gate_pid=$(sed -n 's/^gate-integrate.sh: g1 (pid \([0-9]*\)).*/\1/p' "$scratch/integrate.out")
expect "runs the loaded-red steps under the gate's own hold" held_by "$gate_pid"
expect "releases the hold at its end" [ ! -e "$marker" ]
# The integrate run judges no duration, so every cargo call that runs tests
# measures; fmt runs none.
expect "runs cargo test arms" grep -qE '^cargo (test|nextest) ' "$probe"
unmeasured=$(grep -E '^cargo (test|nextest) ' "$probe" | grep -v ' | timing=measure | ' || true)
expect "runs every cargo test arm with HOLON_PBT_TIMING=measure${unmeasured:+, not: $unmeasured}" [ -z "$unmeasured" ]
# A `just` arm runs cargo inside its recipe, out of the stub's sight: a recipe
# that runs integration tests must default the mode to measure.
unmeasured_just=$(sed -n 's/^just \([^ ]*\) | timing=\([^ |]*\) | .*/\1 \2/p' "$probe" | while read -r r t; do
  [ "$t" != measure ] || continue
  body=$(just --justfile "$here/../justfile" --show "$r")
  grep -qE -- '--test integration|just pbt ' <<<"$body" || continue
  grep -qE 'HOLON_PBT_TIMING=(\$\{HOLON_PBT_TIMING-)?measure' <<<"$body" || printf '%s ' "$r"
done)
expect "runs every just test arm with measure${unmeasured_just:+, not: $unmeasured_just}" [ -z "$unmeasured_just" ]
expect "marks the target for the sweep" [ -f "$integ/target/.gc-mark" ]
# The org_suite tests block only through the extra nextest run: it must select
# the whole workspace with the integration tests' features, narrowed by nothing.
extra_runs=$(grep -E '^cargo nextest run .*--workspace .*--features [^ ]*holon-integration-tests/pbt' "$probe" || true)
expect "runs one whole-workspace nextest with the integration features (got: ${extra_runs:-none})" [ "$(grep -c . <<<"$extra_runs")" = 1 ]
extra_argv="${extra_runs%% | *}"
expect "narrows that run by no filter or package (got: $extra_argv)" grep -qxE 'cargo nextest run --profile load-tolerant --no-fail-fast --workspace --features [^ ]*holon-integration-tests/pbt[^ ]*' <<<"$extra_argv"

echo "== the integrate run with a red org_suite test"
rc=0; in_stubs STUB_RED_NEXTEST=1 "$integrate" g3 "1 2" "$tip" >"$scratch/integrate-org.out" 2>&1 || rc=$?
expect "exits 12 (got $rc)" [ "$rc" = 12 ]
expect "names the novel red" grep -qx 'EXTRA NEXTEST NOVEL RED' "$scratch/integrate-org.out"
expect "names no candidate tip" [ -z "$(grep '^candidate tip' "$scratch/integrate-org.out")" ]

echo "== the integrate run with one red step after gate-compile"
rc=0; in_stubs STUB_RED=gate-arch "$integrate" g2 "1 2" "$tip" >"$scratch/integrate-red.out" 2>&1 || rc=$?
expect "exits 1 (got $rc)" [ "$rc" = 1 ]
expect "names the red step" grep -qx 'RED STEP \[7\] gate-arch: exit 1, log target/gate-logs/g2-step-7.log' "$scratch/integrate-red.out"
expect "names no other step red" [ "$(grep -c '^RED ' "$scratch/integrate-red.out")" = 1 ]
expect "runs every other step to a green row" [ "$(rows_of g2 | grep -v ' 0$')" = "7 gate-arch 1" ]
expect "writes one gate-steps row per step" [ "$(rows_of g2 | cut -d' ' -f1 | tr '\n' ' ')" = "$sorted_steps" ]
expect "names no candidate tip" [ -z "$(grep '^candidate tip' "$scratch/integrate-red.out")" ]

echo "== the integrate run with a red cargo-free check"
rc=0; in_stubs STUB_RED=cargo-free-checks "$integrate" g7 "1 2" "$tip" >"$scratch/integrate-cargo-free.out" 2>&1 || rc=$?
expect "exits 1 (got $rc)" [ "$rc" = 1 ]
expect "names the feature-map step red" grep -qx 'RED STEP \[9\] featuremap: exit 1, log target/gate-logs/g7-step-9.log' "$scratch/integrate-cargo-free.out"

echo "== the integrate run with a red step under the hold"
rc=0; in_stubs STUB_RED=windowed-gate "$integrate" g5 "1 2" "$tip" >"$scratch/integrate-held-red.out" 2>&1 || rc=$?
expect "exits 1 (got $rc)" [ "$rc" = 1 ]
expect "releases the hold" [ ! -e "$marker" ]

echo "== the integrate run whose own hold is up before its tracks start"
# gate-compile takes the hold for the gate's pid, so the tracks start under it in the real wrapper.
rc=0; in_stubs GATE_HOLD_FIRST=1 "$integrate" g6 "1 2" "$tip" >"$scratch/integrate-own-hold.out" 2>&1 || rc=$?
expect "exits 0 (got $rc)" [ "$rc" = 0 ]
expect "lowers no track of its own" [ -z "$(grep 'this build runs in the background class' "$scratch/integrate-own-hold.out")" ]
expect "releases the hold" [ ! -e "$marker" ]

echo "== each track alone"
: > "$probe"
rc=0; tracks_alone || rc=$?
expect "every track exits 0 (got $rc)" [ "$rc" = 0 ]
expect "the loaded-red steps run under the hold" held_by $$
expect "no other step runs under it" held_by_none_other
expect "the hold is released" [ ! -e "$marker" ]
expect "warns of no expired hold" [ -z "$(grep 'quiet hold may have expired' "$scratch/tracks.out")" ]

echo "== the quiet track whose step outlives the hold's age limit"
rc=0; in_stubs GATE=g4 GATE_CARDS="1 2" GATE_PID=$$ GATE_QUIET_MAX_AGE_S=1 STUB_SLEEP=windowed-gate "$integrate" --track 1 >"$scratch/track-expired.out" 2>&1 || rc=$?
expect "exits 0 (got $rc)" [ "$rc" = 0 ]
expect "names the time after which reds may be load reds" grep -qE '^quiet hold may have expired; reds after [0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:]{8}Z may be load reds$' "$scratch/track-expired.out"

echo "== the quiet track while another job holds the hold"
sleep 60 & other=$!
HOME="$home" "$orch/with-build-slot.sh" --quiet-hold "$other" 2>/dev/null
(sleep 1; HOME="$home" "$orch/with-build-slot.sh" --quiet-release "$other" 2>/dev/null) &
: > "$probe"
rc=0; in_stubs GATE=g4 GATE_CARDS="1 2" GATE_PID=$$ "$integrate" --track 1 >"$scratch/track-wait.out" 2>&1 || rc=$?
kill "$other"; wait
expect "waits for the release and exits 0 (got $rc)" [ "$rc" = 0 ]
expect "then runs its steps under its own hold" held_by $$

echo "== the integrate run of a batch that selects two steps"
selected="$scratch/selected"
printf 'fmt tree-check\nkeystone-tour crate-reached\n' > "$selected"
: > "$probe"
rc=0; in_stubs GATE_SELECT=1 GATE_SELECTED="$selected" "$orch/gate-step.sh" run "$integrate" g7 "1 2" "$tip" >"$scratch/integrate-selected.out" 2>&1 || rc=$?
expect "exits 0 (got $rc)" [ "$rc" = 0 ]
[ "$rc" = 0 ] || tail -n 20 "$scratch/integrate-selected.out"
# The scripts evaluate justfile variables; every other call is a step's.
step_calls=$(grep -v -- ' --evaluate ' "$probe")
expect "runs only the selected steps (got: $(cut -d'|' -f1 <<<"$step_calls" | tr '\n' ','))" \
  [ "$(cut -d'|' -f1 <<<"$step_calls" | tr '\n' ',')" = "cargo fmt --all -- --check ,just keystone-tour ," ]
expect "no step sees the runner's record file" [ -z "$(grep -v ' | record=unset | ' <<<"$step_calls")" ]
expect "writes a skipped row for every other step" [ "$(rows_of g7 | grep -c ' skipped$')" = $(($(wc -w <<<"$steps") - 2)) ]

echo "== the timing gate of a batch that selects no timing step"
: > "$selected"; : > "$probe"
rc=0; in_stubs GATE_SELECT=1 GATE_SELECTED="$selected" "$orch/gate-step.sh" run "$timing" g7 "1 2" "$tip" >"$scratch/timing-selected.out" 2>&1 || rc=$?
expect "exits 0 (got $rc)" [ "$rc" = 0 ]
expect "calls no just or cargo but to evaluate the shape" [ -z "$(grep -v -- ' --evaluate ' "$probe")" ]

echo "== the timing-gate recipe while another job holds the hold"
# The real recipe from this justfile runs; only its steps script is a stub.
recipe="$scratch/recipe"
mkdir -p "$recipe/scripts"
cat > "$recipe/scripts/timing-gate-steps.sh" <<'EOF'
echo "timing-gate-steps | quiet=$(cat "$HOME/.cache/holon-build/quiet")" >> "$GATE_PROBE"
EOF
sleep 60 & other=$!
HOME="$home" "$orch/with-build-slot.sh" --quiet-hold "$other" 2>/dev/null
(sleep 1; HOME="$home" "$orch/with-build-slot.sh" --quiet-release "$other" 2>/dev/null) &
: > "$probe"
rc=0; in_stubs "$real_just" --justfile "$here/../justfile" --working-directory "$recipe" timing-gate >"$scratch/recipe-wait.out" 2>&1 || rc=$?
kill "$other"; wait
expect "waits for the release and exits 0 (got $rc)" [ "$rc" = 0 ]
expect "then runs its steps under its own hold" grep -qE "^timing-gate-steps \| quiet=[0-9]+\$" "$probe"
expect "not under the other job's hold" [ -z "$(grep "^timing-gate-steps | quiet=$other\$" "$probe")" ]
expect "releases the hold" [ ! -e "$marker" ]

echo "== the timing-gate recipe on a settle probe without a contention line"
for case in "host admitted\n|probe line absent — judging" "|the probe round printed neither an admission nor a contention value"; do
  rc=0; in_stubs SETTLE_OUT="${case%%|*}" "$real_just" --justfile "$here/../justfile" --working-directory "$recipe" timing-gate >"$scratch/recipe-noprobe.out" 2>&1 || rc=$?
  expect "settle output '${case%%|*}': exits 0 (got $rc)" [ "$rc" = 0 ]
  expect "settle output '${case%%|*}': names the state '${case#*|}'" grep -q "${case#*|}" "$scratch/recipe-noprobe.out"
done

echo "== the timing gate on the integrated tip"
: > "$probe"
rc=0; in_stubs "$timing" g1 "1 2" "$tip" >"$scratch/timing.out" 2>&1 || rc=$?
expect "exits 0 (got $rc)" [ "$rc" = 0 ]
expect "calls the build check, then timing-gate, then target-gc's replay of the record, and nothing else" \
  [ "$(grep -v -- ' --evaluate ' "$probe" | cut -d' ' -f1-2 | tr '\n' ' ')" = "just timing-gate-build just timing-gate cargo test " ]
expect "the replay builds and runs nothing new: --no-run, no nextest profile" \
  grep -q -- '^cargo test --workspace --features [^ ]* --no-run --message-format=json |' "$probe"

echo "== one disk floor: the integrate guard and the timing gate's target-gc both use the justfile's"
floor=$(env -u GATE_DISK_FLOOR_GIB "$real_just" --justfile "$here/../justfile" --evaluate GATE_DISK_FLOOR_GIB)
rc=0; in_stubs env -u GATE_DISK_FLOOR_GIB "$timing" g1 "1 2" "$tip" >"$scratch/timing-floor.out" 2>&1 || rc=$?
expect "the timing gate exits 0 (got $rc)" [ "$rc" = 0 ]
expect "its target-gc pass runs at floor $floor GiB" grep -q ", floor $floor GiB\$" "$scratch/timing-floor.out"
rc=0; in_stubs env -u GATE_DISK_FLOOR_GIB STUB_DF_FREE=$((floor - 1)) "$integrate" g1 "1 2" "$tip" >"$scratch/integrate-floor.out" 2>&1 || rc=$?
expect "the integrate run refuses 1 GiB below it: exits 8 (got $rc)" [ "$rc" = 8 ]
expect "and names that floor" grep -qx "DISK BELOW $floor GiB" "$scratch/integrate-floor.out"

echo "== a red timing gate under disk pressure"
rc=0; in_stubs STUB_RED=timing-gate GATE_DISK_FLOOR_GIB=999999999 "$timing" g1 "1 2" "$tip" >"$scratch/timing-red.out" 2>&1 || rc=$?
expect "exits 1 (got $rc)" [ "$rc" = 1 ]
expect "leaves the target GC out" grep -qx 'target-gc skipped: the timing run is red (exit 1)' "$scratch/timing-red.out"

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
