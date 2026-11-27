# Spike report: does TLC find write-back and settle races before verifiers do?

Card: #1490. Base: ffa1611f40950b386f80932fb0e8e521c5af8e4b.

Output: target/lane-1490/r2/numbers.md
Output: target/lane-1490/r2/run.log

## Answer

- Partly. TLC replays the #600 class from shipped pre-fix code (`b452395b`),
  with the user-level properties only.
- TLC did not replay #1435 from shipped code: the real pre-fix waiter
  (`ffa1611f` `spin`, `7af013ce`) is green. TLC finds the #1435 class only on
  two designs that no commit held: a hybrid (`nowake`) and a verifier's mutant
  (`late`).
- On main: TLC found a fold-level window, settle level open (#1506). A feed
  change that the `home_by` fold has not handed to the write-back consumer
  reads as settled in the model. It is not counted as a confirmed bug: no
  test has shown it at the settle level yet. The open convergence-predicate
  diff (#1466) closes it in the model: the spec of main with that diff is
  green.
- Recommendation: adopt, for concurrency cards that change a wait, a settle or
  a hand-over between tasks (details at the end).

## What is where

- Specs, configs, the run script and tool notes: this folder (see `README.md`).
- Run: `python3 docs/Testing/ModelChecking/run.py`.
- Logs of the runs, the numbers file and the probes: `target/lane-1490/`
  (not tracked).

## The specs

### Shared properties (`SettleProperties.tla`)

Every spec instantiates the same three properties. None of them names a
race-specific variable.

- `SettledReadsNewest == returned => output = newest`
- `EveryOwedRenderWritten == \A v \in 1..Writes : [](newest >= v => <>(output >= v))`
- `SettleReturns == [](settling => <>returned)`

- `SettledReadsNewest`: after a settle returns, the disk (or the frame) holds
  the newest render.
- `EveryOwedRenderWritten`: every owed render is written.
- `SettleReturns`: a settle that starts returns. A missed wake does not show
  in the other two: the waiter hangs while the pipeline completes. This third
  property is the user-level form of "the test does not hang".

### Bounds

- bound.writes: 3
- bound.blocks: 1
- bound.documents: 1
- bound.debounce_firings: unbounded (FlushTick enabled whenever armed)
- The editor writes only between settles, as a PBT transition does. Every
  pipeline step of another actor can fall between any two steps, so the
  three writes can interleave with the debounce, the fold and the watcher
  read-back in every order. The read-back reads the file at processing time,
  so it can arrive before, between and after two renders.

### `Writeback.tla`: org write-back of one block

Action and the code it models (file:function at `ffa1611f`):

- EditorWrite: Modelling choice: any operation that commits a block edit
- FeedApply: `crates/holon-api/src/live_data.rs` `subscribe` (apply actor)
- FoldTake, FoldHand: `crates/holon-api/src/live_data/home_by.rs` `home_diffs_with_progress`; `crates/holon-api/src/live_data/supervision.rs` `run_supervised`
- ConsumerRoute: `crates/holon-orgmode/src/di.rs` `register_org_file_sync_core` (write-back consumer task)
- LoopRerender, FlushTick: `crates/holon-orgmode/src/di.rs` `run_file_sync_controller`; `crates/holon-filesystem/src/file_sync_controller/writeback.rs` `owe_render`
- NetWrite: `crates/holon-filesystem/src/file_sync_controller/writeback.rs` `drive_writeback_net`
- FileEvent (echo suppression): `run_file_sync_controller` file arm; `crates/holon-filesystem/src/file_sync_controller/writeback.rs` `read_disk`
- PollPass: `crates/holon-orgmode/src/di.rs` `poll_pass`
- SettleStart, SettlePoll: `crates/holon-integration-tests/src/pbt/convergence.rs` `converge_signals`, `owed`, `writeback_owed`

Designs (constant `Design`), each with the commit that held it:
- `prefix`: the settle of `b452395b` (shipped), the parent of the #600 fix `3b10d7bb1b`.
  Quiet = no moved CDC watermark, feed seq or tick, and no fold or
  pass-in-flight flag, over two reads.
- `main`: the settle of `ffa1611f` (shipped): the stages of `owed` and `writeback_owed`.
- `through`: main plus the write-back feed cursor of the open #1466 diff. No
  commit on main holds it; the open diff does.

### `SettleWake.tla`: a windowed settle that waits for a change

Action and the code it models:

- Emit: Modelling choice: an engine write and its wake
- Apply: `crates/holon-api/src/live_data.rs` `DeliveryCursor::applied_through`; `crates/holon/src/api/ui_watcher.rs` `begin_render`
- ChildEnd: `crates/holon-api/src/live_data.rs` `OwedChild::drop`
- Check: `frontends/gpui/tests/pbt_harness/settle.rs` `settle_window`
- Arm, Wake: Modelling choice: main has no change wait; the shape is the one of the open #1435 diff

Designs (constant `Waiter`), each with the commit that held it:
- `spin`: `ffa1611f` and `7af013ce` (shipped pre-fix code of the #1435
  class). `settle_window` checks again at once and never parks.
- `fixed`: `813caed2` (the #1435 lane tip, not on main): subscribe, check,
  park; the `OwedChild` drop wakes.
- `nowake`: never shipped, a hybrid. The waiter of `fixed` with the wake
  sources of `7af013ce`. The change wait and the drop wake arrived together
  in `db6074bc`, so no commit had a parking waiter without the drop wake.
- `late`: never shipped, a mutant. The waiter checks, then subscribes (the
  subscribe-late mutant of the second verify round of #1435).

## First replay: an owed render reads as quiet (#600, #610)

Pre-fix design: `b452395b` (`Writeback.tla`, `Design = "prefix"`).
TLC violates `SettledReadsNewest`. Its shortest trace
(Writeback_prefix.SettledReadsNewest.trace_states: 4):

- `Init newest=0 feed=0 disk=0`
- `EditorWrite newest=1`: the edit commits to SQL
- `SettleStart phase=poll`: converge_signals takes its baselines
- `SettlePoll phase=returned disk=0`: no flag raised, nothing moved: settled

- From EditorWrite to SettlePoll: the CDC batch is emitted but the block feed has not applied
  it. The pre-fix settle reads only change of the CDC watermark, so a batch
  emitted before the settle started is invisible. The doc comment of
  `read_when_written` in
  `crates/holon-integration-tests/tests/dense_patch_engine_exact.rs`, added
  by the fix `3b10d7bb1b`, names this window: "between the edit landing in
  SQL and the write-back loop being handed the render ... every settle
  returns".

The window that #610 names, a render that is queued, is among the violating
returns. A shortest path to it in the state graph of the pre-fix design
(`target/lane-1490/path-rerender-channel.log`, made by
`target/lane-1490/path.py`):

- `Init`
- `EditorWrite newest=1`
- `FeedApply feed=1 srcPending`: live_data.rs subscribe
- `FoldTake folding`: home_diffs_with_progress
- `FoldHand supChan=[1]`: run_supervised tx.send
- `ConsumerRoute queued=1 rrChan=[1]`: write-back consumer: queued.queued
- `SettleStart`
- `SettlePoll returned, disk=0`: pass_in_flight and the fold flag are down

Violating settle returns of the pre-fix design, by the stage that holds the
newest edit:

- Writeback_prefix.window.cdc-hop: 132
- Writeback_prefix.window.fold-source: 118
- Writeback_prefix.window.supervisor-channel: 118
- Writeback_prefix.window.rerender-channel: 192
- Writeback_prefix.window.net-behind: 175
- Writeback_prefix.window.fold: 0
- Writeback_prefix.window.bulk-armed: 0
- Writeback_prefix.window.returns: 735

The fold and the bulk debounce show no return: the pre-fix settle reads both
flags. `windows.py` counts every distinct return state in the full state graph
that TLC dumps (`-dump dot,actionlabels`), so the counts are exact for the
bounds.

## Second check: a settle waiter misses its wake (#1435 class, not a replay)

This is not a replay of shipped code. The shipped pre-fix waiter (`spin`,
`ffa1611f` and `7af013ce`) never parks and is green on all three
properties. The brief's case "check-then-wait in the sim driver" assumed a
pre-fix check-then-wait that no commit held: the sim waits used clock polls
until `813caed2`. TLC finds the class only on two designs that never shipped.

Hybrid `nowake` (never shipped): the wake sources of `7af013ce` (unchanged on
`ffa1611f`) with the change-wait waiter of the #1435 lane (`SettleWake.tla`,
`Waiter = "nowake"`).
TLC violates `SettleReturns` (SettleWake_nowake.SettleReturns.trace_states: 9):

- `Init`
- `Emit newest=1 changes=1`: engine write wakes
- `Apply applied=1 owedChild changes=2`: applied_through; begin_render takes an OwedChild
- `Emit newest=2 changes=3`
- `Apply applied=2 changes=4`
- `SettleStart`
- `Check parked, sub=4`: settle_window: owed() not empty, so it waits
- `ChildEnd owedChild=FALSE shown=2`: OwedChild::drop: the check would pass now, no wake
- `Stuttering`: the waiter stays parked (the hang guard fires)

This is the missed wake that `db6074bc` avoids by adding the change wait and
the drop wake together (`OwedChild::drop` bumps the root cursor's
`release_wake`).

The check-then-wait order itself, mutant `late` (never shipped): TLC violates
`SettleReturns` (SettleWake_late.SettleReturns.trace_states: 14). Its first
steps emit, apply and render the first two writes; from the third write on:

- `Emit newest=3 changes=7`
- `SettleStart`
- `Apply applied=3 owedChild changes=8`
- `Check phase=arm`: owed() not empty; not yet subscribed
- `ChildEnd owedChild=FALSE shown=3 changes=9`: the wake goes to nobody
- `Arm parked, sub=9`: subscribes after the wake
- `Stuttering`

This is the shape of the subscribe-late mutant of the second verify round of
#1435.

Main's spinning waiter and the #1435 lane tip are green on all three
properties.

## The current design (main `ffa1611f`)

`Writeback.tla` with `Design = "main"` violates `SettledReadsNewest`
(Writeback_main.SettledReadsNewest.trace_states: 5):

- `Init`
- `EditorWrite newest=1`
- `FeedApply feed=1 srcPending`: live_data.rs subscribe: consumed_seq = routed
- `SettleStart`
- `SettlePoll returned, disk=0`: every stage of owed() reads settled

Violating returns of main, by stage:

- Writeback_main.window.fold-source: 22
- Writeback_main.window.supervisor-channel: 22
- Writeback_main.window.returns: 44
- every other stage: none (see the numbers file)

Mapped to code:
- fold-source: the feed actor in `crates/holon-api/src/live_data.rs` queues the diff on the signal
  map and stores `last_consumed_seq`. The `home_by` stream runs on the
  supervisor task and has not been polled: `is_folding` is still false,
  `consumed_seq` equals the routed seq, and nothing is queued.
- supervisor-channel: `home_diffs_with_progress` lowers `is_folding` once its
  last diff is yielded; `run_supervised` has sent it into the unbounded
  channel; the write-back consumer has not received it, so `queued_renders` is
  empty.
- In the model, both windows outlast the two reads of `converge_signals`
  when the scheduler delays the supervisor or the consumer task. Whether a
  real settle returns in them is the open settle-level question of #1506.

Classification (contract C3), per main's ruling of 2026-11-27:
- TLC found a fold-level window, settle level open (#1506). Not a model
  error, and not counted as a confirmed bug: #1506 owns the settle-level
  oracle that would confirm it.
- The fold-level patch `target/lane-1490/red/feed-change-reads-as-owed.patch`
  is not a red-first test of this window.
  - It is a property test in `crates/holon-api/src/live_data/home_by/tests.rs`
    (module `supervision`): after each drawn feed change, before the
    supervisor runs, the fold's progress flag or the consumer's channel must
    show the change. It is red on `ffa1611f`.
  - Its oracle, as the invariant `RedTestOracle` of the `through` design (the
    design that closes the window), is violated too
    (Writeback_through_oracle.RedTestOracle.result: violated, in
    Writeback_through_oracle.RedTestOracle.trace_states: 3). The oracle reads
    a component state that every unpolled fold task reaches, whatever the
    settle reads, so the test cannot turn green after the fix.
  - `run.py` runs this oracle row with the expected verdict "violated"; a
    future red-first test of #1506 replaces it with a row expected to hold.
- Owner of the fix: #1466 (open, in progress). Its diff adds `Homed::Through` and a
  write-back feed cursor that applies only after the consumer took every diff
  before the marker. `Design = "through"` models that, and all three
  properties hold.

### Which actions the open diffs change

- #1466 (diff-1466.patch):
  - FoldTake, FoldHand: the fold yields `Homed::Through(seq)` after the diffs of a burst.
  - ConsumerRoute: the consumer applies the write-back feed cursor through that seq (the `handed` variable).
  - SettlePoll: the cdc/block-feed stage compares the store with `handed`, not with the feed's consumed seq; the net-behind stage reads `writeback_net::converged`.
- #1435 (diff-1435.patch):
  - Check: the waiter parks between failed checks (`spin` to `fixed`).
  - ChildEnd: `OwedChild::drop` wakes the root cursor.
  - Arm, Wake: the new change wait; its subscription comes before the check.

## Quint (contract C5)

`Writeback.qnt` models the pre-fix design of the first replay with the same actions.
`quint run` (random simulation) violates `settledReadsNewest`:

- quint.Writeback.result: violated
- quint.Writeback.trace_states: 10
- quint.Writeback.seconds: 1.0

Comparison:
- Authoring: agent_minutes.quint_model: 0.7 for a translation of a checked
  TLA+ spec, against agent_minutes.writeback_spec: 5.2 for the TLA+ spec
  itself (after the code reading). A fair first-time Quint figure lies in
  between.
- Readability: Quint reads like typed code (records, `List`, `nondet`,
  `any`). It needs every unchanged variable spelled out per action, as TLA+
  `UNCHANGED` does, and its effect checker rejects some plain boolean
  expressions on the right of an assignment until they are parenthesised.
- Checking: `quint run` samples traces and finds a violation, but it is not
  exhaustive, it gave a longer trace than TLC's shortest one, and it checks
  no temporal property. Exhaustive checking (`quint verify`) needs Apalache.
  TLC checks the TLA+ specs exhaustively, liveness included.
- Verdict: TLA+ with TLC for the specs that gate; Quint only where a reader
  prefers it, as a second view.

## Cost (contract C4)

Agent time (from the lane's timestamps):
- agent_minutes.code_reading_and_tools: 5.5
- agent_minutes.writeback_spec: 5.2
- agent_minutes.settlewake_spec: 1.0
- agent_minutes.quint_model: 0.7
- agent_minutes.run_script_and_classifier: 14.7
- agent_minutes.lane_start_to_all_verdicts: 12.6

TLC per spec and property (the run that the numbers file reads, host load high, `-workers 2`, `nice`):

| spec | property | result | distinct states | states generated | seconds |
|---|---|---|---|---|---|
| Writeback_prefix | SettledReadsNewest | violated | 21 | 25 | 3.9 |
| Writeback_prefix | EveryOwedRenderWritten | holds | 50847 | 159752 | 30.2 |
| Writeback_prefix | SettleReturns | holds | 50847 | 159752 | 10.6 |
| Writeback_main | SettledReadsNewest | violated | 34 | 44 | 1.1 |
| Writeback_main | EveryOwedRenderWritten | holds | 17846 | 54875 | 11.4 |
| Writeback_main | SettleReturns | holds | 17846 | 54875 | 7.6 |
| Writeback_through | SettledReadsNewest | holds | 14524 | 43781 | 2.0 |
| Writeback_through | EveryOwedRenderWritten | holds | 14524 | 43781 | 7.4 |
| Writeback_through | SettleReturns | holds | 14524 | 43781 | 4.8 |
| SettleWake_spin | SettledReadsNewest | holds | 80 | 154 | 1.8 |
| SettleWake_spin | EveryOwedRenderWritten | holds | 80 | 154 | 1.5 |
| SettleWake_spin | SettleReturns | holds | 80 | 154 | 1.4 |
| SettleWake_fixed | SettledReadsNewest | holds | 339 | 541 | 1.3 |
| SettleWake_fixed | EveryOwedRenderWritten | holds | 339 | 541 | 1.7 |
| SettleWake_fixed | SettleReturns | holds | 339 | 541 | 2.1 |
| SettleWake_nowake | SettledReadsNewest | holds | 181 | 303 | 1.5 |
| SettleWake_nowake | EveryOwedRenderWritten | holds | 181 | 303 | 2.3 |
| SettleWake_nowake | SettleReturns | violated | 181 | 303 | 2.5 |
| SettleWake_late | SettledReadsNewest | holds | 461 | 738 | 2.2 |
| SettleWake_late | EveryOwedRenderWritten | holds | 461 | 738 | 1.9 |
| SettleWake_late | SettleReturns | violated | 461 | 738 | 1.2 |

The red-test oracle row (outcome row `Writeback_through_oracle.RedTestOracle` of `run.py`):

| config | red test oracle | result | distinct states | seconds |
|---|---|---|---|---|
| Writeback_through_oracle | RedTestOracle | violated | 7 | 1.5 |

- tlc_seconds_all_runs: 100.4
- Writeback_prefix.graph.seconds: 23.4 (state-graph dump for `windows.py`)
- Writeback_main.graph.seconds: 7.0

Races:
- races_known_replayed: 1 (the #600 class from `b452395b`; only replays of
  shipped pre-fix code count)
- races_known_on_unshipped_designs: 2 (the #1435 class on the hybrid
  `nowake` and the mutant `late`)
- races_found_on_main: 0
- fold_level_windows_on_main_settle_level_open: 1 (#1506)
- red_test_patches_red_first: 0 (the fold-level patch fails its oracle row)
- races_new_unowned: 0
- false_alarms: 0
  - No counterexample was a model error. One configuration slip: TLC first
    reported "deadlock" for the parked waiter; the configs now turn deadlock
    checking off so the hang shows as a `SettleReturns` violation.
- Tool fault: TLC's `-continue` mode crashed twice while it rebuilt an error
  trace (`ArrayIndexOutOfBoundsException`). `run.py` stopped with its tool-error exit, as
  designed. The window count now reads the state-graph dump, which needs no
  trace rebuild.

Keeping the specs in step with the code:
- The anchors name modelled_files: 7 files. Over the last thirty days of
  main, modelled_files_commits_30d: 36 commits touched them.
- Most of those commits change no modelled step. The cost lies in noticing
  the few that do: a commit that adds a hop between two tasks, or changes
  what a settle reads.
- Proposed practice: a concurrency card that touches an anchored function
  updates the spec in the same change, and `run.py` runs when a modelled file
  or this folder changes. With the anchors in place, an update is a small
  edit of one action and its predicate.

## Limits of the models

- One block in one document. A bug that needs two documents (a re-home
  between files) is out of reach.
- One settle read is one atomic snapshot. The code reads its stages one after
  another, so the real settle can miss windows that the model cannot.
- The supervised restart and its seed path are not modelled.
- The bulk pass renders the version the feed holds (modelling choice).
- Loro, share projection and the CDC delivery ledger beyond one comparison
  are out of the model.

## Stateright

- Stateright checks a Rust model in Rust, so its actions can call the same
  pure functions as production (the write-back net's predicates, for example)
  and cannot drift on those.
- The tokio tasks, channels and atomics of the write-back pipeline are not
  pure, so a Stateright model would restate them by hand, as the TLA+ spec
  does.
- Worth a later spike for the write-back net, which is already a pure state
  machine; not a replacement for the specs here.

## Recommendation (contract C6)

Adopt, scoped:
- Where in the flow: at the contract stage of a card whose change moves a
  hand-over between tasks, a wake source, or what a settle or wait reads.
  The lane first writes or updates the spec, shows the red trace of the
  current code (or the green run of the planned design), then implements.
- Each counterexample on main becomes a red test at the lowest observing
  layer, as for any other bug. Before it counts, its oracle is checked as an
  invariant of the design that closes the race (a `run.py` oracle row): it
  must hold there.
- Gate: `run.py` runs when this folder or a modelled file changes. It is a
  cargo-free check.
- Not for: logic inside one task, rendering, data formats. The PBTs cover
  those.
- Basis: the #600 class replays from shipped code in minutes of agent time
  and seconds to minutes of TLC; the #1435 class shows on designs that never
  shipped, and the shipped pre-fix waiter is green, so that check guards
  future waiter designs rather than replaying a past bug; the one
  counterexample on main is a fold-level window that an open lane already
  targets, settle level open (#1506); no false alarm.
