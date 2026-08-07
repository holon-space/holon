# Turso Bug Fix: CDC change callbacks are UNLABELED and PRE-COMMIT

Handoff for a session working in `~/Workspaces/bigdata/turso/` (the
nightscape/turso fork — it is ours, we extend it deliberately).
Current pin used by the downstream app: `447f0faeb87c96a92dc62bfeb1f6bbb2bfddce3d`.

This document is self-contained. You do not need the downstream application's
context to execute it.

---

## 1. Problem statement

Turso's materialized-view change callbacks
(`Connection::set_change_callback`, `RelationChangeEvent`) are the change-data-
capture (CDC) wire for a downstream reactive application. Two properties of
that wire are wrong.

### 1.1 The wire carries no transaction boundary

Emission site: `core/vdbe/mod.rs`, `Program::apply_view_deltas`, the
`ViewDeltaCommitState::Processing` arm (the `if *current_index >= views.len()`
block, ~lines 2207-2353 at the current pin). That block already holds **all of
the commit's affected views in one `views: Vec<String>`** — computed by the
BFS in the `NotStarted` arm (~2110-2205). It then loops over that vec and
fires **one `RelationChangeEvent` per view**, each with:

```rust
pub struct RelationChangeEvent {   // core/types.rs:3663
    pub relation_name: String,
    pub columns: Vec<String>,
    pub changes: Vec<DatabaseChange>,
}
```

There is **no commit identifier, no group size, no ordinal**. A consumer that
receives events for `v_a` and `v_b` cannot tell whether they came from one
commit or from two unrelated ones. The information exists at the emission
site and is thrown away one line later.

The base-table CDC path has the same gap: `op_notify_cdc_change`
(`core/vdbe/execute.rs:18953`, roughly :19054 for the event construction)
builds a `RelationChangeEvent` per **row** with no commit identity either.

**Measured downstream consequence.** The consumer must group deliveries into
"one user operation's worth of change" before recomputing derived state. With
no label it can only group by *arrival timing* — drain the delivery channel
until it returns `Pending` and call that a burst. Whether two events emitted
microseconds apart land in one burst or two is then a race between the
consumer's wake-up and the runtime's delivery. The downstream measurement of
that race is a **uniform residual of 2 redundant recomputations per document
per window** on structural edits — i.e. the burst boundary was placed wrong
roughly every time, deterministically, not as a tail.

The problem is compounded on the writer side: one user-level operation reaches
the SQL layer as **several separate transactions**. In the downstream app,
`crates/holon/src/core/sql_operation_provider.rs` runs the row create in its
own transaction (~:2752, "Run the create atomically in one transaction") and
then follow-up field writes in further `db_handle.transaction(..)` calls
(~:2580, ~:2595), while the batch path uses a single transaction (~:3758,
"Phase 2: Execute all SQL in a single transaction"). So one edit is N commits
× M views = N·M unlabeled events. Merging those transactions is the
downstream app's own follow-up work; **it is out of scope for this Turso
change**, but it is why the label must identify a *commit*, and why the
downstream side will additionally want to carry its own operation id — see
§2.4.

### 1.2 Emission happens before the commit

`apply_view_deltas` is called from `commit_txn` (`core/vdbe/mod.rs:2447`)
*near the top*, well before `commit_txn_wal` (:2572) / `commit_txn_mvcc`
(:2659) actually commit. The event type documents the hazard against itself
(`core/types.rs:3648-3660`):

> **IMPORTANT**: Callbacks fire BEFORE the transaction commits. This means:
> - Changes may still be rolled back if a later error occurs
> - The data in `changes` represents pending changes, not committed state

and the emission site repeats it inline:

> // Notify BEFORE clearing - callbacks fire BEFORE commit completes.
> // WARNING: Changes may still be rolled back if a later error occurs.

