# Petri-net layers: petri-net, petri-workflow, holon-net

*Target design, ruled. The executor is in `crates/petri-workflow/src/exec.rs`
today; the migration section below carves it into these layers. Ratified
semantics: [ADR 0032](../adr/0032-petri-net-execution-semantics.md) §8, §9.*

The engine has three layers, like UDP, TCP and HTTP:

| Layer | Crate | Knows |
|---|---|---|
| L0 | `petri-net` | an ordinary coloured Petri net |
| L1 | `petri-workflow` (planned) | workflow features: effects, retry, timers, undo, human tasks; nothing of Holon |
| L2 | `holon-net` | Holon: values, the Loro store, Holon names |

## Worked example: one effectful transition

An author writes one high-level transition:

```
FetchFeed
  consumes  Feed
  reads     Settings
  produces  Items
  body      Effectful(receiver = Keyed)
  retry     backoff, max 3 tries
```

L1 compiles it into L0 places and L0 transitions. L0 sees only these:

| L0 place | Holds |
|---|---|
| `Feed`, `Settings`, `Items` | the author's places |
| `Clock` | the current time, one token |
| `FetchFeed.Running` | the moved input token while the effect runs, with its attempt and read trace |

| L0 transition | Trigger | Consumes | Reads | Produces | Bind |
|---|---|---|---|---|---|
| `FetchFeed.take` | Auto | `Feed[s]` | `Settings[s]`, `Clock` | `FetchFeed.Running[s]`, `tries` (planned, not in the code) raised | the author's guard; `tries < 3` (planned, not in the code); no `next_try`, or `next_try` at or before `Clock` |
| `FetchFeed.record` | External | `FetchFeed.Running[s]` | | `Items[s]`, or the candidate place, or nothing (stale-record rule) | caller binding: attempt, outputs |
| `FetchFeed.fail` | External | `FetchFeed.Running[s]` | `Clock` | `Feed[s]` with the cause (planned, not in the code) and `next_try` | caller binding: attempt, cause (planned, not in the code) |
| `Tick` | External | `Clock` | | `Clock` with the new time | caller binding: `now`; guard: `now` is not older than `Clock` |

