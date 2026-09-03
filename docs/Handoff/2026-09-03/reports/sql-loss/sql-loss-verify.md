# Verify `sql-loss` — REFUTED (partial: one half is sound, the other has a reproducible false-positive)

Fresh-context verifier. Tree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/sql-loss`
(`pwd` printed in every round). `still_under` and `assert_updated_rows_exist` confirmed
present on disk at `crates/holon/src/core/sql_operation_provider.rs:374` and `:1781`.
No jj/git write commands were run. Scratch test written, run, and deleted
(`git status --porcelain crates/holon/tests/` clean).

## Verdict: REFUTED

Half (a), `StagedParents::still_under`, holds under my own probes.
Half (b), `assert_updated_rows_exist`, **reports a correct batch as data loss**.

## The counterexample (reproduced by me, `lane-logs/verify-probe-3.log`)

Production write path, real `block_raw` schema (`CoreSchemaModule` + `LinkSchemaModule`),
`SqlOperationProvider::execute_batch_with_origin`.

State: `block:pp` → `block:c1` → `block:c2`.
Batch, in order: `update block:c2 {content:"C2 edited"}`, then `delete block:c1`.

The update names no `parent_id`, so — correctly, per the lane's own unit test
`an_update_without_a_parent_stages_nothing` — `still_under` keeps `block:c2` in the
cascade. The transaction commits and the subtree is deleted, which is exactly what the
batch asked for.

Expected: `Ok`.
Actual:

```
PROBEX err=Some("batch UPDATE hit 1 row(s) that do not exist in `block_raw`: [\"block:c2\"]
  — the sink lost rows the caller still believes in, and an UPDATE to a missing row is a
  silent no-op that can never restore them")
PROBEX rows_after=["block:pp", "sentinel:no_parent"]
```

`rows_after` proves the sink lost nothing. The assertion's own error text is false.

### Defect

`crates/holon/src/core/sql_operation_provider.rs:4655-4662` — the exclusion set is
`created_ids` plus only the ids the batch **explicitly** names in a `delete` op:

```rust
"delete" => updated_ids.retain(|u| u.as_str() != id),
```

It never subtracts the **cascade descendants** that `prepare_purge` computed and had in
hand (`all_ids`). Any batch that updates a block and deletes one of its ancestors is
therefore reported as a failure after a correct commit.

### Consequence

Not silent. `execute_batch_with_origin` returns `Err` post-commit, so every caller of the
batch seam (Loro→SQL projection, bulk org ingest) sees a delete-plus-edit batch as failed.
For the projection that is a spurious full reseed on a healthy sink; for other callers it
is a false "the sink lost rows" error on work that succeeded. It inverts the lane's own
error-handling goal: the loud signal now fires on the correct case.

## What I could NOT refute

- **Probe 4 — the subtract-only claim: PASSES.** Batch `update block:c3 {parent_id:block:c1}`
  + `delete block:c1` (staging a child INTO the deleted subtree) fails LOUD at COMMIT and
  rolls back whole — `before == after`, all three rows intact. The deferred self-FK
  (`crates/holon-turso/sql/schema/blocks.sql:33-35`, `DEFERRABLE INITIALLY DEFERRED`) does
  what the design comment claims. No orphan, no silent drop.
- **Probe 5 — cost.** A 1000-update batch commits and passes the assertion; the check is
  one indexed `IN` query, no per-id fan-out, no expression-depth failure. PASS.
- **Check 6 — cross-batch.** `prepare_purge`'s cascade still reads the DB in PREPARE, but
  `db_handle.transaction(all_sql)` commits before `execute_batch_with_origin` returns, so
  no earlier batch can be uncommitted while a later one prepares. The single-batch fold is
  the complete case; the fix does generalise.

## Not independently reproduced (harness, not code)

The gate/pin re-run could not be completed here. Two submissions
(`lane-logs/verify-gate-1.log`, `verify-gate-2.log`) both ended 0-byte: the first was
killed by the 12:25 outage, the second by the harness 600 s foreground deadline, each
leaving orphan `rustc`. I did not resubmit into them and did not `pkill`. So the lane's
997/997, `loro-suite` 13/13, keystone, and the 22/24 `two_instance` runs are **unverified
by me** — the refutation above does not depend on them.

## Recommended routing (I did not fix anything)

`prepare_purge` already returns the cascaded ids inside its `row_statements`; the batch
loop needs those ids subtracted from `updated_ids` alongside the explicitly deleted one.
Until then the un-ignored pin can be green while the assertion mis-fires on a shape the
pin does not generate.

---

# Rev 2 — CONFIRMED

Re-verify of the lane's response to the rev-1 refutation above. Tree: same workspace,
`sql_operation_provider.rs` restored and hash-verified after mutation (`shasum -a 256 -c`
against a pre-mutation backup — never `jj restore`). No jj/git write commands run.

## Verdict: CONFIRMED

Rev 2 is sound: it keeps the safety net at full strength (both causes still `Err`, both
still drive the reseed) while making the diagnosis honest. Rev 1's message ("the sink lost
rows") was false in the shape my original counterexample constructed; rev 2's is not.

## (1) The lane's measurement — reproduced, and it is real

I re-applied the prescribed mutation myself: `assert_updated_rows_exist` filters
`cascade_removed` out of `ids` before the existence check, dropping the split diagnosis
entirely (byte-identical in effect to "subtract `prepare_purge`'s cascade ids from
`updated_ids`"). Backed up the pre-mutation file first (`/tmp/vr2-orig-backup.rs`,
`shasum -a 256` recorded), restored via `cp` + hash-check after, never `jj restore`.

Ran the pin 5 times total under the mutation: 3 **RED** with the exact signature the lane
reported, 2 PASS.

| run | result | log |
|---|---|---|
| solo run 1 | PASS | `lane-logs/verify-r2-mutation-pin-2.log` |
| 3x batch, run 1 | **RED** `receiver block:c2: held in Loro, ABSENT from block_raw` | `lane-logs/verify-r2-mutation-pin-3x.log` L167-206 |
| 3x batch, run 2 | PASS | `lane-logs/verify-r2-mutation-pin-3x.log` L369-371 |
| 3x batch, run 3 | **RED** same signature | `lane-logs/verify-r2-mutation-pin-3x.log` L533-571 |

The lane's own `lane-logs/rev2-pin-144143.log` (read, not reproduced by me — pre-existing
on disk) shows the same signature on the `owner` side; mine landed twice on the `receiver`
side. Same divergence class either way — `SqlOperationProvider::TwoInstanceHandle`'s
`sql_projection_lag` oracle reports it for whichever peer's Loro→SQL pass hits the shape.

**This is flaky, not spurious.** `LoroSyncController::run_loop` is wake-driven with bounded
re-drive (see `cross_peer_indent_then_join_stalls_the_receiver_projection`'s doc comment);
which pass happens to observe the ambiguous shape, and whether an unrelated later pass
happens to re-walk the block anyway, is a real race against wall-clock scheduling. A ~40%
reproduction rate across 5 runs is exactly what a race looks like — it does not weaken the
lane's claim, it explains why the rev-1 refutation's single clean counterexample run never
hit it. **The measurement is right: excluding cascade ids from the assertion reopens real
data loss.**

## (2) Was the original counterexample "correct"? — Premise partially wrong, said plainly

No, not in the unqualified sense my rev-1 verdict claimed. My batch's **end state** was
correct (`rows_after` proved nothing was lost in that one deterministic construction), but
its **SQL shape** — an UPDATE naming no `parent_id` landing in the same transaction as a
delete of an ancestor it does not name — is **provably indistinguishable, at the point
`assert_updated_rows_exist` runs, from the real-loss shape this rev-2 measurement produced**.
Both present as: a row this batch UPDATEs is gone after commit, and nothing in the batch's
own data says whether that's an intended sweep or a genuine desync.

Given that ambiguity, and this codebase's own stated priority order (`CLAUDE.md`
Error-Handling Philosophy: *fails with a clear error message* outranks *silently degrades to
look "fine"*), erroring on the ambiguous shape is the engineering-correct call, not a false
positive to be optimized away. My rev-1 verdict was right that rev-1's *message* was false in
my specific run, and right that the routing needed a name for the two cases — it was wrong to
call the underlying `Err` itself spurious. Rev 2 fixes exactly the part that was actually
wrong (the text), not the part I mischaracterized as wrong (the control flow).

## (3) The split diagnosis — distinguishes, and does not weaken the net

Code (`crates/holon/src/core/sql_operation_provider.rs:1796-1839`): `missing` is partitioned
by `cascade_removed.contains`, and **both** branches return `Err` — confirmed by reading the
function (no `Ok` path for either partition) and by the two live pin-scenario logs above and
the shipped-code gate run below, where the pin's oracle output is bit-for-bit what a caller
would act on:
- `"batch UPDATEd N row(s) its own delete cascade removed …"` — self-consistency violation,
  named, and traceable to the batch's own delete;
- `"batch UPDATE hit N row(s) that do not exist … the sink lost rows"` — kept only for a row
  this batch's own deletes cannot account for.

`LoroSyncController::emit_ops` propagates the batch's `Result` as an untyped `String` error
(`applied?` at `loro_sync_controller.rs:1330`) — the reseed-on-failure path downstream does
not branch on message content, so the diagnosis split is purely for a human reading the log;
it changes no control flow and cannot reduce recall. Confirmed by reading `emit_ops` and by
`batch_delete_cascade_updates.rs`'s own two `.expect_err(...)` cases both asserting an `Err`
was returned, differing only in which substring appears.

## (4) The upstream gap — located, confirmed absent

`crates/holon-loro/src/loro_sync_controller.rs:2184-2196`, `fn block_diff_params`:

```rust
if old.parent_id != new.parent_id {
    params.insert("parent_id".into(), Value::String(new.parent_id.to_string()));
}
```

`parent_id` is emitted **only** when the diff snapshot's `old`/`new` disagree. When a pass's
`old` baseline already agrees with `new` on the parent (because an earlier pass never
persisted the true placement to the sink, or the sink's row was cascaded away by an unrelated
op the diff snapshot has no visibility into), the update carries no `parent_id` at all —
exactly the shape both my constructed counterexample and the rev-2 pin regression hit. This
is the root cause the lane flags as open and NOT fixed here: grounding this UPDATE's
`parent_id` the way a CREATE's is already grounded (per the doc comment on the sibling pin at
`two_instance_composed_pbt.rs:918-919`) would make the reseed this assertion forces
unnecessary for this shape, not just recoverable from it.

## Gates run on the shipped (restored, hash-verified) code

| gate | result | log |
|---|---|---|
| `-p holon --test batch_delete_cascade_updates` | 5/5 pass | `lane-logs/verify-r2-final.log` L73-79 |
| `-p holon --lib staged_parents_tests` | 4/4 pass | `lane-logs/verify-r2-final.log` L145-150 |
| pin, 3x | PASS 12.8s / 12.9s / 13.1s | `lane-logs/verify-r2-final.log` L318-320, L482-484, L646-648 |

## Not independently re-verified this round

`-p holon -p holon-app` full suite, `loro-suite`, `keystone-smoke`, `fmt`, `bugfunnel` — the
rev-1 report's coverage of these stands; this round targeted only the rev-2 diff and its
disputed claim.