Downstream consumers treat every delivered batch as fact and have no
retraction mechanism. A commit that fails *after* dispatch therefore delivers
**phantom deltas** for writes that never happened, and nothing discloses it.
This is tracked downstream as an open hazard (board item #16); closing it is
an explicit acceptance criterion of this work.

**Honest scoping note — read this before you start.** The reproducer
(§3) shows the durability gap directly: the callback fires while the commit's
frames are not yet in the WAL. But the two *obvious* user-level rollback
paths — `BEGIN; INSERT; ROLLBACK` and a commit-time deferred-FK failure —
**do not** currently produce phantom events, because both short-circuit
before reaching the emission site (rollback clears
`view_transaction_states` in the `NotStarted` arm at ~:2116; the deferred-FK
check halts the COMMIT program before `commit_txn` runs `apply_view_deltas`).
Those two cases are therefore included as **passing characterization tests**
that must stay passing. The live window is between `apply_view_deltas` and
the WAL/MVCC commit — an I/O, checkpoint, or busy failure there. Do not
"fix" this by arguing the window is unreachable; the ratified requirement is
that emission moves to the post-commit side so the window is
*unrepresentable*, and the WAL-durability test is the mechanical proof.

---

## 2. Specification (ratified 2026-08-07)

Three changes, all inside the CDC emission path.

### 2.1 Envelope: stamp commit identity on every event

Add commit identity to `RelationChangeEvent`:

```rust
pub struct RelationChangeEvent {
    pub relation_name: String,
    pub columns: Vec<String>,
    pub changes: Vec<DatabaseChange>,
    /// Identity of the commit this event belongs to. All events emitted for
    /// one commit share this value. Never 0 for a delivered event.
    pub commit_id: u64,
    /// This event's position in its commit's fan-out, and the fan-out size.
    /// `commit_index < commit_len` always; the consumer knows the burst is
    /// complete when it has seen `commit_len` events for `commit_id`.
    pub commit_index: u32,
    pub commit_len: u32,
}

impl RelationChangeEvent {
    /// REQUIRED by the reproducer: an *inherent* accessor with exactly this
    /// name and signature. See §3.1 for why a public field alone is not enough.
    pub fn commit_id(&self) -> u64 { self.commit_id }
}
```

Requirements:

- `commit_id` is **process-monotonic and non-zero**. A simple
  `static AtomicU64` incremented once per emitting commit is sufficient and
  preferred over a wall clock. Start the counter at 1 so `0` is reserved for
  "unset/unlabeled" — the reproducer's shim depends on `0` meaning unlabeled.
- **Every** event of one commit's fan-out carries the same `commit_id` —
  including the base-table events from `op_notify_cdc_change`, if that path
  can be reached in the same commit as view deltas. If wiring the base-table
  row path to the same counter turns out to require threading state through
  the VDBE in a way that is not localized, stop and report it rather than
  inventing a second numbering space: an inconsistent id is worse than none.
- `commit_len` counts the events actually **delivered** for that commit. Note
  that the current code `continue`s past views whose output delta is empty
  (the "Skip the callback when the matview's output didn't actually change"
  branch), so `commit_len` must be computed **after** that filtering, not from
  `views.len()`. Getting this wrong makes a consumer wait forever for a burst
  member that never arrives.

**Alternative considered and rejected:** emitting one composite event carrying
all views' deltas. Rejected because it breaks every existing consumer's
`relation_name`-per-event shape and its filter semantics
(`RelationFilter`, `core/types.rs:3677`) for no additional grouping power.

### 2.2 Emission moves to post-commit

Callbacks must fire **after** the transaction is committed and durable, not
from inside `commit_txn`'s pre-commit phase.

- Compute the events (schemas, deltas, `commit_id`, `commit_len`) where they
  are computed today — the data lives in `view_transaction_states` and must be
  read before it is cleared.
- **Stage** them, and dispatch to the registered callbacks only on the success
  path after `commit_txn_wal` / `commit_txn_mvcc` has completed.
- Keep the existing `catch_unwind` panic protection and the
  "clone callbacks out of the lock before invoking" discipline; a panicking
  consumer must not unwind through the VDBE, and the callback registry lock
  must not be held across a callback.

### 2.3 Rollback: NO delivery (ratified)

A commit that does not complete delivers **nothing**. There is no retraction
event and no "tentative" flag.

Rationale, and it is a ruling, not a preference: the downstream consumers have
no retraction machinery, so a retraction event would have to be *implemented*
by every consumer before it could be *correct*, whereas "no delivery" is
correct for every existing consumer with zero consumer-side change. Staged
events for a failed commit are dropped, and the staging area is cleared on
every exit path (success, error, rollback) exactly as
`view_transaction_states` is cleared today.

### 2.4 Backward compatibility

- The event's existing fields keep their meaning and their per-relation
  fan-out shape: one event per changed relation, same `relation_name`, same
  `columns`, same `changes`. The pinned test
  `per_relation_fan_out_and_payload_are_unchanged` enforces this.
- `RelationFilter` semantics are unchanged: filtering is still per relation
  name, applied per event.
- The struct gains public fields. `RelationChangeEvent` is **constructed only
  inside `core`** (two sites: `core/vdbe/mod.rs` and
  `core/vdbe/execute.rs`); consumers only read it, so adding fields is not a
  breaking change for consumers. Do not make the struct `#[non_exhaustive]`
  in this change — it is unnecessary and would churn downstream matching.
- **Timing change is the observable break.** Consumers that (incorrectly)
  relied on seeing a delta *before* `execute(..)` returned will now see it
  after the commit. This is the intended semantic change; the downstream app's
  own "watermark" helper samples the emission counter *after* the write
  returns, so it is compatible either way.
- The doc comments on `RelationChangeEvent` (`core/types.rs:3648-3660`) that
  advertise pre-commit timing must be rewritten to state the new guarantee.
  A stale comment here is worse than none — it is the sentence downstream
  consumers quote.

### 2.5 Downstream (informational — do NOT implement here)

For context only, so you understand what the envelope is for. The downstream
app's tap is `crates/holon-turso/src/turso.rs:1568-1592` — a single
`set_change_callback` that converts each event into a batch, stamps a
process-monotonic `seq` onto batch metadata, and broadcasts it. From there:
`broadcast` → a spawned bridge actor → an mpsc `ReceiverStream`
(`turso.rs:928-948`) → the derived-state layer. The consumer will copy
`commit_id`/`commit_index`/`commit_len` into its batch metadata
(`crates/holon-api/src/streaming.rs:401`, `BatchMetadata`, which today has
only `seq`) and coalesce batches sharing a `commit_id` into exactly one
recomputation. The multi-transaction-per-operation half (§1.1) is fixed
downstream, not here.

### 2.6 Worker mirror (answer the question explicitly)

The downstream repo pins the same fork rev **twice**: the workspace
(`Cargo.toml:183-186`, `turso` / `turso_core` / `turso_sdk_kit` / `turso_ext`)
and a separate WASM worker crate
(`frontends/holon-worker/Cargo.toml:42`, `turso_core` only). Both must be
re-pinned to the new rev in lockstep.

**Does this change require worker code changes? No.** The worker uses
`turso_core` but never names `RelationChangeEvent` and never registers a
change callback, so the additive fields and the timing change do not reach it.
The only worker obligation is the lockstep rev bump. Keep the change
`wasm32`-clean: no new threads, no wall-clock dependency, no `std::time`
in the emission path. An `AtomicU64` counter satisfies this.

---

## 3. Reproducer

**Path:** `~/Workspaces/bigdata/turso/bindings/rust/tests/cdc_commit_envelope_repro.rs`
(present in your working copy, **uncommitted** — commit it as part of the fix).

**Run:**
```bash
cd ~/Workspaces/bigdata/turso
cargo test -p turso --test cdc_commit_envelope_repro 2>&1 | tee /tmp/cdc-repro.log
```

Five tests: **2 must go from RED to GREEN**, **3 must stay GREEN**.

### 3.1 How the labeling test reds today without a compile error

The test file defines a fallback:

```rust
trait UnlabeledCommitEnvelope {
    fn commit_id(&self) -> u64 { 0 }
}
impl UnlabeledCommitEnvelope for RelationChangeEvent {}
```

Rust resolves an **inherent** method before a trait method at the same
autoderef step. So today `event.commit_id()` hits the trait fallback and
returns `0`; as soon as you add the inherent
`impl RelationChangeEvent { pub fn commit_id(&self) -> u64 }` from §2.1, the
same source line reads the real value — **with no edit to the test file**.
This is why a public `commit_id` *field* alone does not flip the test: field
access with call syntax does not resolve to a non-callable field. Add the
inherent accessor.

### 3.2 Verified RED output today (pin 447f0fae)

```
running 5 tests
test a_commit_that_fails_at_commit_time_delivers_nothing ... ok
test an_explicitly_rolled_back_transaction_delivers_nothing ... ok
test per_relation_fan_out_and_payload_are_unchanged ... ok
test the_callback_runs_after_the_commit_reaches_the_wal ... FAILED
test two_views_of_one_commit_share_a_commit_id ... FAILED

---- the_callback_runs_after_the_commit_reaches_the_wal stdout ----
assertion `left == right` failed: the CDC callback fired while the commit's
frames were NOT yet in the WAL (wal at callback = 24752 bytes, wal after the
write returned = 32992 bytes) — the consumer was told about data that is not
durable and can still be rolled back
  left: 24752
 right: 32992

---- two_views_of_one_commit_share_a_commit_id stdout ----
every CDC event must carry a non-zero commit identity so a consumer can group
one commit's fan-out deterministically; got [0, 0] for ["v_upper", "v_all"]

test result: FAILED. 3 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
```

### 3.3 What each test pins

| Test | Today | After the fix |
|---|---|---|
| `two_views_of_one_commit_share_a_commit_id` | RED — `[0, 0]` | GREEN — both non-zero and equal |
| `the_callback_runs_after_the_commit_reaches_the_wal` | RED — 24752 vs 32992 bytes | GREEN — equal; emission is post-commit |
| `an_explicitly_rolled_back_transaction_delivers_nothing` | GREEN (characterization) | must STAY green |
| `a_commit_that_fails_at_commit_time_delivers_nothing` | GREEN (characterization) | must STAY green |
| `per_relation_fan_out_and_payload_are_unchanged` | GREEN (compat pin) | must STAY green |

### 3.4 One test to ADD as part of the fix

The reproducer cannot express `commit_len` today (there is no field to read).
Add, alongside the fix:

`a_commits_fan_out_is_self_delimiting` — one commit touching three views
where **one view's output delta is empty** (e.g. a filtered view the row does
not match). Assert every delivered event reports the same `commit_len`, that
`commit_len` equals the number of events actually delivered (**2**, not 3),
and that the `commit_index` values are exactly `0..commit_len`. This is the
trap named in §2.1 and it needs its own guard.

