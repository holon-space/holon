#!/usr/bin/env bash
# Exercises gate-combine.sh, gate-integrate.sh and gate-timing.sh against
# throwaway jj repos, with `just` and `cargo` stubbed; nothing is built. Exit 0 = every case
# behaves as specified.
set -euo pipefail
# Run as a flow job, the job's variables would reach the real slot script of a nested track, whose
# end then removes the job's TMPDIR and writes into the real job registry.
unset $(compgen -e | grep '^FLOW_')
export GATE_DISK_FLOOR_GIB=0  # the disk guard is the host's, not under test

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
combine="$here/gate-combine.sh"
integrate="$here/gate-integrate.sh"
timing="$here/gate-timing.sh"
real_just=$(command -v just)
# A caller that sourced cargo-shape-env.sh (run-step.sh does) put this checkout's
# shim on PATH; behind the stubs it would keep the gate scripts from adding it in front.
PATH=$(tr ':' '\n' <<<"$PATH" | grep -vxF "$here/cargo-shape" | paste -sd: -)
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
fails=0
live_jobs="$HOME/.cache/holon-flow/runtime/holon-space-holon-work-2/jobs.jsonl"
live_start=$( [ -e "$live_jobs" ] && wc -c < "$live_jobs" || echo 0)
export FLOW_RUNTIME_DIR="$scratch/flow-runtime"

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

# `just` and `cargo` stubs append each call (a `just --evaluate` then runs the real just), its HOLON_PBT_TIMING, the record file of gate-step.sh's runner,
# the build-script env the gate pins and the quiet marker's content to $GATE_PROBE; `just latency-slo-gate` prints $SETTLE_OUT (default: a contention line
# and an admission), `just timing-gate-build` prints $BUILD_OUT
# `just $STUB_RED` exits 1 and `just $STUB_SLEEP` sleeps 2 s; with STUB_RED_NEXTEST set, every nextest run
# fails one org_suite test no known-red row lists; with STUB_BAD_FILTERSET set, it fails as nextest does on a filterset
# it cannot parse. STUB_EXIT="<tool>:<first arg>=<code> ..." makes those calls exit with that code; the call
# STUB_KILL="<tool>:<first arg>" kills the track that runs it. HOME is a scratch dir, so the hold's marker,
# lock, log and slot walk are scratch files and no real build is lowered.
home="$scratch/home" stubs="$scratch/stubs" probe="$scratch/probe.log"
orch="$home/.claude/skills/orchestrator/scripts"
mkdir -p "$orch" "$home/.cache/holon-build" "$stubs"
ln -s "$HOME"/.claude/skills/orchestrator/scripts/* "$orch/"
# A track runs in this test's slot after STUB_SEAT_S seconds, or with GATE_HOLD_FIRST set in the real
# one; the quiet hold is the real one. Each refused take appends "<pid> <epoch s>" to $refusals, and
# STUB_QUIET_RC makes every take fail with that exit.
refusals="$scratch/refusals"
rm "$orch/with-build-slot.sh"
cat > "$orch/with-build-slot.sh" <<EOF
#!/usr/bin/env bash
case "\$1" in
  --quiet-hold)
    [ -z "\${STUB_QUIET_RC:-}" ] || { echo "stub: quiet hold refused" >&2; exit "\$STUB_QUIET_RC"; }
    rc=0; "$HOME/.claude/skills/orchestrator/scripts/with-build-slot.sh" "\$@" || rc=\$?
    [ "\$rc" != 75 ] || echo "\$2 \$(date +%s)" >> "$refusals"
    exit "\$rc" ;;
  --quiet-*) exec "$HOME/.claude/skills/orchestrator/scripts/with-build-slot.sh" "\$@" ;;
esac
[ -z "\${GATE_HOLD_FIRST:-}" ] || exec "$HOME/.claude/skills/orchestrator/scripts/with-build-slot.sh" "\$@"
sleep "\${STUB_SEAT_S:-0}"
HOLON_BUILD_SLOT_HOLDER=\$\$ exec "\$@"
EOF
chmod +x "$orch/with-build-slot.sh"
for tool in just cargo; do
  cat > "$stubs/$tool" <<'EOF'
#!/usr/bin/env bash
echo "$(basename "$0") $* | timing=${HOLON_PBT_TIMING-unset} | record=${GATE_RECORD-unset} | shape=font:${RUST_FONTCONFIG_DLOPEN-unset} sha:${HOLON_BUILD_SHA-unset} time:${HOLON_BUILD_TIME-unset} rustflags:${RUSTFLAGS-unset} | quiet=$(cat "$HOME/.cache/holon-build/quiet" 2>/dev/null)" >> "$GATE_PROBE"
case " $* " in *" --evaluate "*) exec "$REAL_JUST" "$@" ;; esac
[ "${STUB_KILL:-}" != "$(basename "$0"):${1:-}" ] || { kill -TERM "$(ps -o ppid= -p "$PPID" | tr -d ' ')"; sleep 1; exit 1; }
[ "$*" != timing-gate-build ] || [ -z "${STUB_HOLDER_DIES:-}" ] || kill "$HOLON_BUILD_SLOT_HOLDER"
[ "$*" != timing-gate-build ] || printf '%s\n' "${BUILD_OUT:-}"
for kv in ${STUB_EXIT:-}; do [ "${kv%=*}" != "$(basename "$0"):${1:-}" ] || exit "${kv#*=}"; done
[ "$*" != gate-compile ] || [ -z "${GATE_HOLD_FIRST:-}" ] || "$HOME/.claude/skills/orchestrator/scripts/with-build-slot.sh" --quiet-hold "$GATE_PID"
[ "${1:-}" != latency-slo-gate ] || printf '%b' "${SETTLE_OUT-mean boot matview_ddl 1.0ms\nhost admitted\n}"
[ "$*" != "${STUB_RED:-}" ] || exit 1
[ "$*" != "${STUB_SLEEP:-}" ] || sleep 2
[ -z "${STUB_RED_NEXTEST:-}" ] || [ "${1:-} ${2:-}" != "nextest run" ] || { echo "        FAIL [   1.000s] holon-integration-tests::integration org_suite::stub_red"; exit 100; }
[ -z "${STUB_BAD_FILTERSET:-}" ] || [ "${1:-} ${2:-}" != "nextest run" ] || { echo "error: failed to parse filterset"; exit 94; }
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
steps="1 20 23 2 13 17 7 14 5 6 8 9 18 19 15 11 3 4 12 10 22"
sorted_steps=$(tr ' ' '\n' <<<"$steps" | sort -n | tr '\n' ' ')
# Without the caller's HOLON_PBT_TIMING, only the gate scripts can set it.
# The real wrapper asks rustup for rustc; under the scratch HOME rustup would install a toolchain.
# Every call gets a job id of its own (a case may pass its own after it): a track's slot run records
# "<id>.track<n>", and no two runs may record one id twice.
job_prefix="check-gate-scripts.${scratch##*/}" draws=0
in_stubs() { env -u HOLON_PBT_TIMING REAL_JUST="$real_just" HOME="$home" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" PATH="$stubs:$PATH" GATE_PROBE="$probe" HOLON_BUILD_SLOT_HOLDER=$$ GATE_STEP_POLL_S=0.1 FLOW_JOB_ID="$job_prefix.$BASHPID.$((++draws))" "$@"; }
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
  for t in 0 1 2; do in_stubs GATE=g4 GATE_CARDS="1 2" GATE_PID=$$ GATE_SEAT_ASKED_AT="$(date +%s)" "$integrate" --track $t >>"$scratch/tracks.out" 2>&1 || return; done
}

