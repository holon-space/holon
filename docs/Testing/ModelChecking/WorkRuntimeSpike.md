# Spike report: generate the TLA+ behaviour of a net from the net file

Base: 725b48f490c9. Net: `crates/holon-net/nets/work-runtime.yaml`.

## Answer

- Go. A generator turns the net file into a TLA+ module with no hand-written
  action. The properties, the witnesses and the outside world are the only
  hand-written parts (42 lines of TLA+ against 444 generated lines).
- The executor conforms to the generated module: a conformance PBT replays
  executor calls as spec steps, and an executor mutant is red at its first
  wrong firing. The PBT found two generator mistakes, which are fixed (see
  "The generator").
- The generated module follows the file: removing one inhibitor arc from the
  file turns a property red with a trace.
- TLC found two slot leaks in the net, a dead transition and a subject that a
  definite failure blocks for good (see "Findings").
- The 10-minute bound of H1 holds for every property at a load of 16 to 40 on
  16 cores: the bound of 2 slots and 3 jobs has 402,460 distinct states, and
  one TLC run of one property took at most about 4 minutes. At a load of 50
  to 60, beside other TLC runs, one run took up to about 16 minutes. No run
  was made on a quiet machine.

## What is where

| File | What it holds | Written by |
|---|---|---|
| `WorkRuntimeNet.tla` | The behaviour of the net: one variable per place, one action per firing, the priority rule | Generated |
| `WorkRuntimeEnv.tla` | The outside world: effect outcomes, written probe places, manual firings, timers, resolutions | Hand |
| `WorkRuntime.tla` | Properties, their witnesses, the model bound, the fairness of the timers | Hand |
| `WorkRuntime_*.cfg` | One model each: rule (`exec`, `l0`), world (`pool` or `full`), subjects, pool size | Hand |

- Regenerate: `target/lane-1518/cargo/debug/netgen crates/holon-net/nets/work-runtime.yaml WorkRuntimeNet WorkRuntimeEnv docs/Testing/ModelChecking/WorkRuntimeNet.tla`.
  This command works only in the spike workspace: the generator is a scratch
  crate, and the follow-up card
  `target/lane-1518/followups/1-land-net-to-tla-generator.md` lands it.
- The generator and the conformance PBT are not in the tree. They are a
  scratch crate:
  - Generator: `target/lane-1518/netgen/src/lib.rs`, binary `target/lane-1518/netgen/src/bin/netgen.rs`.
  - Conformance PBT: `target/lane-1518/netgen/tests/conformance.rs`.
- Logs: `target/lane-1518/r2/*.log` (not tracked). Each log starts with the
  base, the tree and the start time. Logs directly under `target/lane-1518/`
  and under `target/lane-1518/stale/` and `target/lane-1518/stale2/` come from
  earlier generations of the module and are no evidence.
- The generator needs a read-only accessor on `NetDef` (`places()`,
  `transitions()` in `crates/petri-workflow/src/exec.rs`). It is not in the
  tree: it lands with the generator. The patch:
  `target/lane-1518/netdef-accessor.patch`. The PBT logs ran with it applied.

## The generator

- Input: the executor's own loader (`petri_workflow::load::load`), never a
  second YAML parser. `holon_net::load` is a one-line facade over the same
  loader; the scratch crate calls the loader directly because a dependency on
  `holon-net` builds the whole workspace through its workspace-hack.
- The loader needs effect bodies and a bind closure. The generator gives each
  effectful transition an effect and each pure transition a body that emits
  one output per declared output place. It reads the net shape through the new
  accessor only.
- Mapping:
  - A place is a function from subject to presence (`single`, `register`), to
    a token count (`fifo`), or a shared count (`fifo` with a pool). Token
    values are not modelled: the PBT gives every write a fresh value, so a
    write always moves the row version.
  - A transition is one action per subject. Enabling is the arcs, the
    run state of an effect (running, retry pending, failed-on-this-input mark),
    and the priority rule.
  - An effectful transition is a start action and one action per outcome:
    done, failed, failed and retryable, unknown, plus the timer actions (run,
    abandon, retry due) and the resolutions of a parked attempt (verify, retry,
    drop).
  - A probe place has a write and a retract action. The environment module
    says which places the world writes.
  - The actions come in four groups: `AutoNext` (auto firings, the fair
    part), `ManualNext`, `TimerNext` and `CallNext` (outcomes, probes,
    resolutions). A group with no action is `FALSE`.