---

## 4. Acceptance criteria

Mechanical. Every line must be run, teed to a log, and the log quoted in the
final report.

**In the Turso repo (`~/Workspaces/bigdata/turso`):**

1. `cargo test -p turso --test cdc_commit_envelope_repro` — **5 passed,
   0 failed**, with the two previously-red tests named as passing and the
   three characterization tests still passing.
2. The new `a_commits_fan_out_is_self_delimiting` test (§3.4) passes.
3. A rolled-back or failed commit delivers **zero** events — covered by tests
   b1/b2 plus, if you can reach the post-`apply_view_deltas` failure window
   with the repo's existing fault-injection machinery, a test that exercises
   it directly. If you cannot reach it, say so plainly in the report; do not
   quietly drop the criterion.
4. `cargo test -p turso` — full bindings suite green, in particular the
   existing `test_view_change_callback_*` family in
   `bindings/rust/tests/integration_tests.rs` (~:1299-1530).
5. `cargo test -p turso_core` green, no new failures against the pre-change
   baseline. Capture the baseline BEFORE you change anything; the
   pass/fail set must be identical modulo the intended additions.
6. `cargo clippy --workspace --all-targets` and `cargo fmt --check` clean for
   every file you touched.

**Downstream validation loop (run after the fix lands and the fork is
re-pinned) — `~/Workspaces/pkm/holon`:**

