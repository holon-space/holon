---
id: 2026-09-17-dense-patch-apply-is-not-atomic
date: 2026-09-17
gap: ORACLE
secondary: ENVIRONMENT
status: MITIGATED
summary: >-
  Reported as "dense_patch applied its batch and then returned an error, twice";
  the two gates that errored both return BEFORE any dispatch, so those errors
  applied nothing — but the handler holds no transaction, so a mid-loop failure
  really can leave a batch half-applied and a retry really can duplicate rows.
  A mid-loop failure is now a LOUD partial-apply report naming every row that
  landed. No TRANSACTIONAL seam exists on either leg; compensating rollback via
  the pinned Loro's unused `revert_to` is measured viable but only behind a
  guard, because it destroys a peer's concurrent write.
---

## Bug

Reported by the orchestrator at ~00:50 on 2026-09-17 against the running app
and the live vault, on Now.org block
`block:c0450284-7413-44e6-a5dd-4680d09ad9f8`. Two consecutive `dense_patch`
calls returned errors while the caller observed the batch's rows present in the
store:

1. `unknown projection handle proj:0cf2dc8b-… — it was never issued or has been
   evicted; re-run dense_query`
2. on the retry, `conflict: 3 block(s) changed since dense_query — patch
   rejected`

The natural reading is "applied, then reported failure", whose consequence is
stated plainly by the reporter: **a caller who trusts the error re-applies and
duplicates rows.** That consequence is why this is recorded even though the
reading does not survive inspection.

## Root cause

What the source says, verified rather than assumed.

The running handler is `frontends/mcp/src/tools.rs` (`dense_patch`), and
`md5` is `743c933e3db70df4d6be51d622fead11` in BOTH the primary repo and this
workspace, so the running build's handler is this code. Its order is:

| line | step |
|---|---|
| `tools.rs:3149` | handle lookup → `dense_projection.rs:148` |
| `tools.rs:3152-3161` | parse the dense text, parse delete aliases |
| `tools.rs:3163-3218` | conflict re-read of `updated_at`, `return Err` at `:3211` |
| `tools.rs:3220-3240` | `dry_run` early return |
| `tools.rs:3246` | **apply loop begins** |

Both gates `return Err` before `:3246`. Neither of the two reported errors can
therefore follow a dispatch: **for these two errors the "applied then failed"
reading is refuted, and the rows the caller attributed to those batches were not
written by them.**

Which branch fired is pinned by the quoted text. `dense_projection.rs:148-165`
has two DISTINCT messages:

```
Some(p) if p.created.elapsed() < self.ttl => Ok(p.clone()),
Some(_) => bail!("projection handle {handle} has expired (TTL {}s) — re-run dense_query …"),
None    => bail!("unknown projection handle {handle} — it was never issued or has been evicted; …"),
```

The report quotes the `None` arm, so the handle was **absent from the registry**,
not present-and-expired. Two ways to be absent, both real:

- the 30-minute TTL sweep in `insert` removed it — `dense_projection.rs:138-146`
  does `map.retain(|_, p| p.created.elapsed() < self.ttl)` before inserting a
  NEW projection, so the next `dense_query` silently sweeps an expired handle
  and subsequent lookups take the `None` arm rather than the expired one;
- the registry is per-server-instance (`server.rs:481`,
  `ProjectionRegistry::new(Duration::from_secs(60 * 30))`), so a handle issued by
  a different MCP instance is unknown by construction, not by eviction.

### The defects that are real, and why this is filed rather than dismissed

1. **No atomic apply.** The handler holds no transaction — a grep for
   `transaction`/`rollback`/`begin()`/`commit()` over `tools.rs:3140-3560`
   returns 0 — and dispatches each op individually (create at `:3285`, the
   `UpdateTitle` arm at `:3293`, and so on for update/move/delete). Any failure mid-loop returns `Err` with the earlier ops
   already committed. So a batch really can be partially applied and reported as
   failed; the mechanism is a mid-loop failure, not the two gates the report
   named. That is the class to hunt.
2. **The snapshot is invalidated by the tool's own writes.** The conflict gate
   re-reads `updated_at` from the `block` matview (`:3176`) and compares it with
   the snapshot `dense_query` took. A successful patch writes the store, the org
   write-back renders the file, and the file watcher re-ingests, bumping
   `updated_at` on exactly the blocks the patch touched. So a retry with the
   pre-patch projection legitimately conflicts — which is what error 2 looks
   like. Composed with (1), this is the duplicate-rows hazard: the error says
   "re-run dense_query and retry", and a caller who does so without knowing the
   first patch landed re-applies it.
3. **The TTL is undisclosed and the two failures are indistinguishable in
   effect.** The handle lives 30 minutes; the tool description states no TTL;
   and for the caller's only real question — did anything land? — "never issued"
   and "expired" mean the same thing (nothing landed) yet read differently.
   A rejection message that said "a rejected patch applied NOTHING" would remove
   the trap on its own.

### Store check (read-only, live)