- The environment is a separate module that the generated module `EXTENDS`.
  The contract asked for `INSTANCE`; tla-checker (the in-process step checker)
  does not resolve a named instance (`Env == INSTANCE ...` fails with
  "instance Env not found"). `EXTENDS` keeps the same separation: the
  generated module refers to `Outcomes`, `Written`, `Retracted`, `Manual`,
  `Timers` and `Resolutions` only.
- One refinement came from the conformance PBT: when the run timer of a
  transition is not longer than its abandon timer, the run timer ends the run
  first, so the abandon action requires that no attempt is in flight. Without
  it the spec let an attempt be abandoned while the executor still counted it
  in flight, and a later retry resolution had no spec step.
- A second refinement came from the fixed cases of the parked resolutions:
  the executor keeps the read trace of an attempt across the park and checks
  the row versions when it records the result. The generator reset the
  moved-row set at the park and let the retry and verify resolutions choose it
  freely. The replay took the empty choice at a retry, and a later stale
  success had no spec step. The generator now carries the moved rows across
  the park: a write or retract of a read row while the attempt runs or is
  parked adds the row, a retry keeps the set, and verify records against it
  and then clears it (diff `target/lane-1518/r2/generator-fix.diff`; red
  before the fix: `target/lane-1518/r2/f3-fixed-cases-red-round1.log`).

## Hypotheses

### H1, generation: yes; the 10-minute bound only below a high load

- No hand-written action: the module in the tree is byte-equal to the
  generator output (`target/lane-1518/r2/h4-line-counts.log`).
- Bound 2 slots, 3 jobs (`WorkRuntime_exec.cfg`, world `pool`, symmetry over
  subjects, at most 2 waiting tokens and 1 parked attempt per subject):
  - 402,460 distinct states under both rules. The moved-row set that the
    spec now carries across the park makes the space larger than with a free
    choice at resolve.
  - `FailureReturnsInput`, `AtMostPoolJobs` and `NoJobWhileStarting` hold
    under both rules, and `JobEndReturnsSlot` holds under L0.
  - One TLC run of one of these properties took 1 to 4.5 minutes, one run
    after the other, at a load of 16 to 40
    (`target/lane-1518/r2/d5-exec-AtMostPoolJobs.log`, 253 s, the longest;
    the time of each run is its last line). With 2 to 3 other TLC runs beside
    it at a load of 50 to 60, one run took 7 to 16 minutes on an earlier
    module of this size: the time is no property of the state space alone.
- The world `full` (every outcome, probe, timer and resolution) is checked
  with one subject only. It has 211,236 distinct states under the
  executor rule and 207,880 under L0, under 1 minute per property that holds.

### H2, executor conformance: yes

- PBT: `target/lane-1518/netgen/tests/conformance.rs`.
  - It drives the executor with random calls (write and retract of `Agent`,
    `Budget`, `Card`; manual firings; effect results done, failed, failed
    retryable, unknown; clock ticks across every backoff and timer; resolutions
    of parked attempts) on 3 subjects with the file's pool of 4.
  - After each call it records the observed marking as a scenario step, and
    tla-checker searches for a spec path that reaches it.
  - A trace module (written by the test) grants each call exactly one outside
    action and names it, so a call cannot be explained by another action that
    leaves the same marking, and the search between two calls explores auto
    firings only.
  - The attempt count is observed while an attempt is in flight. The
    moved-row set and the unknown flag of a run have no public reading; the
    check leaves them free.
  - Fixed cases run first: 33 of them. Together they replay every call kind
    at least once (each outcome, each resolution, both manual firings, write,
    retract and tick); the test fails when one kind is missing, and dropping
    the drop resolution from the fixed cases makes it red
    (`target/lane-1518/r2/mutant-floor.log`).
  - 27 of the fixed cases park an attempt with an unknown outcome and cover
    each resolution (verify, retry, drop) times each later outcome (done,
    failed, unknown) times each change of a read row while parked (written,
    retracted, untouched). Each must replay its resolution call.
  - A random draw is reproducible: `CONFORMANCE_SEED` sets the seed (else the
    time; the log prints it), `CONFORMANCE_SHRINK` caps the shrink steps
    (default 16), `CONFORMANCE_CASES` sets the number of random cases.
- Green, executor rule: 64 random cases, seed 20261128, after the fixed cases
  (`target/lane-1518/r2/h2-conformance-exec.log`).