7. Re-pin all four fork deps in `Cargo.toml:183-186` **and**
   `frontends/holon-worker/Cargo.toml:42` to the new rev. Edit the `rev =`
   strings by hand. **Never run a bare `cargo update`** (see §5).
8. `cargo check --workspace --all-targets --features pbt` — exit 0, no new
   warnings.
9. `cargo test -p holon-turso 2>&1 | tee /tmp/holon-turso.log` — the CDC tap
   and watermark tests are the first thing the timing change can break.
10. The composed keystone end-to-end PBT:
    `cargo test -p holon-integration-tests --test general_e2e_composed_pbt
    --features holon-integration-tests/pbt,holon-gpui/pbt 2>&1 | tee
    /tmp/keystone.log`. Its known-red signatures are catalogued in
    `docs/Testing/KeystoneKnownReds.md`; the failure set must be **identical**
    to the pre-re-pin baseline. Capture that baseline first.
11. `just hand-authored` — the deterministic replay corpus. Watch the
    read/write budget numbers: the point of this change is that they go DOWN
    once the consumer-side grouping lands. They must not go UP from the
    re-pin alone.

---

## 5. What NOT to do

- **No bare `cargo update`.** It has broken this workspace's build before via
  transitive ed25519/lockfile churn. Change only the `rev =` strings, then let
  cargo resolve those packages alone.
