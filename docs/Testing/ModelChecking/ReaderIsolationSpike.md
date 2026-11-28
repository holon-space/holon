# Reader isolation during a take: TLA+ spike

The question: does the design of ruling (C) with the parked-take rule hold
over every interleaving within a bound, and does the test model that the code
is checked against have a gap?

## Worked example

`T0` takes the token `k` from `P0[s]`; `k` is hidden while T0's effect runs.
`T1` binds and reads `P0[s]`: empty (a hole made by an open take). T0 fails
and puts `k` back. Under ruling (C), T1's commit waits for T0; the restore
makes T1 stale and T1 binds again, now reading `k`. Without the commit wait,
T1 commits "no input" on a state that never committed (a dirty read). That is
the trace TLC finds in G4.

## Files

| File | What it holds |
|---|---|
| `ReaderIsolation.tla` | Take, bind, record, hold, restore, park, resolve; I1-I7, liveness, witnesses, the case scripts |
| `ReaderIsolationAnswers.tla` | The gate of `Answer.T` and "one effect per ask" |
| `ReaderIsolationSupersede.tla` | The supersede test of a restore, with two takes of one Single row |
| `ReaderIsolationIdle.tla` | The idle rule of asks: an offered ask has its Answer enabled |
| `ReaderIsolationFifoAsk.tla` | An ask work on a FIFO row whose front an open take holds |
| `ReaderIsolationWaitGraph.tla` | The wait graph of takes that read each other's rows |
| `ReaderIsolation_<design>.cfg` | One design or mutant: constants only |
| `ReaderIsolation_case_<net>.cfg` | The net of a group of case tests; `run.py` adds the `Case` constant |
| `ReaderIsolationAnswers_<gate>.cfg` | The production busy set, or the test net's gate |
| `ReaderIsolationSupersede_<rule>.cfg` | The supersede test on the version, or on the slot |
| `ReaderIsolationIdle_<design>.cfg` | Ask shape, I2 variant, and the held-present ruling on or off |
| `ReaderIsolationFifoAsk_<bind>.cfg` | The bind rule of the ask work: current, (d) or (b) |
| `ReaderIsolationWaitGraph_ring<N>.cfg` | A ring of N takes, each reading the row of the next |
| `run.py` (`RI_ROWS`) | One row per check: design, property, expected outcome |

Run: `python3 docs/Testing/ModelChecking/run.py` runs every row with the
other specs. Every row names the outcome it expects; a witness row expects a
violation, so a check that cannot reach its state turns the run red.

## The spec

- Written from the rulings and the invariants (packed brief sections 1 and
  3), not from the executor or the test model. The case scripts are the op
  lists of the case tests; their verdicts come from the verdict table and the
  contract sheet, not from the code.