- Green, L0 rule: 64 random cases, seed 20261129
  (`target/lane-1518/r2/h2-conformance-l0.log`).
- Each log states how many executor calls the random cases replayed.
- Mutant: an inhibitor arc counts only for a transition that consumes nothing
  (`crates/petri-workflow/src/exec.rs`, the inhibitor check of `bind_input`).
  Red at the fixed case "job beside waiting", call 6: StartJob starts beside
  the Job made by the run that call 6 completes (`target/lane-1518/r2/mutant-inhibitor.log`).
  - The mutant of the contract ("ignore every inhibitor") is no conformance
    kill: the `Stalled` transition inhibits its own output, so the executor
    never quiesces and panics with `NoQuiescence` before any spec check.
- The mutants run in a copy of the tree and are reverted there; each mutant
  log ends with the check that `exec.rs` equals the base plus the accessor
  patch.
- tla-checker matches a step greedily: it keeps the first path that reaches a
  recorded marking and does not backtrack. Each hidden effect that two paths
  can explain needs an observation or a call label; three such harness
  defects were found and closed, each with a fixed case:
  - A tick that fires no timer is a call that changes nothing.
  - A rewrite of a present row leaves the marking as it is, so it explained an
    unknown outcome; the call label closes this.
  - A run timer ends the run of an attempt still in flight; the observed
    attempt count closes this.

### H3, file fidelity: yes

- Mutant net file: `target/lane-1518/h3-work-runtime-no-inhibitor.yaml`
  (StartJob without `inhibits: Job`; diff `target/lane-1518/h3-net-mutant.diff`).
- `NoJobWhileStarting` is violated within seconds with an 11-state trace: a run
  timer ends a run while a second waiting token is ready, the attempt
  succeeds, and StartJob starts again beside the Job
  (`target/lane-1518/r2/h3-mutant-NoJobWhileStarting.log`).
- The original holds under both rules (`target/lane-1518/r2/d5-exec-NoJobWhileStarting.log`,
  `target/lane-1518/r2/d5-l0-NoJobWhileStarting.log`).
- The diff of the generated modules is the inhibitor and what follows from it
  (enabling, the moved-row set) only: `target/lane-1518/r2/h3-spec-diff.txt`.

### H4, only properties by hand: yes

Non-blank, non-comment lines (`target/lane-1518/r2/h4-line-counts.log`):

| File | Lines |
|---|---|
| `WorkRuntimeNet.tla` (generated) | 444 |
| `WorkRuntime.tla` (properties, witnesses, bound, timer fairness) | 31 |
| `WorkRuntimeEnv.tla` (outside world) | 11 |
| `WorkRuntime_*.cfg` (6 models) | 44 |

### H5, composition: yes

- Change: StartJob may no longer report `ok`. The diff touches
  `WorkRuntimeEnv.tla` only (`target/lane-1518/h5-env-diff.log`).
- Effect: the witness `NoJobEnds` holds (no Job is ever made), where it is
  violated with the file's outcomes (`target/lane-1518/r2/h5-no-ok-NoJobEnds.log`,
  `target/lane-1518/r2/d5-exec-NoJobEnds.log`).

## Priority rule (D5)

- One definition, `MayFire(t, s)`, switched by the constant `FiringRule`:
  - `"executor"`: among the auto producers of one place, the preferred enabled
    one blocks the others (the executor's `preferred` set: higher priority,
    then earlier declaration).
  - `"L0"`: an auto pair fires only if no earlier auto transition is enabled
    for any subject (fire one pair, check again).
- In this net the executor rule never binds: no two auto transitions produce
  the same place, so every `Preferred` set is empty.
- What the real executor does instead: it scans the transitions in
  declaration order each round, and the first one takes the shared input. For
  `JobEnded` and `AgentTurnEnded` (the same input arcs) `JobEnded` always wins
  (`crates/petri-workflow/src/exec.rs`, `quiesce`). The executor rule of the
  spec does not model that scan, so it over-approximates.

Results (one TLC log per row and rule: `target/lane-1518/r2/d5-<config>-<property>.log`;
the verdict of each run: `target/lane-1518/r2/matrix-progress.txt`):

Bound `pool` (3 subjects, 2 slots) and `full` (1 subject, every outcome,
probe, timer and resolution). "Holds" is a full search with no error; a trace
length is the number of states TLC printed.