- **No changes outside the CDC emission path.** The scope is:
  `core/types.rs` (`RelationChangeEvent` + its doc comment),
  `core/vdbe/mod.rs` (`apply_view_deltas` / `commit_txn` staging + dispatch),
  `core/vdbe/execute.rs` (`op_notify_cdc_change`, only to carry the same
  `commit_id`), and the tests. Anything else needs a reason in the report.
- **The IVM / materialized-view maintenance machinery is out of scope.** Do
  not touch delta computation, the view dependency BFS, `DbspCircuit`,
  view state persistence, or matview DDL. You are changing *when* and *with
  what label* an already-computed delta is announced — nothing about *what*
  the delta is.
- **Do not add a retraction event** or a "tentative/committed" flag. §2.3 is
  ratified: rolled-back commits deliver nothing.
- **Do not collapse the per-view fan-out** into one composite event (§2.1).
- **Do not swallow errors.** If the staging/dispatch move hits a case you
  cannot make correct — most likely the base-table `op_notify_cdc_change`
  path sharing the commit counter — report it loudly and stop. A silently
  inconsistent `commit_id` is worse than the current honest absence of one.
- **Do not `jj`/`git` anything in the downstream repo.** The re-pin is the
  downstream session's job.

---

## 6. Where things are

| What | Where |
|---|---|
| Fork | `~/Workspaces/bigdata/turso/` |
| Current pin | `447f0faeb87c96a92dc62bfeb1f6bbb2bfddce3d` |
| Event type | `core/types.rs:3648-3680` |
| Matview emission site | `core/vdbe/mod.rs`, `apply_view_deltas`, ~:2099-2445 |
| Commit driver | `core/vdbe/mod.rs`, `commit_txn` :2447; `commit_txn_wal` :2572; `commit_txn_mvcc` :2659 |
| Base-table CDC emission | `core/vdbe/execute.rs`, `op_notify_cdc_change` :18953 (event at ~:19054) |
| Callback registry | `core/database.rs:567`; registration `core/connection.rs:5258-5305` |
| Rust binding re-export | `bindings/rust/src/lib.rs:63`; `bindings/rust/src/connection.rs:264-300` |
| Existing callback tests | `bindings/rust/tests/integration_tests.rs` ~:1299-1530 |
| Reproducer (uncommitted) | `bindings/rust/tests/cdc_commit_envelope_repro.rs` |
| Downstream tap | `~/Workspaces/pkm/holon/crates/holon-turso/src/turso.rs:1568-1592` |
| Downstream delivery tail | `.../crates/holon-turso/src/turso.rs:903-949` |
| Downstream batch metadata | `.../crates/holon-api/src/streaming.rs:401` |