`SELECT id, content, task_state, updated_at FROM block WHERE parent_id =
'block:c0450284-7413-44e6-a5dd-4680d09ad9f8'` → **9 rows**; the same query
grouped by content `HAVING COUNT(*) > 1` → **0 groups**. No duplication is
visible now. That is not evidence against the hazard (the caller may have
reconciled between then and now), but it does mean this entry does not claim the
duplicate rows are in the store.

## Missing piece

**ORACLE.** The interaction is generatable and IS generated:
`crates/holon-integration-tests/src/pbt/composed/live_mcp.rs:718` drives a real
`dense_query → edit → dense_patch` round trip through the real MCP tool and back
into the store. What is absent is any judgement on failure: no invariant
anywhere under `crates/holon-integration-tests/src/pbt/` mentions all-or-nothing,
partial apply, or atomicity (verified by grep), and the drive exercises the
happy path only. So a batch that fails halfway and leaves half its ops committed
passes every invariant the fleet has. Partly closed: `mod
dense_patch_atomicity_tests` in `frontends/mcp/src/tools.rs` now judges the
failure path at the applier's boundary. The `:718` drive still does not.

**Secondary ENVIRONMENT.** Error 2 needs a store whose `updated_at` moves under
the caller's feet — a document with write-back enabled and a live file watcher.
The `:718` drive is an append-and-confirm round trip and the same harness states
"Watches are not driven over MCP", so the echo that invalidates the snapshot is
plausibly absent in the test environment. FLAGGED, not asserted: it needs one
look at that fixture's vault before it is used for anything.

## Remedy

MITIGATED by lane `dense-patch-atomic`, base `5543f4bc`, plus a verifier delta
round. A loud partial-apply report is what landed. A compensating rollback is
measured viable and specified but not wired; see the correction below.

### No transactional seam exists, on either leg

There is no write seam that can carry a `dense_patch` plan as one unit.

- `OriginTaggedWrites::execute_batch_with_origin` (`crates/holon-core/src/traits.rs:230`)
  is the only multi-op write seam in the tree, and `SqlOperationProvider`
  (`crates/holon/src/core/sql_operation_provider.rs:3443`) is its ONLY
  production implementor. The other five are test doubles:
  `holon-integration-tests/tests/loro_suite/projection_harness.rs:131`,
  `holon-integration-tests/src/pbt/loro_sync/stub_sut.rs:404`,
  `holon/tests/sync_import_read_only_adoption.rs:112`, and
  `holon-loro/src/loro_share_backend.rs:5532,5578`. Under the Loro `CrudAuthority` the desktop
  app registers (`crates/holon-app/src/wiring.rs:334`), block CRUD is served by
  `LoroBlockOperations`, which exposes `execute_operation` alone
  (`crates/holon-loro/src/loro_block_operations.rs:1623`). So on the production
  leg there is no batch entry point to route the plan through.