| Property | Kind | exec, pool | L0, pool | exec, full | L0, full |
|---|---|---|---|---|---|
| `AtMostPoolJobs` | safety | holds | holds | holds | holds |
| `NotAllSlotsBusy` (witness) | witness | violated, 12 | violated, 12 | holds (one subject cannot fill 2 slots) | holds (same) |
| `NoJobWhileStarting` | safety | holds | holds | holds | holds |
| `NeverJobAndWaiting` (witness) | witness | violated, 10 | violated, 10 | violated, 10 | violated, 10 |
| `FailureReturnsInput` | action | holds | holds | holds | holds |
| `NoStartFails` (witness) | witness | violated, 8 | violated, 8 | violated, 8 | violated, 8 |
| `SlotsConserved` | safety | violated, 9 | violated, 9 | violated, 9 | violated, 9 |
| `JobEndReturnsSlot` | action | violated, 9 | holds | violated, 9 | holds |
| `NoJobEnds` (witness) | witness | violated, 9 | violated, 9 | violated, 9 | violated, 9 |

| Property | Kind | exec, live | L0, live |
|---|---|---|---|
| `WaitingStarts` (1 subject, `full`, weak fairness of auto firings and timers) | liveness | holds | holds |
| `NeverReady` (witness) | witness | violated, 5 | violated, 5 |
| `WaitingStarts` with StartJob never enabled (teeth) | liveness | violated | violated |

- `WaitingStarts` says: a ready subject starts its job or stops being ready.
  `Ready` holds when a waiting token, the budget and a free slot are there,
  no Job is, and no parked attempt, failure mark or pending retry holds the
  subject. Each of these blocks StartJob in the net, and only a StartJob
  firing or an unfair outside action falsifies `Ready`. So the teeth row,
  the generated module with the enabling of StartJob made `FALSE`, violates
  it.
- A `Ready` that needs the agent and no stall token holds on that module
  too: with the agent present, the fair `Stalled` firing ends the readiness,
  so the property has no teeth (`target/lane-1518/r2/f1-teeth-red-round1.log`,
  the earlier `Ready`). `Ready` therefore names no condition that a fair
  firing other than StartJob can falsify.
- `run.py` runs the teeth row of each liveness property
  (`WORK_RUNTIME_TEETH`) and fails when one holds.

- The property that holds under one rule only: `JobEndReturnsSlot`. It holds
  under L0 and is violated under the executor rule, where `AgentTurnEnded`
  takes the Job and does not return the slot.
- Every other property has the same verdict under both rules.

## Findings

- Slot leak on drop: a parked StartJob attempt that the user drops keeps its
  pool token; the slot is gone for good (`SlotsConserved`, both rules, 9-state
  trace: start, unknown outcome, drop;
  `target/lane-1518/r2/d5-l0-SlotsConserved.log`).
- Slot leak on a stale success: a Budget write while StartJob runs makes the
  attempt stale; the Job is withheld and the pool token is consumed. With the
  drop resolution taken out of the world, `SlotsConserved` is still violated
  by this path (9-state trace: start, Budget write, done;
  `target/lane-1518/r2/stale-nodrop-l0-SlotsConserved.log`).
- Dead transition: `AgentTurnEnded` has the same input arcs as `JobEnded` and
  comes later in the file, so it never fires in the executor and never under
  the L0 rule. Under the spec's executor rule it fires and leaks the slot
  (`JobEndReturnsSlot` violated, `target/lane-1518/r2/d5-exec-JobEndReturnsSlot.log`).
- Waiting tokens stack: while the agent is present, `Stalled` fires again
  after each `Continue`, so every manual `Continue` adds a waiting token
  (the H3 trace shows two).
- A definite StartJob failure blocks its subject for good. The failure mark
  keys on the input of the run, and the returned waiting token stays the head
  of the fifo row; new waiting tokens, a new Budget row and the abandon timer
  do not clear it.
  - TLC: once blocked, the subject stays blocked in every behaviour of the
    one-subject full world (`target/lane-1518/r2/block-StaysBlocked.log`; the
    witness that a subject gets blocked is violated,
    `target/lane-1518/r2/block-NeverBlocked.log`; spec copy with the two
    properties: `target/lane-1518/r2/spec-block/`).
  - The executor: after the failure, three `Continue` firings, a Budget write
    and a 3 h tick start no run, and the spec replays every call
    (`target/lane-1518/r2/mutant-none.log`). With the executor's
    failure-mark check removed, the replay is red at the failure
    (`target/lane-1518/r2/mutant-blocked.log`).
  - `WaitingStarts` leaves it out on purpose: `Ready` requires no failure
    mark, so a blocked subject is never ready. `StaysBlocked` is the property
    that states the block.