- One subject holds P0 (capacity Single, Register or Fifo), P1 (T1's input)
  and, for the take of two rows, P2 (Single). T0 takes the front of each row;
  T1 reads them. T0 is a Keyed effect with two attempts. T1 is pure or a
  Keyed effect, with an optional fact output.
- The executor is single-threaded: an outside operation (write, complete,
  step, resolve) runs only when the net is quiet; the internal steps (start,
  bind, record, release) then run until it is quiet again.
- Rules, each one operator:
  - `DependsOn` is I2, one definition with both variants. Narrow (ruled): a
    reader depends on a take only if the take's failure could put its token
    back into what the reader saw (an absent row, or a FIFO front behind the
    held token). Wide: any open take of the row.
  - `PutsBack`: a restore puts the held token back on a FIFO, and on a
    Single or Register row only when no newer token was written there.
  - `CommitTake` (H1): the row's version moves only if the held token
    counted in the committed row; a reader with an edge to the take moves its
    trace version along when nothing else moved the row (I4).
  - `RestoreTake` (H2): a token put back makes every reader with an edge on
    it stale (I5).
  - Occurrence rule: a T0 occurrence fails only when every attempt failed;
    after an unknown attempt a failure parks it. Verify and Drop commit a
    parked take, Retry runs one more attempt.
  - `StaleFirst` (ruled 2026-11-28): the stale check runs before the hold.
    A reader that a write or a restore made stale rolls back and binds
    again at once, also while it is held; it does not wait for the take.
    So I3 holds for commits: no commit with an open dependency.
  - `EndWait` (ruled 2026-11-28): the Condition "T1[s] waits for T0[s]"
    goes away when the reader's wait ends for any reason: the take is
    resolved, the reader settles (also by a definite failure, or by its
    stale rollback), or the reader is dropped.
  - `CommittedFront` is the I6 oracle: the row's tokens plus the token an
    open take holds from it; on a non-FIFO row with an empty head, the newest
    held token. Executor places stay out of it.
- Fact output: a non-stale record puts its fact into the row (compare and
  set); a stale record's fact is refused.

### Modelling choices

- T1's own retries are left out: a failure of T1's effect settles T1 at once
  and marks its input. Parking and resolve of T1 are in.
- An outside write of equal content into a Single or Register row leaves the
  token; any other write replaces it (Single and Register differ only in
  fact puts that no compare and set backs, which this net does not make).
- T0 is one occurrence per subject at a time (its Running place), so two
  takes in flight need two subjects.

## Results

Every row is a line of `RI_ROWS` in `run.py`; its log is
`<out>/ReaderIsolation.<row>.log`. "yes" means TLC gave the expected outcome.

### G1, design: yes

| Check | Rows | Outcome |
|---|---|---|
| I1-I7 (both halves of I7), fact CAS, deadlock freedom | `single`, `register`, `fifo`, `fact`, `purefact`, `tworows`, `twosubjects`, `wide` `.invariants` | hold |
| Liveness: released unless parked, released when resolved, quiesce ends, a stale held reader is released without a resolve | `singlelive`, `registerlive`, `fifolive`, `factlive` | hold |
| Witness per invariant (each excluded state is reachable) | `single.Never*`, `fifo.Never*`, `fact.Never*`, `tworows.Never*`, `twosubjects.NeverWaitsParked` | violated, as expected |
| A reader can wait for ever on a parked take | `singlelive.ReleasedWithoutResolve` | violated, as expected |

- Bound: P0 Single, Register and Fifo, one subject, four outside writes;
  two subjects (two takes, two readers in flight) with two writes each; a
  take of two rows (P0, P2) with a fact into P2, three writes; T0 with two
  attempts and all four outcomes, resolve Verify, Retry and Drop; T1 pure
  or Keyed. The liveness rows use three writes.
- The two-subject row checks that every edge names the take that was open
  at the bind (`I2Dependency`), and has no deadlock. Its subjects share no
  row, so no wait cycle can form there; `ReaderIsolationWaitGraph` (below)
  checks takes whose rows overlap.
- `wide` checks I6 with the wide I2: it holds too; the narrow I2 only
  removes waits.
- Each design rule has a mutant whose row is red:

| Mutant | What it breaks | Row |
|---|---|---|
| `nowait`: no commit wait | I6 (G4) | `nowait.I6Sound` |
| `nostale`: a restore does not make the reader stale | I5, I6 | `nostale.I5TakeFails`, `nostale.I6Sound` |
| `noadvance`: a committed take does not move the reader's version along | I4 | `noadvance.I4TakeCommits` |
| `keepedge`: a committed take keeps the edge | deadlock, release | `keepedge.deadlock`, `keepedgelive.ReleasedUnlessParked` |
| `nocond`: no Condition | I7 | `nocond.I7Disclosure` |
| `oldtrace`: a rebind keeps the old versions | quiesce ends | `oldtrace.QuiesceEnds` |
| `oldcond`: the Condition goes only at resolve | I7, other half | `oldcond.I7NoStaleCondition` |
| `holdfirst`: the hold runs before the stale check | a stale reader is released without a resolve | `holdfirstlive.StaleReleased` |
| `staleatrecord`: the stale check only at record, not for a held reader | the same | `staleatrecordlive.StaleReleased` |

### G2, lowering rule of answers: yes

- `answers.production`: one effect per ask holds with the production busy
  set; `answers.production.NoAskAnswered` (an ask gets its effect) is
  violated, as expected.
- `answers.testnet`: without `OutcomeUnknown` in the gate, an answer gets in
  while the work is parked: two effects for one ask.

### G3, cases: yes, one disagreement with the verdict table

Each case runs the op list of its test as a script; after each named op,
once the net is quiet, its check holds on every interleaving of the internal
steps. Each case also has an `.end` row (the script reaches its end) that
must be violated. With a mutant of the rule a case pins, its check is red.

| Case | Expected (verdict table, or contract sheet) | Spec |
|---|---|---|
| held_token_superseded_by_a_write | stale | stale |
| absent_read_then_written_and_taken | stale | stale |
| dirty_absent_read_records_before_the_taker_fails | waits, then stale | waits, then stale |
| read_of_a_token_written_during_a_take (G) | table: waits, then not stale; sheet: not stale, no wait | not stale, no edge |
| run_state_read_then_the_run_starts (F) | not stale | not stale (an executor place, out of the oracle by construction) |
| fact_into_fifo_appended_behind_head | stale (conservative) | stale |
| fifo_append_behind_the_read_head | stale (conservative) | stale |
| read_then_taken_taker_runs | not stale | not stale |
| read_then_taken_taker_done | stale | stale |
| read_then_taken_taker_fails | not stale | not stale |
| read_while_taken_taker_done | waits, then not stale | edge, then not stale |
| read_while_taken_taker_fails | stale | stale |
| read_while_taken_taker_retries | stale | stale |
| present_fifo_read_whose_held_front_goes_back | stale | stale |
| a_stale_reader_puts_no_fact_over_the_restored_token | stale; the restored token stays in P0 | stale; P0 holds the restored token |
| reader_waits_on_a_parked_take_verify | waits with Condition; commits; Condition gone; the other subject still waits | same |
| reader_waits_on_a_parked_take_drop | as Verify | same |
| reader_waits_on_a_parked_take_retry_then_fails | Retry: Condition gone, still waits; definite fail: parks again, Condition back; Drop: commits | same |
| a_wait_condition_goes_when_the_reader_fails | Condition while T0 is parked; gone when T1 fails definitely (`EndWait` ruling) | same; red under `oldcond` |
| a_reader_stale_by_a_write_is_not_held | stale at record, not held, no Condition (`StaleFirst` ruling) | same; red under `holdfirst` |
| superseded_token_stays_out_when_a_later_take_holds_the_newer_one | P0 empty, reader stale | same (version and slot) |
| superseded_token_stays_out_after_the_newer_one_is_consumed | P0 empty, reader not stale | version: same; slot: k1 comes back, reader stale |
| an ask on a hole while its take runs (Inhibit read) | ruling on: no ask offered, idle rule holds | ruling on: holds; ruling off: the Answer is held and the ask stays offered |
| an Answer of a newer token written during a take (present read) | narrow: commits, idle rule holds | narrow: holds; wide: the Answer is held |

- Case G: with the wide I2 the reader has an edge
  (`case.read_of_a_token_written_during_a_take.wide` is red on its "no
  edge" check). The table row is stale; the sheet's narrowing is right.
- `read_while_taken_taker_done`: in its op order the take commits before
  the reader's effect ends, so the reader has an edge but is never held.
- The last four rows are TLC traces of the modules below, not scripts of
  case tests: the supersede cases are scripts of `ReaderIsolationSupersede`;
  the two ask rows are the counterexamples of `idle.absent_off.IdleRule`
  and `idle.wide_off.IdleRule`, in the op order of their source.
- The property test of the isolation machine,
  `a_read_through_a_failed_take_is_stale`, is no case; its operations are
  the outside operations of the spec.
- `a_reader_stale_by_a_write_is_not_held` makes the reader stale before
  its effect ends, so it is never held; it stays green under the mutant
  `staleatrecord` and does not pin the open finding below.
- Case tests out of the bound (one take per FIFO row per subject, a fact
  into one row), each covered by its static example only:
  - `a_refused_retry_keeps_the_wait_condition`: a second take of one FIFO
    row parks and is retried.
  - `committed_front_is_the_oldest_token_of_a_later_take`: two takes of one
    FIFO row; the committed front is chosen by token id.
  - `committed_front_is_a_restored_token_older_than_a_held_one`: the same,
    see G5.
  - `failed_takes_put_fifo_tokens_back_in_write_order`: two takes of one
    FIFO row both fail; the order of the restores.
  - `a_stale_reader_puts_no_fact_over_either_restored_token`: a fact into
    two rows; the spec puts a fact into one row
    (`a_stale_reader_puts_no_fact_over_the_restored_token`).
- Helpers of the test file, no case tests: `x_of_p0`, `committed_x_of_p0`,
  `taker_runs`, `fact_output`, `spec`, `outputs`, `net`, `name`, `op`,
  `is_t1`, `of`, `quiesced`, `new`, `p0_ids`, `settled`, `apply`,
  `resolve`, `observe`, `check`, `t1_held`, `verdict`, `init_state`,
  `transitions`, `init_test`, `check_invariants`, `w`, `wx`, `done`,
  `fails`, `case`, `g_ops`, `beside`, `p0_order`, `committed_read_of_t1`,
  `t1_waits_on_parked_t0`, `park_t0`, and `reader_waits_on_a_parked_take`
  (the body of the three parked-take cases).

### G4, not vacuous: yes

`nowait.I6Sound`: write P0, T0 takes k, write P1, T1 binds the hole, T1's
effect ends Done, T1 commits "no input" while T0 still holds k. That is the
dirty read of the worked example; I6 fails at the commit.

### G5, test-model conformance: waiting for the final commit of the commit-wait change

The commit-wait change is still open: it owes the release of a reader that
a write makes stale while it is held (the open finding below). The random
step check runs on its final commit, on a card that the lane lead files.
The rule map below is the part done so far.

At commit 104f32fe (the commit-wait change after its fix round), each spec
rule against the executor (`crates/petri-workflow/src/exec.rs`) and the test
model (`crates/holon-net/tests/exec_support/model.rs`):

| Spec rule | Executor | Test model | Agrees |
|---|---|---|---|
| I2 narrow (`DependsOn`) | `held_from_read_rows` :3669: an edge to each open take of a plain-read row whose token is not superseded | `waits_on` from the same filter :1390 | yes |
| Held present for asks (`AskView`) | `Reader::committed` :1511 reads `committed_front` | `committed_view` :343 | yes |
| Version supersede (`Superseded`) | `superseded` :3750: not FIFO and version /= `taken_at` | `superseded` :1936 | yes |
| Committed front (`CommittedFront`) | `committed_front` :3759: FIFO, the oldest of the held and the row's tokens by id; else the row's token, else the newest held | `committed_token` :1645, same | yes |
| Rule (d) | `decide` :3539: an asked work needs no `front_taker` (:3786) | `front_held` :1017, `front_taker` :1625 | yes |
| Condition lifetime (`EndWait`) | `derive_parked_waits` :3831: derived again from the waits that still exist | `derive_parked_waits` :2000 | yes |
| Stale before hold (`StaleFirst`) | `record` :3962: holds only a reader that is not stale | same | at record only: see Findings |
| Failure mark | `fail` :4298 calls `set_mark` :4281; `decide` :3509-3515 refuses an Auto re-fire on the same read trace | `set_mark` :1951 | yes |
| `max_attempts` | `run_failed` :4324 | :1973 | yes |
| Restore | `rollback` :4267, `Tokens::restore` :1248: the supersede test; a FIFO token goes back at its token-id place | `rollback` :1912, same | yes (the spec moves the version on a restore; the verdicts agree) |

A FIFO row's committed front is the oldest of the held tokens and the
row's own tokens, so a restored token older than a held one is the front.
The spec's `CommittedFront` takes the minimum over both. The bound cannot
reach that state (one take per FIFO row per subject, so no token is held
while an older one is restored); the static example
`committed_front_is_a_restored_token_older_than_a_held_one` covers it.

Not run yet: the random step check of the test model against the spec
(runs of model.rs, each step checked as a spec step, with a planted model
mutant). It waits for the final commit of the commit-wait change.

### Supersede test of a restore

`ReaderIsolationSupersede.tla`: one Single row, two takes, one reader. The
supersede test is one definition used by the restore, by the edge test
(narrow I2) and by the committed front. Two variants:

- `version`: superseded iff the row version is not the version the take
  recorded. `supersede.version` holds `NoRevive` (a restore never brings
  back a token older than a written one) and I6; both cases hold.
- `slot`: superseded iff the row holds a token or another open take holds
  one. `supersede.slot` breaks `NoRevive` and I6. Trace (the second case):
  T0 takes k1; write k2; T2 takes k2; the reader reads the hole; T2 Done;
  T0 fails: nothing holds a newer token, so k1 comes back after k2 was
  consumed, and the reader is stale.

The take of `ReaderIsolation.tla` uses the version rule too (`PutsBack`:
no newer token was written); with one take per subject the two rules agree
there.

### Idle rule of asks

`ReaderIsolationIdle.tla`, `IdleRule`: an offered ask has its Answer
enabled. The ask's derive does not count the Answer's own Running place.

| Ask reads | I2 | Held present | IdleRule | I6 |
|---|---|---|---|---|
| Item present | narrow | off | holds | holds |
| Item present | narrow | on | holds | holds |
| Item present | wide | off | fails: write k1, Grab takes k1, write k2, Answer reads k2 and is held | holds |
| Item present | wide | on | fails: write k1, Grab takes k1, Answer reads the held k1 and is held | holds |
| Item absent | narrow | off | fails: write k1, the take takes k1, the ask reads the hole, the Answer is held | holds |
| Item absent | narrow | on | holds | holds |

- The held-present rule reads the committed front, so it is one function
  with the supersede test.
- Finding: "Answers never wait" under the held-present rule holds only with
  narrow I2. With wide I2 an Answer of the held token still has an edge.
  Narrow I2 is the ruled variant, so this is no gap.
- Latent in production: no production manual transition consumes a row
  that an ask reads.

### Wait graph of takes whose rows overlap

`ReaderIsolationWaitGraph.tla`: N transitions in a ring; transition t
takes the token of row t and reads row t+1. Each is a take and a reader of
another take's row, so a wait cycle is one edge set away. Bind, hold,
commit, restore and the stale check are the rules of `ReaderIsolation.tla`
over Single rows: at its bind a reader gets an edge to each take open then
whose token it saw as a hole.

| Row | Result |
|---|---|
| `waitgraph.ring2`, `waitgraph.ring3`: `NoWaitCycle`, no deadlock | hold (ring of 3: 1,222,411 states) |
| `waitgraph.ring<N>.NeverCrossEdge`: t waits on u while u reads t's row | violated, as expected |
| `waitgraph.ring<N>.NeverHeld` | violated, as expected |

- Why no cycle forms: an edge goes from a reader to a take open at its
  bind, so it points to an earlier take; a cycle would need a take earlier
  than itself.
- Mutant: Bind also gives each live reader of the row it takes an edge to
  this later take. `NoWaitCycle` is violated on both rings: T1 takes row 1;
  T2 takes row 2 and binds the hole of row 1 (edge T2 to T1); the mutant
  adds T1 to T2, since T1 reads row 2.

### Ask work on a FIFO row

`ReaderIsolationFifoAsk.tla`, `AskConsumesWhatItSaw`: each committed ask
work consumed the token its answered situation saw.

| Bind rule | Result |
|---|---|
| current: the work pops the visible head | fails: write k1, write k2, T0 takes k1, the Answer reads k1, the work pops k2 |
| (d): enabled only while no open take holds the committed front | holds, no deadlock |
| (b): the ask is hidden while a take holds the front | fails: the Answer first, then T0 takes k1, the work pops k2 |

Under (d) the work waits while T0 is parked and settles once a committing
resolve is weakly fair; without that it may wait for ever. It settles under
weak fairness only because of assumption `A-FailMarkAndAttemptBound` (see
Fairness), which the spec models as guards of T0.

## Findings

Fixed at commit 104f32fe (found against commit 1ff6e144):

- The test model picked the FIFO committed front and its taker by start
  order; now by token id, as the executor and the spec.
- A FIFO restore put the token at the front; now at its token-id place
  (the write order), as ruled on 2026-11-28 and as the spec does.
- The Condition of a parked take went away only at resolve; now it ends
  with the wait (`EndWait`, ruled on 2026-11-28). The old rule `oldcond`
  breaks I7: T1 binds the hole, T0 parks, T1's effect fails definitely,
  and the Condition stays.
- A reader was held before its stale check; now `record` checks first
  (`StaleFirst`, ruled on 2026-11-28). The old order `holdfirst` breaks
  `StaleReleased`.

Open at commit 104f32fe:

- A reader that a write makes stale while it is already held stays held
  until its take ends (`release_held` releases a held reader only when its
  edges are gone). With the spec mutant `staleatrecord` (the stale check at
  record only), `staleatrecordlive.StaleReleased` is violated: T1 binds the
  hole and is held on T0; a write to P0 makes T1 stale; T0 parks; T1 waits
  for a resolve that may never come. The spec releases a stale held reader
  at once. Ruled on 2026-11-28: the release round takes the stale path for
  every held reader that a write makes stale, and drops its edges and its
  wait Condition.

## Fairness

Each liveness property holds under these assumptions, each kept by an
executor step:

- Every internal step runs (weak fairness): `quiesce` loops to a fixpoint.
- Every running effect completes (weak fairness): the runner replies, or the
  run timer completes it.
- A due retry runs (weak fairness): `step` moves the clock.
- `A-FailMarkAndAttemptBound`: the executor does not let an auto take T0
  keep the front of a FIFO row by failing and taking it again. Two
  mechanisms keep it; `ReaderIsolationFifoAsk` models each as a guard of
  T0 (`Mech = "executor"`):
  - the failure mark: a definite failure marks the run's read trace
    (`set_mark`, `crates/petri-workflow/src/exec.rs:4281`), and `decide`
    refuses an auto re-fire on the same read trace (`FailedOnThisInput`,
    `exec.rs:3509-3515`), so the ask work W0 fires next;
  - the attempt bound: a retryable failure retries only while attempts <
    `max_attempts` (`exec.rs:4324`), after a backoff (`RetryPending`,
    `exec.rs:3507`).
  - The executor is not fair between enabled transitions (`quiesce` fires
    in declaration order, `exec.rs:2874`); the property does not need it.
  - Evidence: `fifoask.d.WorkSettlesWhenResolved` holds under weak fairness
    only. Without the failure mark, `fifoask.dnomark.WorkSettlesWhenResolved`
    is violated: T0 fails definitely, k1 goes back, T0 takes k1 again, for
    ever. Without both, `fifoask.dnomech.WorkSettlesWhenResolved` is
    violated (the operator retries a parked T0 for ever).
  - The anchors are at commit 104f32fe.
- Resolve has no fairness: it is the operator's call. `ReleasedWhenResolved`
  assumes strong fairness of a committing resolve (Verify or Drop) in its
  hypothesis; `ReleasedWithoutResolve`, the same claim without it, must fail.

## Can this spec be the engine module of a generated net spec

Not as it is. It fixes a net of two transitions with hand-written per-
transition state. The rules are operators over one row and one take
(`DependsOn`, `PutsBack`, `CommittedFront`, `CommitTake`, `RestoreTake`), so
an engine module with generic transitions and rows could take them over; the
per-transition state (`t0`, `t1`) would become a function over transitions.

## Not checked

- G5 step check: random runs of the test model, each step checked as a spec
  step, with the planted mutant. Waits for the final commit of the
  commit-wait change.
- The case scan and the case-table rows run again at that final commit: the
  scan here ran at 104f32fe, and the final commit may add case tests.
- Two takes of one FIFO row in one subject (the findings of G5 come from
  reading the code).
- Retract of a row token: the isolation machine has no retract operation.