- **RETRACTED CLAIM (verifier round).** An earlier version of this entry said
  "Loro offers no rollback of a sequence of local writes". That is FALSE and is
  quoted here only to retract it — it generalised "no rollback is USED in
  holon-loro" into "none EXISTS".
  `LoroDoc::revert_to(&Frontiers)` is in the pinned loro 1.13.9 (rev
  `6f5b2d7e`, `crates/loro/src/lib.rs:1469`), unused in Holon, and it does
  undo a partly-applied block batch. Measured on the block tree in
  `crates/holon-loro/src/revert_to_rollback_probe.rs` (5 tests): it restores
  the pre-batch node set, and a later unrelated write does not resurrect the
  reverted create. Two conditions make it unusable BLIND, and both are
  measured there: a concurrent REMOTE op inside the batch window is reverted
  with ours (`before=[root, theirs, ours]` → `after=[root]` — the peer's write
  is destroyed), and a revert across a shallow-snapshot boundary fails with
  `SwitchToVersionBeforeShallowRoot`. So rollback is viable only behind a
  guard that compares `oplog_vv()` per peer against the pre-batch vector and
  refuses when anyone else wrote; that guard is implemented and tested in the
  probe. Wiring it into `dense_patch` needs `LoroDocumentStore` resolved at the
  MCP layer and a Loro-mode test harness (the mcp crate's harness is SqlOnly),
  and is deliberately left as follow-up rather than shipped untested on the leg
  where a mistake deletes a peer's data.
- `LoroDocument::with_write` is the
  natural candidate and does NOT serve: it gives ISOLATION, not rollback.
  MEASURED (`crates/holon-loro/tests/with_write_is_isolation_not_rollback.rs`,
  log `lane-logs/08-probe-with-write-abort.log`) — a closure that inserts
  `STEP-ONE` and then returns `Err` leaves the doc reading `"STEP-ONE"`, and
  the next unrelated successful batch commits it under ITS origin, leaving
  `"LATER|STEP-ONE"`. `write_batch` bails before `txn.commit()`
  (`loro_document.rs`), but Loro applies local ops to `DocState` immediately
  and `WriteTxn` offers no abort — the repo already records that an
  uncommitted insert is indistinguishable at the frontiers and that any export
  commits it implicitly (`loro_document.rs`, `export_compact_snapshot`).
  Building the batch on `with_write` would therefore turn a visible partial
  apply into a DEFERRED one carrying the wrong provenance: strictly worse than
  reporting it. Half-born local writes are the related known hazard
  (`crates/holon-loro/src/import_atomicity_probe.rs:1-38`, whose verdict is
  about the IMPORT path, explicitly contrasted with local writes).
- Even SqlOnly, where the batch seam IS the authority, the plan does not fit
  it: `BatchOp` carries `create`/`update`/`delete` with a pre-minted
  `MintedPosition` (`sql_operation_provider.rs:4726-4734`), while a plan's
  `Move` and `SetState` ops dispatch `move_block` and `set_field`, whose
  order-key minting and property merge live in the provider's single-op paths.
  Rebuilding them in the MCP layer is the second-writer hazard, and would not
  help the production leg anyway.
- Compensating rollback is also unavailable: only `OpOrigin::User` operations
  push undo entries (`crates/holon-api/src/operation_engine.rs:116`) and the
  MCP facade dispatches as `OpOrigin::Agent`, so there is no journal to unwind.
  A hand-rolled inverse cannot restore a `Delete` in any case — the cascade is
  gone.

Closing this properly needs a transaction seam spanning
`DispatchingOperationEngine` and BOTH authorities. That is an architecture
change, recorded here rather than improvised. The cheaper path is the
compensating `revert_to` rollback corrected above — viable behind the peer
guard, and not a transaction.

### What landed instead

`apply_plan` (`frontends/mcp/src/tools.rs`) now dispatches op by op through
`dispatch_patch_op` and, on the first engine failure, returns
`partial_apply_error`: a message that says `PARTIAL APPLY`, names how many ops
are in the store, warns that re-applying duplicates them, and carries
structured `applied` / `failed` / `not_applied` row lists — each create
naming the id it minted, which is the caller's only handle on a row it was
never told about. A failure on the FIRST op says `applied NOTHING` instead, so
the caller is not sent reconciling rows that do not exist. The tool description
now states both halves: rejections apply nothing, a mid-write failure does not.

Pinned by three tests in `mod dense_patch_atomicity_tests`
(`frontends/mcp/src/tools.rs`), driven by an engine-level failure (a `Move` of
a block that is not there — refused by the engine, not by any plan gate):

| test | asserts |
|---|---|
| `an_engine_failure_midway_reports_exactly_what_landed` | op 1 listed applied, op 3 listed not-applied, and the store holds exactly the id the report names |
| `a_failure_on_the_first_op_reports_nothing_applied` | `partial_apply: false`, store untouched |
| `a_blind_retry_after_a_partial_apply_duplicates_the_landed_rows` | two attempts leave two copies — the hazard the report exists to prevent, pinned so it cannot be mistaken for solved |

Red log (pre-fix, op 1 persisted and the error disclosed nothing):
`lane-logs/01-red-probe.log` — `PROBE error: move_block on block:ghost failed:
… Block not found` with `child_count == 1`. Green: `lane-logs/02-green-dense-patch.log`.

### Still open, ordered by value per unit of work:

1. **Rollback via `revert_to`, behind the peer guard** — measured viable (see
   the correction above), and the highest-value open item. Needs
   `LoroDocumentStore` resolved at the MCP layer, a Loro-mode test harness,
   and the loud report kept as the disclosed fallback for three cases: SqlOnly
   mode, a window a peer wrote into, and a shallow-snapshot boundary.
2. **A real transaction seam** across the operation engine and both
   authorities. Removes the class outright instead of compensating for it, and
   would serve every batch writer rather than this one tool.
3. **Or make the plan idempotent instead**, which meets the caller's real need
   (a safe retry) without a transaction: mint each new block's uuid
   deterministically from the projection handle plus the plan's temp index, so
   re-running the same plan re-creates the SAME ids and
   `SqlOperationProvider::recognize_create` holds the existing row instead of
   duplicating it. Cheaper than (1) and not mutually exclusive with it. Not
   taken here — it widens this lane's scope and needs a ruling on whether a
   retry should also bypass the conflict gate.
4. **Make the conflict rejection say what it knows**, and publish the handle TTL
   in the tool descriptions (or return the handle's issue time with it), so the
   caller is not left inferring whether to re-apply.
5. **Lift the invariant into the keystone.** The three tests above live at the
   applier's own boundary; the MCP round trip at
   `crates/holon-integration-tests/src/pbt/composed/live_mcp.rs:718` still
   judges nothing on failure, so a fault-injected `dense_patch` there would
   cover the whole drive rather than the applier alone.

## Not established

Whether a mid-loop failure actually occurred at 00:50. The app-side log for that
session was not located in this lane, so the entry claims only what the source
proves: the two reported errors cannot have followed a dispatch, and a partial
apply is reachable by a route the report did not name. The reporter's own
inference is superseded on that point but its consequence — re-apply on a
misread error duplicates rows — is the reason the remedy above is written that
way.