`FetchFeed.record` is an illustration. Whether the record is one L0
transition or three is open, see
[Record: one transition or three](#record-one-transition-or-three).

What happens:

1. A token arrives in `Feed[s]`. `FetchFeed.take` is enabled. It fires and
   moves the token into `FetchFeed.Running[s]`. It raises `tries` (planned, not in the code) in the
   token's header. The payload and its stamp do not change. A crash between
   the take and the failure therefore counts as a try.
2. `Feed[s]` is now empty, so `FetchFeed.take` is no longer enabled. The
   transition disables itself by firing. This is what the Level-dead law
   requires.
3. The L1 driver sees the new Running token and asks the host to run the
   effect.
4. The host reports the result. The driver fires `FetchFeed.record` or
   `FetchFeed.fail` with a binding that holds the result.
5. `FetchFeed.record` compares the stamps in the read trace with the current
   stamps of the authority rows. All equal: it produces `Items[s]`. Else the
   stale-record rule decides (ADR 0032 §9.2).
6. `FetchFeed.fail` returns the token to `Feed[s]` with the cause (planned, not in the code) and
   `next_try` (the time on `Clock` plus the backoff) in its header. Other
   consumers of `Feed` can take it. The guard of `FetchFeed.take` is false
   until `Clock` reaches `next_try`. The driver fires `Tick` at that time.
   When `tries` (planned, not in the code) reaches 3, the guard stays false.

L0 never reads a system clock, never sees an effect, a retry policy or a Holon name. It sees
places, tokens, arcs, binds and atomic firings.

## Goal and constraints

Goal:

- An ordinary Petri-net kernel behind a cargo-enforced crate boundary.
- Workflow features as a second layer, built on the kernel, independent of
  Holon.
- Holon as the outer layer.

Constraints:

- The executor behaves as today. The existing executor PBT is the oracle.
- Callers keep their imports from `holon-net`. `holon-net` is the single
  instantiation point of the generic layers.
- Latency SLO: p95 interaction to projection below 200 ms.
- No special cases in the firing loop (ADR 0032 NET-run.a).
- One expression language for guards.
- Fail loud. Parse at the boundary. No old code path after a refactor.

## The three layers

### L0 `petri-net`: an ordinary coloured Petri net

- **Tokens.** An opaque token type `T`, addressed by (place, key).
- **Place ids.** Generic. L0 does not name places.
- **Places.** A store trait with the disciplines Single (one token, kept),
  Register (one token, replaced), Fifo and Counted. A pool is a counted place.
- **Stamp.** Each token carries a stamp, and each row has a row stamp. The
  store trait owns the stamp type, so a replicated store can bring its own.
  Every store keeps two laws: equal row stamps mean equal row content
  (stamp-sound), and a token that moves out and back, with no other change
  of the row, gives the same row stamp (move-neutral). The in-memory store
  uses the list of the row's token stamps as the row stamp (rule L); an
  empty row is the empty list, a Counted row uses its count. The stamp of
  the head token alone is not a row stamp: an append to a Fifo row keeps the
  head and changes the content, so it breaks stamp-sound. The shadow-stamp
  spike shows this.
- **Arcs.** Consume, read, inhibit and produce, declared per transition.
- **Bind.** Guard and binding in one predicate. It reads through a view.
- **Footprint.** The view records every row it read, every row it found
  absent and every place it scanned. It also holds the output rows whose
  discipline can refuse. `enabled` returns the footprint also for a disabled
  pair, because a change-driven scheduler must check that pair again when
  one of these rows changes. Reading an undeclared place is a refusal.
- **Fire.** Pure and atomic. It returns the outputs of one firing.
- **Outputs.** A new token, or `Moved { from, at: Front | Back }`: a token
  that the firing consumed, with its stamp, put at the front or the back of
  the row. `Front` is legal only on Fifo places. Ruled in
  [holon-work#1476](https://github.com/holon-space/holon-work/issues/1476#issuecomment-7205138440).
- **Triggers.** Auto and External. An external transition fires with a
  binding that the caller supplies; a manual transition is an external one.
- **No outside writes.** Every change from outside is a firing of an external
  transition. L0 has no put, take or revert API.
- **Firing rule: level semantics, interleaving.** Fire one enabled
  (transition, key) pair, then check again, in priority order. No batched
  rounds. No per-place producer-priority rule. A firing bound turns a run
  that does not settle into an error.
- **Analysis.** Structural analysis over the declared arcs: conflicts, cycles,
  the self-feed check (ADR 0032 §9.3).
- **Why-not.** Structural: which arc or bind blocks a pair.
- **Not in L0:** time, effects, failure, retry, undo, answers, token classes,
  record fields, conditions.

### L1 `petri-workflow`: workflow nets, independent of Holon

- **Typed bookkeeping tokens.** `Token<V>` holds an `Envelope` header
  (`tries` per contribution id (planned, not in the code), `cause` (planned, not in the code), `next_try`) around a `Record<V>`
  payload. The Running, Clock and undo tokens are typed variants, never
  encoded into `V`.
- **The stamp covers the payload only.** A take raises `tries` (planned, not in the code) in the header
  and keeps the payload stamp, so the take is a move that changes no content
  and no version (ADR 0032 REV11-running.a) and still counts the try
  (ADR 0032 §9.1).
- **Time is a Clock place.** Timed transitions read it through read arcs.
  To L0 it is an ordinary place. The driver advances it by firing the
  external `Tick` transition with `now` in its binding, at the due times of
  its next-due index (a rebuildable cache). The time rule, and what is built
  today, are in
  [Principles.md](Principles.md#functional-core-imperative-shell).
- **Compiler.** Compiles each high-level transition into L0 places and L0
  transitions. Features it compiles:
  - effects with the receivers Keyed, StateProbe and None;
  - `<T>.Running` and attempts;
  - the record with the stale-record rule, by stamp;
  - candidate places;
  - facts;
  - retry: a definite failure returns the input to its place with `tries` (planned, not in the code),
    `cause` (planned, not in the code) and `next_try` in the header; the retry guard compares
    `next_try` with the Clock place;
  - run timers and abandon timers, as guards over the Clock place;
  - failure and timeout ports;
  - the fail or restore of a take, which moves the old token back to its
    row:
    - when a newer token was written to the row while the take ran, a
      Single or Register row drops the old token;
    - a Fifo row moves the old token back with `Front`;
    - the old token is also dropped when the newer token is now held by
      another take;
  - undo: UndoRow tokens and compiled inverse transitions;
  - human tasks as generic primitives: offer, witness,
    fire-if-witness-unchanged, expiry.
- **A take is an exclusivity lock.** A token that a take holds is hidden from
  every other transition. A reader that saw the hole of an open take may run
  its effect, but its commit waits until the take commits or fails. When the
  take fails and restores the token, the reader is stale and binds again.
  Ruled in
  [holon-work#1474](https://github.com/holon-space/holon-work/issues/1474#issuecomment-7212524683).
- **Typed place ids.** A user place, or (transition, role).
- **Decompile.** A first-class map from an L0 marking back to high-level
  state. The viewer, why-not and the differential test use it.
- **ChangeDriven.** Re-check only the pairs whose footprint changed. A pure
  optimisation over L0's level semantics. Its waiter index is a rebuildable
  cache.
- **Driver.** Sans-IO. It turns Running tokens into effect requests. It fires
  the external transitions for results and answers, and `Tick` for time.
- **Also L1:** occurrence ids, conditions, derived places, token class
  semantics, the net file format, the loader, probes.
- The crate doc states that it does not check workflow-net soundness.

### L2 `holon-net`

- `V = holon_pattern::Value`.
- The Loro data-class store.
- The host functions of `Host<Value>`: the value encoder, text, probe
  facts and the answer codec.
- Holon names: `Answer.T`, `AskRow`, the `TypeHint` table, the `AskMeta`
  side table that `Asks::asks` reads.
- The uft lowering.
- The mapping from header fields to holon-expr fields, so guards read
  `tries` (planned, not in the code) and `cause` (planned, not in the code).
- The derived projection and its analyses, unchanged and separate from the
  executor.

## Feature to layer

| Feature today | L0 `petri-net` | L1 `petri-workflow` | L2 `holon-net` |
|---|---|---|---|
| place and subject names, tokens | yes, generic place id | typed place ids | Holon place names |
| row versions | one stamp per row, type owned by the store | stale-record rule by stamp | |
| capacity Single / Register / Fifo, pools | store disciplines, Counted | PoolFull condition | |
| arcs, priority, bind, trigger | yes | | |
| pure body | fire | | |
| effectful body, receiver, Running, attempts, OutcomeUnknown | | compiled to places | |
| facts, failure and timeout ports, retry, run timer, abandon | | compiled | |
| reader and read trace | footprint: rows, absent rows, place scans | field values, witness | |
| re-check through waiters | level semantics, interleaving, priority order | ChangeDriven (cache) | |
| round bound | firing bound, error | NoQuiescence condition | |
| write and retract from outside | external transitions | | |
| `step(now)`, `complete`, `resolve` | external transitions | driver | |
| time, timers, retry wait | none: the Clock place is an ordinary place | Clock place, `Tick`, next-due index, `next_try` guard | |
| manual fire with occurrence id | Manual trigger | occurrence ids | |
| undo | | UndoRow tokens, inverse transitions, undo mark; UndoHeld as why-not | |
| answers, asks, expiry | | offer, witness, fire-if-witness-unchanged, expiry (a second transition with the negated bind) | `Answer.T`, `AskRow` |
| derived places, candidates | | yes | |
| token classes | | semantics | Loro data-class store |
| conditions | | yes | |
| why-not | structural | "in Running of T", UndoHeld | |
| deliberately wrong twins | with each rule | with each rule | with each rule |
| net file, loader, probes | | yes | |
| uft lowering | | | yes |
| conflict and cycle analysis | over declared arcs | | over the projection |
| decompile to high-level state | | yes | |

## Durable state and laws

Durable-state rules:

- All durable state is marking.
- Every change is a firing of a declared transition.
- Everything else is a rebuildable cache: the waiter index, the next-due
  index, live attempts, the decompiled view.

Persistence:

- The compiled place layout is a persisted schema. A change to the compiler
  that changes the layout is a schema change.
- `<T>.Running` takes the class of its input, at most device
  (ADR 0032 REV11-running.a).
- The Clock place and timers are process class and monotonic
  (ADR 0032 §9.1).

Laws, checked as PBT invariants:

- **Level-dead.** At quiescence, no auto pair is enabled when checked again
  from scratch. So every auto transition disables itself by firing, and
  ChangeDriven equals level semantics.
- **Crash.** Drop the driver and every cache, rebuild from the marking: the
  observable run is the same. Today the executor PBTs check it with a shadow
  executor that crashes before every call; a drawn `Crash` action is planned.
  Of the fields of `Executor` (crates/petri-workflow/src/exec.rs), `net` is the
  definition and `tokens` the marking. `crash()` rebuilds the caches
  `watchers`, `preferred`, `waiters`, `pending` and `pending_all`. Every other
  field is state outside the marking that later carve steps compile into
  tokens: `runs`, `marks`, `in_flight`, `parked`, `attempts`, `occurrences`,
  `answers`, `undone`, `deltas`, `conditions`, `requests`, `now` (the Clock
  place), `next_attempt` and `next_minted`.
- **Witness.** The witness is a digest of the read trace. The host gives
  `NetDef` an encoder function from `V` to bytes, so `V` needs no `Eq + Hash`
  bound and no trait (Holon's `Value` holds floats and is only `PartialEq`).
  The law: two values that are `==` encode to the same bytes.

## Migration: an inside-out carve

Not a strangler rewrite. Each step keeps the existing executor PBT green and
adds the two laws as invariants.

1. Make today's executor generic over the token value `V` and move it to
   `petri-workflow`. Done: `crates/petri-workflow` holds the executor, the
   net file format, the loader and probes. The host passes a `Host<V>` of
   plain functions in `NetDef` (the value encoder, text in and out of `V`,
   the probe fact parser, and the answer codec with the keys of an answer
   token), and an ask is declared by its places and transition. `holon-net`
   keeps the faces at `V = Value`: type aliases, the host functions, the ask
   names and `AskMeta` side table, and the `Asks` surface of the UI and MCP.
   Typed bookkeeping tokens follow in this step.
2. Carve the kernel under the executor into `petri-net`: marking, stores,
   arcs, atomic fire, level semantics with interleaving.
3. Compile one feature at a time down to L0: timers, the effect lifecycle,
   retry, asks, undo. Each step is green on the existing executor PBT plus
   the Level-dead and Crash laws.

Removed by the carve: the special cases of today's quiesce loop that
ADR 0032 NET-run.a forbids or NET-level.a replaces.

- Answers are routed specially inside the quiesce loop.
- Pending undos are applied in each round.
- Firing runs in step rounds, with a per-place producer-priority rule.

## Evidence

Measured on today's executor, with its PBT:

- **Level-dead holds today.** A full re-check at every quiescence of the
  executor tests and the drawn-net PBTs finds no enabled auto pair. A planted
  fault in the timer due check makes the check fail, so the check has teeth.
- **Today's executor is already level for every pair that fired.** A firing
  touches `<T>.Running[s]`, which the transition watches, so a fired pair is
  always checked again. An auto transition that does not disable itself does
  not settle and is refused with NoQuiescence. ChangeDriven is only an
  optimisation.
- **The per-place producer-priority rule exists because rounds are step
  semantics.** Without it, only the priority PBT fails: two producers with an
  inhibitor on a common empty output both pass the check in one round and
  both fire. Under interleaving the first firing fills the output and the
  inhibitor blocks the second. So interleaving makes the rule unnecessary,
  and priority only orders firings.
- **The extra L0 firings fit.** Rounds per quiescence stay small (at most a
  few in both the executor PBT and the drawn nets). Three L0 firings per
  high-level firing stay far below the firing bound.

## Open questions

### A persisted `next_try` against a fresh process clock

The Clock place is process class, so it restarts with the process. A
`next_try` sits in the header of a token whose class is its input's class,
so it can persist. Open: how a persisted `next_try` is read against a fresh
process clock.

- (a) `next_try` holds wall-clock time, and `Tick` puts wall-clock time
  beside the monotonic time. Simple; a forward jump fires retries early, and
  the `Tick` guard refuses a backward jump, so a backward jump stops time until
  the wall clock catches up or the process restarts.
- (b) `next_try` holds (process epoch, monotonic time). A `next_try` from an
  older epoch counts as due, or is re-armed by one declared rule.
- (c) `next_try` holds the remaining wait, and the first take after a restart
  counts it from the fresh clock.

Recommendation: (b). Time stays monotonic inside an epoch, and the boundary
has one explicit rule.

### `tries` per token against attempts per run

Today `Run.attempts` per (transition, subject) is the only count; the count
bound reads it. The header holds no `tries` yet: a second counter that must
equal the first broke on every way a token is replaced or consumed while its
run is open. The retry step picks the unit of a retry.

Example: `T0` (Keyed) takes `x` from `P0[s]` and fails retryably; its run
stays open with 1 attempt and a backoff. `T1` consumes `x`. After the
backoff an outside write puts `y` with the same content into `P0[s]`. The
failure mark sees the same input, so the run of (`T0`, s) is still open.
`T0` takes `y`: attempt 2 of the run, but `y` has never been taken.

- (A) The token is the unit. A run ends when the token it counts on leaves
  the net; `y` is new input with 0 tries and fires at once, without the
  backoff. The bound reads the header.
- (B) The run is the unit. A token that joins an open run gets
  `tries = attempts - 1` at the take; the header copies the run, and the
  bound may read either.

Recommendation: open, for the retry step.

### Pool tokens

A pool token serves the runs of every subject. Open for the retry step:
does a failed run over a pool token retry, and with (A), does the pool
token count the tries of one subject's run?

### `notify(rows)` for Loro-backed stores

A Loro merge from another device changes rows outside any local firing.

- (a) The store reports the changed rows, and the driver fires an external
  merge transition for them. The durable-state rule holds.
- (b) The report only invalidates the ChangeDriven cache. The merge is not a
  firing, an exception to the rule.

Recommendation: (a). It keeps "every change is a firing" without an
exception.

### Record: one transition or three

- (a) Three L0 transitions with mutually exclusive binds: fresh, candidate,
  withheld.
- (b) One transition whose fire branches.

(a) gives a more precise structural why-not; (b) gives a smaller net.

### Outbox place

- (a) No outbox. The driver derives effect requests from Running tokens.
- (b) An outbox place that holds each effect request as a token.

Recommendation: (a), until a feature needs requests that no Running token
carries.

### holon-engine convergence

`holon-engine` (YAML nets, Rhai guards, WSJF ranking, what-if) could run on
L0: clone the marking and fire.

Recommendation: out of scope now.

### Projection analysis in L0

The conflict and cycle analysis of the derived projection lives in
`holon-net`; L0 has its own structural analysis over declared arcs.

- (a) Later, compile the projection to an L0 net and use L0's analysis.
- (b) Keep the two analyses separate.

Recommendation: decide after migration step 3, when L0's analysis exists.