SEED=${SEED:-$RANDOM}; RANDOM=$SEED; echo "drawn cases: SEED=$SEED"
seat=$((RANDOM % 3))
echo "== the integrate run, its tracks granted a seat after $seat s"
rc=0; in_stubs STUB_SEAT_S=$seat "$integrate" g1 "1 2" "$tip" >"$scratch/integrate.out" 2>&1 || rc=$?
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
seat_lines=$(grep '^track [0-9]*: build seat after ' "$scratch/integrate.out" | sort || true)
expect "each track of its own slot names its seat wait, track 0 none (got: $seat_lines)" \
  [ "$(sed 's/ after [0-9]* s$//' <<<"$seat_lines" | tr '\n' ,)" = "track 1: build seat,track 2: build seat," ]
for n in $(sed 's/.* after \([0-9]*\) s$/\1/' <<<"$seat_lines"); do
  expect "a seat wait of $n s is the stub's $seat s" [ "$((n >= seat && n <= seat + 1))" = 1 ]
done
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
expect "exits 1 under its runner (got $rc)" [ "$rc" = 1 ]
expect "names the step red with the classifier's exit 12" grep -qx 'RED STEP \[19\] extra-nextest: exit 12, log target/gate-logs/g3-step-19.log' "$scratch/integrate-org.out"
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

echo "== integrate runs that flow's own judge reads from their rows"
read -ra names <<<"$("$integrate" --steps)"
pre_steps=("${names[@]:0:4}") post=("${names[@]:4}") killable=()
for name in "${post[@]}"; do
  case $name in hand-authored | keystone-smoke | extra-nextest) ;; *) killable+=("$name") ;; esac