- Doc and code differ: `docs/Architecture/NetFileFormat.md` says a `single`
  place refuses a second put loudly; the executor replaces the token on a
  non-fact put and asserts only on a fact put.

## Constructs not expressed

| Construct | How the spec treats it |
|---|---|
| Token values, the bind closure | Left out: places hold presence or counts; every write moves the row |
| Row versions | Abstracted to a moved-row set per run: a write or retract of a read row while the attempt runs or is parked adds the row, as the executor's version check at record time sees it |
| Timer durations, backoff | Over-approximated: a timer may expire at any time once armed |
| Executor scan order within a round | Over-approximated (see the priority rule) |
| Failing pure bodies | Left out: pure bodies never fail |
| Late results of abandoned attempts | Left out |
| Probe places | Hand-written: `NetDef` has no probe list, so `Written` lists them |
| Effect names | Hand-written: `NetDef` has no effect name, so outcomes are keyed by transition |
| `state_probe` receiver | Generated, but this net has no instance, so it is unchecked |
| Urgency of auto firings | Outside actions need `Quiescent`; timers do not (over-approximation) |

## The replay net

- `work-runtime-replay.yaml` needed one generic generator change: a net with no
  effects has no timer actions, and SANY refused the empty disjunction. The
  fix emits `FALSE` for an empty action group.
- With the final generator SANY parses it and TLC checks one subject: 16
  distinct states, no error (`target/lane-1518/replay-netgen.log`,
  `target/lane-1518/replay-sany-after-fix.log`, `target/lane-1518/replay-tlc.log`).
- The generator fix of the moved-row set leaves this module byte-equal: the
  net has no effects (`target/lane-1518/r2/regen.log`).

## Run

```sh
python3 docs/Testing/ModelChecking/run.py --out target/model-checking/logs
```

`run.py` checks the rows of `WORK_RUNTIME`, per model: each property that
holds with its witness on the same model, the slot leak as a known defect,
the row that depends on the priority rule with its reason, and the liveness
property with its witness. Then it checks the teeth row of each liveness
property. The rows alone ran through the two check functions with no
mismatch (`target/lane-1518/r2/runpy-work-runtime.log`). The crash test of
`run.py` passes (`target/lane-1518/r2/run-py-exit-codes.log`).

## Statements of ModelVerification.md this spike refines

[ModelVerification.md](ModelVerification.md) states the hypotheses this spike
tests. Three of its statements needed a condition.

ModelVerification.md now states each condition; the evidence:

- The generated spec follows the net only while the gate regenerates it and a
  conformance PBT checks the executor against it. The generated module
  follows the arcs of the file (H3), but the generator also writes the
  executor's run semantics: failure marks, retries, run and abandon timers,
  parking, row versions. A generator mistake there is drift from the
  executor; the conformance PBT found two (the abandon guard and the moved-row
  set, see "The generator").
- Each recorded call is a search over a few unobserved steps. The evaluation
  of one action is fast, but a recorded executor call hides the auto firings
  and timers it triggered, so each step is a search of up to 5 hidden steps.
  64 random cases with several hundred calls take a few minutes, and a step
  that matches no path costs the whole search.
- TLC needs many minutes for larger bounds under load. For the bound of 2
  slots and 3 jobs one property took up to about 16 minutes on the shared
  machine (H1).

## Go / no-go

Go, with four conditions for the follow-up that lands the generator:

- Land the `NetDef` accessor with it (`target/lane-1518/netdef-accessor.patch`);
  the generator reads the net shape only through it.

- Land the generator as a crate or a binary next to the loader, and run it in
  the gate so that `WorkRuntimeNet.tla` cannot drift from the net file (today
  the module in the tree is regenerated by hand).
- Land the conformance PBT with it. It is the only check that the generated
  module and the executor agree; without it, a generator mistake makes TLC
  prove properties of a net that does not run.
- Keep the model bound small or add a quiet runner for TLC: at a shared load
  of 50 to 60 one property of the 3-subject bound took up to about 16 minutes.

What this does not give: proofs about token values, timer durations or the
executor's scan order within a round (see "Constructs not expressed"), and
anything about a net that has no conformance PBT.

Follow-up card drafts: `target/lane-1518/followups/`.