done
stub_key() { case $1 in fmt) echo cargo:fmt ;; extra-nextest) echo cargo:nextest ;; featuremap) echo just:cargo-free-checks ;; *) echo "just:$1" ;; esac; }
# Prints "<property> <0|1> <detail>" for the run of job $1 that ended with $2 and printed $3, drawn as
# reds $4, pre-compile red $5, killed step $6, failure before the steps $7.
judge() {
  /opt/homebrew/bin/python3 - "$@" "$tip" "${names[*]}" "$home/.cache/holon-build" "$HOME/.claude/skills/gh-kanban" <<'PY'
import json, os, re, sys
job, rc, out, reds, pre, kill, early, tip, declared, logs, flow = sys.argv[1:]
sys.path.insert(0, flow)
from flow.gate_rerun import ruled_red
from flow.model import Refused

sel = os.environ.get("JUDGE_SELECTED")
on = (lambda name: True) if sel is None else (lambda name: name in sel.split())
timing = os.environ.get("JUDGE_GATE") == "timing"

def expected_timing():
    # The build compiles and the holder dies only in a run of timing-gate-build; step 16 then returns
    # without running even when unselected, since `step` checks the holder first.
    red = {n for n in (r.split(":")[0] for r in reds.split()) if on(n)}
    if early in ("tree", "usage", "shape") or (early in ("compiled", "holder") and on("timing-gate-build")) or "timing-gate-build" in red:
        return "refused", None
    return ("accepted", ["timing-gate"]) if "timing-gate" in red else ("green", None)

def expected():
    if timing:
        return expected_timing()
    # A quiet hold is taken only for a selected quiet-track step; the holder dies only when timing-gate-build runs.
    early_on = early and {"quiet": on("extra-nextest") or on("windowed-gate"), "holder": on("timing-gate-build")}.get(early, True)
    if (pre and on(pre.split(":")[0])) or (kill and on(kill)) or early_on:
        return "refused", None
    red = sorted(n for n in (r.split(":")[0] for r in reds.split()) if on(n))
    return ("accepted", red) if red else ("green", None)

def read(name):
    path = os.path.join(logs, name)
    return [json.loads(l) for l in open(path)] if os.path.exists(path) else []

rows, ends, text = read("gate-steps.jsonl"), read("gate-ends.jsonl"), open(out).read()
mine = [r for r in rows if r["gate"] == job.split(".")[0]]
want, want_red = expected()
if int(rc) == 0:
    got, got_red = "green", None
else:
    try:
        # (1,): flow's red_exits for a stage that declares none, as this repo's integrate stage does.
        got, got_red = "accepted", sorted(ruled_red({"jobId": job, "exit": int(rc), "signal": None}, rows, ends, tip, (1,)))
    except Refused as e:
        got, got_red = "refused", str(e)
print("judged", int((got, got_red if got == "accepted" else None) == (want, want_red)), f"want {want} {want_red}, got {got} {got_red} (exit {rc})")
if int(rc) == 0:
    ran = {r["name"] for r in mine if r.get("job") == job}
    print("ran-all", int(set(declared.split()) <= ran), f"exit 0 while declared steps have no row: {sorted(set(declared.split()) - ran)}")
end = [e for e in ends if e.get("job") == job]
if want == "refused":
    print("own-exit", int(all(e["own"] != 0 for e in end)), f"end rows {end}")
if timing:
    # A red run keeps its build dirs; target-gc prints one of these whenever it runs.
    gc = bool(re.search(r"^(below the floor: |TARGET-GC NOT RUN)", text, re.M))
    print("no-gc-after-red", int(gc == (want == "green")), f"target-gc ran: {gc}")
else:
    printed_green = bool(re.search(r"^INTEGRATE GREEN$", text, re.M)) or bool(re.search(r"^candidate tip: ", text, re.M))
    print("green-lines", int(printed_green == (want == "green")), f"INTEGRATE GREEN or candidate tip printed: {printed_green}")
if want == "accepted":
    red = [r for r in mine if not r.get("skipped") and r["exit"] != 0]
    unnamed = [f"[{r['step']}] {r['name']} {r['exit']}" for r in red
               if not re.search(rf"^step \[{r['step']}\] {re.escape(r['name'])}: exit {r['exit']}$", text, re.M)
               or not re.search(rf"^RED STEP \[{r['step']}\] {re.escape(r['name'])}: exit {r['exit']}(, log |$)", text, re.M)]
    print("true-codes", int(not unnamed), f"red rows whose code the log does not name: {unnamed}")
if want in ("accepted", "green"):
    bad = [f"[{r['step']}] {r['name']} job={r.get('job')} tip={str(r.get('tip'))[:7]} cont={r.get('cont')}" for r in mine
           if not r.get("skipped") and (r.get("job") != job or r.get("tip") != tip or r.get("cont") is not True)]
    print("row-identity", int(not bad), f"rows without the gate's job, tip or cont: {bad[:4]}")
    ok = len(end) == 1 and set(end[0]["declared"]) == set(declared.split()) and (want != "green" or (end[0]["exit"], end[0]["own"]) == (0, 0))
    print("end-row", int(ok), f"end rows {end}")
PY
}
# One integrate run of gate $1, launched $2 (bare: as the script; runner: under gate-step.sh with GATE_FULL=1;
# select: as the script, for a batch whose selection runs every step; part: the same, selecting the steps $part_sel),
# with the post-compile reds $3 ("<step>:<code> ..."), the pre-compile red $4, the step $5 whose track dies, and the
# failure before the steps $6 (tree, disk, usage, tip, quiet, holder: the build slot holder dies at timing-gate-build).
flow_case() {
  local gate=$1 launch=$2 reds=$3 pre=$4 kill=$5 early=$6 rc=0 nv stub="" env=() args prop ok detail
  local job="$gate.integrate.20261112T000000Z" holder=""
  if [ "$early" = holder ]; then
    holder=$(sleep 600 >/dev/null 2>&1 & echo $!); env+=(HOLON_BUILD_SLOT_HOLDER="$holder" STUB_HOLDER_DIES=1)
  fi
  for nv in $reds $pre; do stub+="$(stub_key "${nv%%:*}")=${nv#*:} "; done
  args=("$gate" "1 2" "$tip")
  case $early in
    tree) args[2]=$(cid main) ;;
    disk) env+=(GATE_DISK_FLOOR_GIB=1 STUB_DF_FREE=0) ;;
    usage) args=("$gate" "1 2") ;;
    tip) args[2]=${tip:0:12} ;;
    quiet) env+=(STUB_QUIET_RC=3) ;;
  esac
  [ -z "$kill" ] || env+=(STUB_KILL="$(stub_key "$kill")")
  local launcher=("$integrate")
  case $launch in
    runner) launcher=("$orch/gate-step.sh" run "$integrate"); env+=(GATE_FULL=1) ;;
    select) printf '* every-step\n' >"$scratch/select-all"; env+=(GATE_SELECT=1 GATE_SELECTED="$scratch/select-all") ;;
    part)
      : >"$scratch/select-part"; for nv in $part_sel; do echo "$nv drawn" >>"$scratch/select-part"; done
      env+=(GATE_SELECT=1 GATE_SELECTED="$scratch/select-part"); local -x JUDGE_SELECTED="$part_sel" ;;
  esac
  in_stubs FLOW_JOB_ID="$job" STUB_EXIT="$stub" ${env[@]+"${env[@]}"} "${launcher[@]}" "${args[@]}" >"$scratch/flow.out" 2>&1 || rc=$?
  [ ! -e "$marker" ] || HOME="$home" "$orch/with-build-slot.sh" --quiet-release "$(cat "$marker")" 2>/dev/null
  [ -z "$holder" ] || ! kill -0 "$holder" 2>/dev/null || kill "$holder"
  [ "$launch" != part ] || launch="part[$part_sel]"
  local what="$gate $launch reds=[$reds] pre=[$pre] kill=[$kill] early=[$early]" verdict jrc=0
  verdict=$(judge "$job" "$rc" "$scratch/flow.out" "$reds" "$pre" "$kill" "$early") || jrc=$?
  expect "$what: the judge ran (exit $jrc)" [ "$jrc" = 0 ]
  while read -r prop ok detail; do expect "$what: $prop ($detail)" [ "$ok" = 1 ]; done <<<"$verdict"
}
flow_case f1 bare "hand-authored:1" "" "" ""
flow_case f2 runner "gate-arch:1" "" "" ""
flow_case f3 bare "hand-authored:1 extra-nextest:100" "" "" ""
flow_case f30 select "keystone-smoke:1" "" "" ""
part_sel="fmt gate-arch windowed-gate"; flow_case f31 part "gate-arch:1 keystone-tour:1" "" "" ""
flow_case f32 select "gate-arch:1" "" "" holder
part_sel="timing-gate-build keystone-tour"; flow_case f33 part "keystone-tour:1" "" "" holder
part_sel="gate-arch"; flow_case f34 part "gate-arch:1" "" "" holder
flow_case f35 runner "timing-gate-build:1 gate-arch:1" "" "" holder
k=4
for code in 1 2 11 12 13 14 100 101; do flow_case "f$k" bare "${killable[k % ${#killable[@]}]}:$code" "" "" ""; k=$((k + 1)); done
flow_case f12 bare "" "" "" ""
flow_case f13 bare "gate-arch:1" "fmt:1" "" ""
flow_case f14 bare "keystone-tour:1" "" "gate-arch" ""
flow_case f15 runner "gate-arch:1" "" "" quiet
flow_case f16 bare "gate-arch:1" "" "" tree
flow_case f17 bare "gate-arch:1" "" "" disk
flow_case f18 bare "" "" "" usage
flow_case f19 bare "gate-arch:1" "" "" tip
# A gate whose build slot holder dies after gate-compile: the later steps return without running.
holder_dies_case() {
  local gate=$1 launch=$2 rc=0 h prop ok detail job="$1.integrate.20261112T000000Z" launcher=("$integrate") env=()
  h=$(sleep 600 >/dev/null 2>&1 & echo $!)
  case $launch in runner) launcher=("$orch/gate-step.sh" run "$integrate"); env+=(GATE_FULL=1) ;; esac
  in_stubs FLOW_JOB_ID="$job" HOLON_BUILD_SLOT_HOLDER="$h" STUB_HOLDER_DIES=1 ${env[@]+"${env[@]}"} "${launcher[@]}" "$gate" "1 2" "$tip" >"$scratch/flow.out" 2>&1 || rc=$?
  ! kill -0 "$h" 2>/dev/null || kill "$h"
  expect "$gate $launch holder dies: a gate whose steps did not all run does not exit 0 (got $rc)" [ "$rc" != 0 ]
  while read -r prop ok detail; do expect "$gate $launch holder dies: $prop ($detail)" [ "$ok" = 1 ]; done <<<"$(judge "$job" "$rc" "$scratch/flow.out" "" "" "" holder)"
  grep -E '^(step \[|RED |INTEGRATE|candidate|gate-step)' "$scratch/flow.out" || true
}
for launch in bare runner; do holder_dies_case "f4${#launch}" "$launch"; done
codes=(1 2 11 12 13 14 100 101) earlies=(tree disk usage tip quiet holder) launches=(bare runner select part)
# Sets $code; a command substitution would draw in a subshell, which bash reseeds, so SEED would not replay it.
draw_code() { if ((RANDOM % 3)); then code=${codes[RANDOM % ${#codes[@]}]}; else code=$((1 + RANDOM % 255)); fi; }
for k in 20 21 22 23 24 25 26 27 28 29; do
  reds="" pre="" kill="" early=""
  for name in "${post[@]}"; do ((RANDOM % 5)) || { draw_code; reds+="$name:$code "; }; done
  ((RANDOM % 7)) || { draw_code; pre="${pre_steps[RANDOM % 4]}:$code"; }
  ((RANDOM % 7)) || kill=${killable[RANDOM % ${#killable[@]}]}
  ((RANDOM % 6)) || early=${earlies[RANDOM % ${#earlies[@]}]}
  part_sel=""; for name in "${names[@]}"; do ((RANDOM % 2)) || part_sel+="$name "; done; part_sel=${part_sel% }
  flow_case "f$k" "${launches[RANDOM % 4]}" "${reds% }" "$pre" "$kill" "$early"
done

echo "== timing runs that flow's own judge reads from their rows"
# One timing run of gate $1 (a gate id no other run uses, so its rows are this run's), launched $2 as in flow_case,
# with the step reds $3 ("<step>:<code> ...") and the failure $4 (tree, usage, shape: the shape env refuses;
# compiled: the build step compiles; holder: the build slot holder dies at timing-gate-build).
timing_case() {
  local gate=$1 launch=$2 reds=$3 early=$4 rc=0 nv stub="" env=() args prop ok detail holder="" names
  local job="$gate.timing.20261112T000000Z" launcher=("$timing")
  local -x JUDGE_GATE=timing
  # judge reads the declared step names from $names.
  read -ra names <<<"$("$timing" --steps)"
  cp target/gate-logs/g1-cargo-calls.jsonl "target/gate-logs/$gate-cargo-calls.jsonl"
  for nv in $reds; do stub+="just:${nv%%:*}=${nv#*:} "; done
  args=("$gate" "1 2" "$tip")
  case $early in
    tree) args[2]=$(cid main) ;;
    usage) args=("$gate" "1 2") ;;
    shape) env+=(HOLON_BUILD_TIME=1) ;;
    compiled) env+=(BUILD_OUT="   Compiling holon v0.1.0") ;;
    holder) holder=$(sleep 600 >/dev/null 2>&1 & echo $!); env+=(HOLON_BUILD_SLOT_HOLDER="$holder" STUB_HOLDER_DIES=1) ;;
  esac
  case $launch in
    runner) launcher=("$orch/gate-step.sh" run "$timing"); env+=(GATE_FULL=1) ;;
    select) printf '* every-step\n' >"$scratch/select-all"; env+=(GATE_SELECT=1 GATE_SELECTED="$scratch/select-all") ;;
    part)
      : >"$scratch/select-part"; for nv in $part_sel; do echo "$nv drawn" >>"$scratch/select-part"; done
      env+=(GATE_SELECT=1 GATE_SELECTED="$scratch/select-part"); local -x JUDGE_SELECTED="$part_sel" ;;
  esac
  in_stubs FLOW_JOB_ID="$job" STUB_EXIT="$stub" ${env[@]+"${env[@]}"} "${launcher[@]}" "${args[@]}" >"$scratch/flow.out" 2>&1 || rc=$?
  [ -z "$holder" ] || ! kill -0 "$holder" 2>/dev/null || kill "$holder"
  [ "$launch" != part ] || launch="part[$part_sel]"
  local what="$gate $launch reds=[$reds] early=[$early]" verdict jrc=0
  verdict=$(judge "$job" "$rc" "$scratch/flow.out" "$reds" "" "" "$early") || jrc=$?
  expect "$what: the judge ran (exit $jrc)" [ "$jrc" = 0 ]
  while read -r prop ok detail; do expect "$what: $prop ($detail)" [ "$ok" = 1 ]; done <<<"$verdict"
}
timing_case t1 bare "timing-gate:1" ""
timing_case t2 bare "timing-gate:3" ""
k=0 part_sel="timing-gate-build timing-gate"
for code in 1 2 3 10 11 12 13 14 100 101; do timing_case "t3c$code" "${launches[k % 4]}" "timing-gate:$code" ""; k=$((k + 1)); done
timing_case t4 bare "timing-gate-build:101" ""
timing_case t5 runner "timing-gate-build:101" compiled
timing_case t6 bare "" compiled
timing_case t7 bare "" holder
timing_case t8 runner "" holder
timing_case t9 bare "timing-gate:1" tree
timing_case t10 bare "" usage
timing_case t11 bare "timing-gate:1" shape
part_sel="timing-gate"; timing_case t12 part "timing-gate:1" holder
part_sel="timing-gate-build"; timing_case t13 part "timing-gate-build:1" ""
part_sel=""; timing_case t14 part "" ""
timing_case t15 runner "timing-gate-build:1" holder
timing_case t16 select "timing-gate:1" ""
timing_case t17 bare "" ""
codes+=(3 10) earlies=(tree usage shape compiled holder)
read -ra timing_names <<<"$("$timing" --steps)"
for k in 20 21 22 23 24 25 26 27 28 29; do
  reds="" early=""
  for name in "${timing_names[@]}"; do ((RANDOM % 3)) || { draw_code; reds+="$name:$code "; }; done
  ((RANDOM % 4)) || early=${earlies[RANDOM % ${#earlies[@]}]}
  part_sel=""; for name in "${timing_names[@]}"; do ((RANDOM % 2)) || part_sel+="$name "; done; part_sel=${part_sel% }
  timing_case "t$k" "${launches[RANDOM % 4]}" "${reds% }" "$early"
done

echo "== each track alone"
: > "$probe"
rc=0; tracks_alone || rc=$?
expect "every track exits 0 (got $rc)" [ "$rc" = 0 ]
expect "the loaded-red steps run under the hold" held_by $$
expect "no other step runs under it" held_by_none_other
expect "the hold is released" [ ! -e "$marker" ]
expect "warns of no expired hold" [ -z "$(grep 'quiet hold may have expired' "$scratch/tracks.out")" ]

echo "== the quiet track whose step outlives the hold's age limit"
rc=0; in_stubs GATE=g4 GATE_CARDS="1 2" GATE_PID=$$ GATE_SEAT_ASKED_AT="$(date +%s)" GATE_QUIET_MAX_AGE_S=1 STUB_SLEEP=windowed-gate "$integrate" --track 1 >"$scratch/track-expired.out" 2>&1 || rc=$?
expect "exits 0 (got $rc)" [ "$rc" = 0 ]
expect "names the time after which reds may be load reds" grep -qE '^quiet hold may have expired; reds after [0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:]{8}Z may be load reds$' "$scratch/track-expired.out"

echo "== the quiet track while another job holds the hold"
sleep 60 & other=$!
HOME="$home" "$orch/with-build-slot.sh" --quiet-hold "$other" 2>/dev/null
(sleep 1; HOME="$home" "$orch/with-build-slot.sh" --quiet-release "$other" 2>/dev/null) &
: > "$probe"
rc=0; in_stubs GATE=g4 GATE_CARDS="1 2" GATE_PID=$$ GATE_SEAT_ASKED_AT="$(date +%s)" "$integrate" --track 1 >"$scratch/track-wait.out" 2>&1 || rc=$?
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

echo "== the integrate run of a batch whose extra-nextest args are no filterset"
printf 'extra-nextest r two three\n' > "$selected"
: > "$probe"
rc=0; in_stubs GATE_SELECT=1 GATE_SELECTED="$selected" STUB_BAD_FILTERSET=1 "$orch/gate-step.sh" run "$integrate" g8 "1 2" "$tip" >"$scratch/integrate-bad-filter.out" 2>&1 || rc=$?
expect "exits non-zero (got $rc)" [ "$rc" != 0 ]
expect "names the args" grep -qx 'extra-nextest: GATE_STEP_ARGS is no valid filterset: two three' "$scratch/integrate-bad-filter.out"
expect "names the step red with exit 2" grep -qx 'gate-step.sh: run red: \[19\] extra-nextest (exit 2)' "$scratch/integrate-bad-filter.out"
expect "names no novel red and runs no classifier" [ -z "$(grep -e 'NOVEL' -e '^\[known-reds\]' "$scratch/integrate-bad-filter.out")" ]
expect "makes one cargo nextest call" [ "$(grep -c '^cargo nextest ' "$probe")" = 1 ]

echo "== the integrate run whose extra nextest ends with a usage error"
rc=0; in_stubs GATE_STEP_ARGS="two three" STUB_BAD_FILTERSET=1 "$integrate" g8 "1 2" "$tip" >"$scratch/integrate-usage.out" 2>&1 || rc=$?
expect "exits 1 under its runner (got $rc)" [ "$rc" = 1 ]
expect "names the red step with exit 2" grep -qx 'RED STEP \[19\] extra-nextest: exit 2, log target/gate-logs/g8-step-19.log' "$scratch/integrate-usage.out"
expect "names no novel red" [ -z "$(grep 'NOVEL' "$scratch/integrate-usage.out")" ]
expect "names no candidate tip" [ -z "$(grep '^candidate tip' "$scratch/integrate-usage.out")" ]

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

echo "== a quiet-hold wait behind foreign holds, of the quiet track and of the timing-gate recipe"
hold_log="$home/.cache/holon-build/holds.jsonl" dummies="$scratch/dummies"
# Takes the foreign holds in order, each "<seconds>:<release|die>", each for a fresh pid; stops at the
# first take refused (the waiter took the hold in a gap). Every pid goes to $dummies.
foreign_holds() {
  local h p
  for h in "$@"; do
    sleep 300 & p=$!
    echo "$p" >> "$dummies"
    HOME="$home" "$orch/with-build-slot.sh" --quiet-hold "$p" 2>/dev/null || { kill "$p"; wait "$p" 2>/dev/null || true; return 0; }
    sleep "${h%%:*}"
    [ "${h#*:}" = die ] || HOME="$home" "$orch/with-build-slot.sh" --quiet-release "$p" 2>/dev/null
    kill "$p"; wait "$p" 2>/dev/null || true
  done
}
# Checks a wait's output $1 against the hold log and the stub's refusals; prints "<invariant> <0|1> <detail>".
check_wait() {
  /usr/bin/python3 - "$1" "$hold_log" "$refusals" "$dummies" <<'PY'
import calendar, collections, json, re, sys, time
out, log, refusals, dummies = sys.argv[1:]
dummy = set(open(dummies).read().split())
takes = [json.loads(l) for l in open(log) if '"event":"quiet-take"' in l]
own_i = next(i for i, t in enumerate(takes) if str(t["holder"]) not in dummy)
own = takes[own_i]
foreign = [str(t["holder"]) for t in takes[:own_i]]
text = open(out).read()
printed = re.findall(r"^quiet wait at \S+: with-build-slot: quiet hold: held by pid (\d+)", text, re.M)
any_held = collections.Counter(re.findall(r"quiet hold: held by pid (\d+)", text))
ends = re.findall(r"^quiet wait: ended after (\d+) s; took the quiet hold at (\S+)$", text, re.M)
refused = [int(l.split()[1]) for l in open(refusals) if l.split()[0] == str(own["holder"])]
iso = lambda s: calendar.timegm(time.strptime(s, "%Y-%m-%dT%H:%M:%SZ"))
print("I1", int(printed == foreign), f"holder lines {printed}, foreign takes of the hold log {foreign}")
print("I2", int(len(ends) == (1 if refused else 0) and all(abs(iso(e[1]) - own["ts"]) <= 1 for e in ends)),
      f"end lines {ends}, refusals {len(refused)}, own take ts {own['ts']}")
print("I3", int(all(abs(int(e[0]) - (own["ts"] - refused[0])) <= 1 for e in ends)),
      f"end lines {ends}, first refusal {refused[:1]}, own take ts {own['ts']}")
print("I4", int(all(n == 1 for n in any_held.values())), f"held-by lines per pid {dict(any_held)}")
PY
}
# One wait of caller $1 (track | recipe) behind the foreign holds $2...
wait_case() {
  local caller=$1 rc=0 driver="" inv ok detail; shift
  local what="$caller behind [$*]"
  : > "$hold_log"; : > "$refusals"; : > "$dummies"
  if [ "$#" -gt 0 ]; then
    foreign_holds "$@" & driver=$!
    until [ -e "$marker" ]; do sleep 0.05; done
  fi
  if [ "$caller" = track ]; then
    in_stubs GATE=g4 GATE_CARDS="1 2" GATE_PID=$$ GATE_SEAT_ASKED_AT="$(date +%s)" "$integrate" --track 1 >"$scratch/wait.out" 2>&1 || rc=$?
  else
    in_stubs "$real_just" --justfile "$here/../justfile" --working-directory "$recipe" timing-gate >"$scratch/wait.out" 2>&1 || rc=$?
  fi
  [ -z "$driver" ] || wait "$driver"
  [ ! -e "$marker" ] || HOME="$home" "$orch/with-build-slot.sh" --quiet-release "$(cat "$marker")" 2>/dev/null
  expect "$what: exits 0 (got $rc)" [ "$rc" = 0 ]
  while read -r inv ok detail; do expect "$what: $inv ($detail)" [ "$ok" = 1 ]; done < <(check_wait "$scratch/wait.out")
}
for caller in track recipe; do
  wait_case "$caller"
  wait_case "$caller" 1:release
  wait_case "$caller" 0.7:die 1:release
done
ends=(release die) callers=(track recipe)
for _ in 1 2 3 4 5 6 7 8 9 10; do
  holds=() n=$((RANDOM % 4))
  # From 0.5 s: a shorter hold can fall between two polls of the waiter, which then never sees it.
  for ((i = 0; i < n; i++)); do t=$((5 + RANDOM % 15)); holds+=("$((t / 10)).$((t % 10)):${ends[RANDOM % 2]}"); done
  wait_case "${callers[RANDOM % 2]}" ${holds[@]+"${holds[@]}"}
done
for caller in track recipe; do
  rc=0
  if [ "$caller" = track ]; then
    in_stubs STUB_QUIET_RC=3 GATE=g4 GATE_CARDS="1 2" GATE_PID=$$ GATE_SEAT_ASKED_AT="$(date +%s)" "$integrate" --track 1 >"$scratch/wait.out" 2>&1 || rc=$?
  else
    in_stubs STUB_QUIET_RC=3 "$real_just" --justfile "$here/../justfile" --working-directory "$recipe" timing-gate >"$scratch/wait.out" 2>&1 || rc=$?
  fi
  expect "$caller, a take that fails with exit 3: exits 3 (got $rc)" [ "$rc" = 3 ]
  expect "$caller, a take that fails with exit 3: keeps the slot script's stderr" grep -qx 'stub: quiet hold refused' "$scratch/wait.out"
  expect "$caller, a take that fails with exit 3: names it" grep -qx 'QUIET HOLD NOT TAKEN: with-build-slot.sh --quiet-hold exit 3' "$scratch/wait.out"
done

echo "== a track of its own seat started without its launch time"
# The gate launches every selected track other than 0 through the slot script with
# GATE_SEAT_ASKED_AT set; such a track that lacks it cannot print its seat wait.
seat_track_without_launch_time() {
  local t=$1 rc=0
  in_stubs GATE=g4 GATE_CARDS="1 2" GATE_PID=$$ "$integrate" --track "$t" >"$scratch/no-seat-time.out" 2>&1 || rc=$?
  [ ! -e "$marker" ] || HOME="$home" "$orch/with-build-slot.sh" --quiet-release "$(cat "$marker")" 2>/dev/null
  expect "track $t of its own seat without GATE_SEAT_ASKED_AT: exits non-zero (got $rc)" [ "$rc" != 0 ]
  expect "track $t of its own seat without GATE_SEAT_ASKED_AT: says so" grep -q 'GATE_SEAT_ASKED_AT' "$scratch/no-seat-time.out"
}
for t in 1 2; do seat_track_without_launch_time "$t"; done

echo "== the timing-gate recipe on a settle probe without a contention line"
for case in "host admitted\n|probe line absent — judging" "|the probe round printed neither an admission nor a contention value"; do
  rc=0; in_stubs SETTLE_OUT="${case%%|*}" "$real_just" --justfile "$here/../justfile" --working-directory "$recipe" timing-gate >"$scratch/recipe-noprobe.out" 2>&1 || rc=$?
  expect "settle output '${case%%|*}': exits 0 (got $rc)" [ "$rc" = 0 ]
  expect "settle output '${case%%|*}': names the state '${case#*|}'" grep -q "${case#*|}" "$scratch/recipe-noprobe.out"
done

echo "== a rerun of one gate test, from any caller env"
# The caller's own values of the variables the gate pins, drawn unset or stale: the rerun's
# nextest call sees the gate's values anyway, and lands in the gate's record.
shape_of() { sed -n 's/^cargo nextest run .* | \(shape=[^|]*\) | quiet=.*/\1/p'; }
gate_shape=$(shape_of <<<"$extra_runs")
pbt=$("$real_just" --justfile "$here/../justfile" --evaluate CANON_FEATURES)
: > "$probe"
in_stubs env -u RUST_FONTCONFIG_DLOPEN -u HOLON_BUILD_SHA -u HOLON_BUILD_TIME \
  cargo nextest run --profile load-tolerant --no-fail-fast --no-tests=fail --workspace --features "$pbt" -E 'test(=x)' >/dev/null
expect "a bare cargo rerun builds another shape than the gate ($(shape_of <"$probe"))" [ "$(shape_of <"$probe")" != "$gate_shape" ]
record=target/gate-logs/g1-cargo-calls.jsonl
filters=('test(=x)' 'package(holon-loro) & test(=iroh_advertiser::tests::a_b)' 'binary(integration) | test(/^org_suite::/)')
draw() { if ((RANDOM % 2)); then printf -v "$1" '%04x%04x' $RANDOM $RANDOM; else printf -v "$1" unset; fi; }
rerun_case() {
  local font=$1 sha=$2 time=$3 filter=$4 unsets=() sets=() kv rc=0 calls
  local what="rerun from font=$font sha=$sha time=$time, -E '$filter'"
  for kv in RUST_FONTCONFIG_DLOPEN=$font HOLON_BUILD_SHA=$sha HOLON_BUILD_TIME=$time; do
    if [ "${kv#*=}" = unset ]; then unsets+=(-u "${kv%%=*}"); else sets+=("$kv"); fi
  done
  : > "$probe"; calls=$(($(wc -l <"$record")))
  in_stubs env "${unsets[@]}" "${sets[@]}" "$integrate" rerun g1 "$tip" "$filter" >"$scratch/rerun.out" 2>&1 || rc=$?
  if [ "$time" != unset ]; then
    expect "$what: refused (got $rc)" [ "$rc" != 0 ]
    expect "$what: before any nextest call" [ -z "$(grep '^cargo nextest' "$probe")" ]
    return
  fi
  expect "$what: exits 0 (got $rc)" [ "$rc" = 0 ]
  expect "$what: runs the gate's nextest arm, narrowed by the filter" \
    [ "$(sed -n 's/^\(cargo nextest .*\) | timing=.*/\1/p' "$probe")" = "cargo nextest run --profile load-tolerant --no-fail-fast -E $filter --no-tests=fail --workspace --features $pbt" ]
  expect "$what: in the gate's shape" [ "$(shape_of <"$probe")" = "$gate_shape" ]
  expect "$what: appends its call to the gate's record" [ "$(($(wc -l <"$record")))" = $((calls + 1)) ]
}
# The bare caller of a hand-written rerun script, a shell left from another tip, a release shell.
rerun_case unset unset unset "${filters[0]}"
rerun_case off 0123456789ab unset "${filters[1]}"
rerun_case unset unset 1 "${filters[0]}"
for _ in 1 2 3 4 5 6 7 8; do
  draw font; draw sha; draw time
  rerun_case "$font" "$sha" "$time" "${filters[RANDOM % ${#filters[@]}]}"
done
rc=0; in_stubs "$integrate" rerun g0 "$tip" 'test(=x)' >/dev/null 2>&1 || rc=$?
expect "a rerun of a gate with no record on this checkout exits 2 (got $rc)" [ "$rc" = 2 ]

echo "== a rerun under another RUSTFLAGS than its gate"
cp "$record" target/gate-logs/g9-cargo-calls.jsonl
in_stubs RUSTFLAGS=-Cdebuginfo=0 "$integrate" rerun g9 "$tip" 'test(=x)' >/dev/null 2>&1 || true
rc=0; /usr/bin/python3 "$here/check-compile-shape.py" target/gate-logs/g9-cargo-calls.jsonl >"$scratch/shape-g9.out" 2>&1 || rc=$?
expect "fails the timing gate's shape check (got $rc)" [ "$rc" = 1 ]
expect "which names RUSTFLAGS" grep -q '^ENV DIFFERS: RUSTFLAGS ' "$scratch/shape-g9.out"

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
: > "$probe"
rc=0; in_stubs "$integrate" rerun g1 "$(cid main)" 'test(=x)' >/dev/null 2>&1 || rc=$?
expect "gate-integrate.sh rerun: a tip other than @- exits 9 (got $rc)" [ "$rc" = 9 ]
expect "gate-integrate.sh rerun: before any cargo or just call" [ ! -s "$probe" ]
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

echo "== the live flow registry"
# Every id this harness hands out or derives; another job's lines in the same window are no finding.
expect "holds no job line of this run" [ -z "$( [ -e "$live_jobs" ] && tail -c +$((live_start + 1)) "$live_jobs" | grep -E "$job_prefix|20261112T000000Z|\"id\":\"g[0-9]+\\." )" ]

starts=$(cat "$FLOW_RUNTIME_DIR/jobs.jsonl" 2>/dev/null | grep -c '"event":"job.start"' || true)
expect "this run's registry ($starts job starts) starts no job id twice" [ -z "$( [ -e "$FLOW_RUNTIME_DIR/jobs.jsonl" ] && grep '"event":"job.start"' "$FLOW_RUNTIME_DIR/jobs.jsonl" | grep -oE '"id":"[^"]*"' | sort | uniq -d)" ]

echo "check-gate-scripts: $fails failure(s)"
[ "$fails" = 0 ]
