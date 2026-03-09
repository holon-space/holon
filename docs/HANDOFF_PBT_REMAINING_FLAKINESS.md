# Handoff — Remaining flakiness in `general_e2e_pbt`

Last updated: 2026-04-19 (fifth pass)

## TL;DR

- The `effort` custom-property fix **still holds**. No panics mention
  `effort`. External-mutation spot-check poll loop in
  `crates/holon-integration-tests/src/pbt/sut.rs:4275-4328` remains.
- **Previously fixed**: `vfn11` invariant false-positive (NavigateHome
  focus bookkeeping) and Bug #1 (Loro outbound reconcile regressing SQL
  via stale full-row UPDATEs; fixed via `_expected_content` WHERE
  guard). See history below.
- **Fixed this pass (Bug C)**: content trailing-whitespace divergence
  in the reference model. The external serializer
  (`serialize_block_recursive` in `org_utils.rs:144`) trims the title
  line via `.trim_end()` before writing it as the headline. The org
  parser then reads back the title-trimmed content. But
  `Mutation::Update::apply_to` in `pbt/types.rs` only called
  `.trim_end()` on the *entire* string, which doesn't touch trailing
  whitespace on the first line if further lines follow. So for a
  mutation with content `"nL q \npAWaeG 212i TE"`, ref_state stored
  the trailing space and backend SQL stored it trimmed, and the UI
  spot-check at `sut.rs:4241` flagged the divergence as soon as any
  later UI mutation touched the same block. Fix: added
  `normalize_content_for_org_roundtrip` in `pbt/types.rs` that splits
  on the first `\n`, `trim_end`s the title line, and preserves body
  verbatim (source blocks pass through unchanged). Applied in both
  `Mutation::Create` and `Mutation::Update` paths. `sql_only` variant
  now passes 50/50 cases.
- **Bug #2 (peer merge) still surfaces** in cross_executor / Full at
  50 cases. With Bug C cleared, the shrinker has started hitting a
  cleaner variant: an External `Update` (content change) that never
  reaches SQL when Loro is enabled, even though the file-parse diff
  *should* emit the `update` op. Prepare_update logs show the UPDATE
  never gets built for the target block — OrgSyncController appears
  to skip the block as unchanged, or the file-watcher event is being
  coalesced with OrgSync's own re-render. Same failure family as the
  peer-merge case (inherited from the original handoff's Bug #2).
  Still unfixed.
- **Bug A (sql_only entity-profile drop)**: no longer observed in 50
  cases after the Bug C fix — the shrink space may have shifted. Not
  yet confirmed resolved; re-run sql_only with a larger `PROPTEST_CASES`
  before claiming it fixed.

## The vfn11 fix (completed today)

**Symptom before fix**:
```
thread 'general_e2e_pbt_cross_executor' panicked at
crates/holon-integration-tests/src/pbt/sut.rs:3268:25:
[vfn11] active render_expr mentions focus_chain and reference model
has focused_block = Some(EntityUri("block:root-layout")), but no
streaming provider produced rows
```

**Root cause**: In production, `maybe_mirror_navigation_focus` in
`crates/holon-frontend/src/reactive.rs:1824` globally clears
`UiState.focused_block` on `go_home`. The SUT's
`E2ETransition::NavigateHome` mirror at `sut.rs:649` calls
`ui_state.set_focus(None)` to match. But the ref-state transition
handler in `state_machine.rs` only cleared per-region state
(`focused_entity_id`, `focused_cursor`) — it did **not** clear the
global `focused_block`. So after a
`NavigateFocus{Main, X}` → `NavigateHome{LeftSidebar}` sequence:
- production/SUT engine: `focused_block = None` (global clear)
- ref_state: `focused_block = Some(X)` (stale)

Next render that read `focus_chain()` got zero rows from production,
but ref_state still asserted `focused_block.is_some()` so vfn11 fired.

**Fix**: one line in `state_machine.rs:2010` — set
`state.focused_block = None` in the NavigateHome branch. See the
comment there.

**Why this matters**: vfn11 is a *test* invariant, not a code bug; the
false-positive was hiding the real bug below it in the shrink space.

## Verifying you're on top of the right bugs

```bash
PROPTEST_CASES=50 cargo nextest run -p holon-integration-tests \
  --test general_e2e_pbt 2>&1 | tee /tmp/pbt.log | tail -30
```

Expected after the vfn11 fix (results non-deterministic):

```
FAIL  ... general_e2e_pbt_cross_executor   (Bug #1 or Bug #2)
PASS or FAIL  ... general_e2e_pbt          (likely PASS most runs)
PASS or FAIL  ... general_e2e_pbt_sql_only (Bug A if it hits)
```

The failure at `sut.rs:3268` (vfn11) should be gone. If it reappears,
the NavigateHome fix regressed — grep `state.focused_block = None` in
state_machine.rs.

---

## Bug #1 — UI mutation content spot-check race (cross_executor / Full)

### Status: FIXED 2026-04-19 (fourth pass)

Verified with `PROPTEST_CASES=50` cross_executor runs: `sut.rs:4241`
content mismatch no longer surfaces. The underlying race (described
below) was closed by making Loro-origin UPDATEs conditional on SQL's
current content still matching the Loro "before" state. See
`crates/holon/src/sync/loro_sync_controller.rs:diff_snapshots_to_ops`
and `crates/holon/src/core/sql_operation_provider.rs:prepare_update`
for the fix.

Historical description preserved below.

### Symptom

```
thread 'general_e2e_pbt_cross_executor' panicked at
crates/holon-integration-tests/src/pbt/sut.rs:4241:21:
assertion `left == right` failed: Post-mutation spot-check:
content mismatch for block 'block:e-0kvq-k1-0----s--63--jm5-37g'
  left:  "S qMd7 9"       # stale content from a prior mutation
  right: "H ue J"         # content we just dispatched via synthetic_dispatch
```

Panic fires immediately after `synthetic_dispatch returned Ok` and
`Block count matched (21)`.

### Trigger recipe (observed today — same as original handoff)

1. Pre-startup `WriteOrgFile` containing a block with ID
   `e-0kvq-k1-0----s--63--jm5-37g` and content `"S qMd7 9"`.
2. `StartApp { enable_loro: true }`.
3. A chain of mutations that keeps the row alive without changing this
   field (in the failing run: `BulkExternalAdd`, various
   `ApplyMutation(External)`).
4. `ApplyMutation(MutationEvent { source: UI,
     mutation: Update { id: "…e-0kvq-…", fields: { "content": "H ue J" } } })`
5. Direct dispatch of `block.set_field`. Spot-check races SQL and loses.

### Where the assertion lives

`crates/holon-integration-tests/src/pbt/sut.rs:4212-4258` — the UI
branch of `apply_mutation`'s spot-check. One-shot SQL query, no poll.
The External branch below (lines 4275-4328) polls for up to 5s.

### Root cause hypothesis (refined after reading the code)

The UI dispatch path is *synchronous end-to-end for SQL* (synthetic_dispatch
→ `BackendEngine::execute_operation` → dispatcher → `SqlOperationProvider::execute_batch`
→ SQL commit, all awaited). The spot-check reads raw SQL via
`BackendEngine::execute_query` which goes through
`db_handle.query(&sql, params)` — no matview, no CDC, direct
ExecuteBatch path. So SQL read-after-write should see the fresh value.

The race window is elsewhere: when **Loro is enabled** (this bug is
Loro-only; `sql_only` never hits it), a prior operation's
`on_loro_changed` may be running in a background task with a snapshot
of Loro state taken *before* this UI's SQL write. The outbound
reconcile emits `execute_batch_with_origin(..., EventOrigin::Loro)`
with `block_to_params(new_block)` — **all fields, not just changed
ones**. If Loro's "current" snapshot (at reconcile time) lags the SQL
write we just did, the reconcile overwrites our fresh content with
Loro's stale view.

Flow:
```
T0: UI synthetic_dispatch starts
T1: SQL UPDATE "H ue J" commits (synthetic_dispatch returns)
T2: event published to bus
T3: (concurrently) some prior on_loro_changed, triggered earlier,
    reads after=snapshot_blocks_from_doc(&doc) where Loro still has
    "S qMd7 9" for this block — its diff emits an UPDATE with "S qMd7 9"
T4: execute_batch_with_origin(Loro, [update "S qMd7 9"]) → SQL overwrites
T5: our spot-check reads "S qMd7 9"
```

The key observation that supports this: `on_loro_changed` in
`crates/holon/src/sync/loro_sync_controller.rs:298-345` is the only
code path that calls `execute_batch_with_origin(..., EventOrigin::Loro)`
against SQL, and it emits full-row updates via `block_to_params` — so
any stale `after` snapshot silently regresses the target row.

### Suggested fixes, in order of preference

1. **Don't let `on_loro_changed` overwrite a row that SQL already has
   correct** — e.g. have `execute_batch_with_origin(Loro, ...)` skip
   UPDATEs where the SQL row already equals the emitted content. A
   read-before-write in the Loro path. Costs one extra SELECT per
   reconciled row but makes the outbound reconcile strictly idempotent
   against concurrent direct SQL writes.
2. **Snapshot SQL, not Loro, as `after`** when building the diff.
   Loro and SQL must agree at quiescence; snapshotting SQL's current
   state would close the race because SQL is the authoritative target.
   Bigger refactor, but potentially cleaner.
3. **Pre-UI-dispatch quiescence wait** in the test — before every UI
   synthetic_dispatch, wait for Loro to be quiescent. Masks the bug;
   doesn't fix it. The handoff's prior author and I agree this is a
   patch, not a resolution.
4. **Polling UI spot-check** matching the External branch. Same caveat
   as #3 — patch only.

The closer to (1) the better — it addresses the architectural
invariant that the outbound Loro reconcile must never regress SQL.

---

## Bug A — `sql_only`: blocks differ (missing entity-profile children)

### Symptom

```
thread 'general_e2e_pbt_sql_only' panicked at
crates/holon-integration-tests/src/assertions.rs:55:5:
assertion `left == right` failed: Backend diverged from reference:
Blocks differ between actual and expected

 left:  [ ...7 blocks, correct doc blocks + content blocks... ]
 right: [ ...9 blocks, including two that are missing from the backend:
   Block { id: "block:w414--",            content: "L6Nh",
           parent_id: "block:8183a051-…" (doc), content_type: Text, ... },
   Block { id: "block:w414--::src::0",
           parent_id: "block:w414--",     content_type: Source,
           source_language: Some(Other("holon_entity_profile_yaml")),
           content: "entity_name: block\ncomputed: …\nvariants: …" },
 ]
```

### Trigger recipe (from run `/tmp/pbt_full_50.log`)

Three pre-startup org files, the interesting one being
`qntq_s_tu__o_q___eg_…_.org`:

```org
* L6Nh
:PROPERTIES:
:ID: w414--
:END:
#+BEGIN_SRC holon_entity_profile_yaml :id w414--::src::0
entity_name: block
computed:
  has_task_state: "= task_state != ()"
variants:
  - name: task
    priority: 1
    condition: "= has_task_state"
    render: 'row(col("content"))'
  - name: default
    priority: -1
    render: 'row(col("content"))'
#+END_SRC
```

Then `StartApp { enable_loro: false }`, `EmitMcpData`, `EmitMcpData`,
UI `set_field` on `block:w414--::src::0`, external `Create` of
`block:block-3`, assertion fires.

Block count *after StartApp* is already short by 2 — the blocks are
missing from ingest, not deleted later. `wait_for_org_file_sync` still
reports `qntq… synced (2 blocks)`, so the file has them but OrgSync
didn't land them in SQL.

### Where to look

- `crates/holon/src/entity_profile.rs:1111` —
  `is_profile_block_by_source_language` identifies
  `holon_entity_profile_yaml` src blocks, but has **no callers**.
  Suggests a refactor removed the caller while leaving the helper. If
  OrgSync previously used this to split profile blocks into a separate
  table, the split may now drop them entirely.
- `crates/holon/src/sync/org_sync_controller.rs` — instrument the
  ingest loop; log which blocks are being upserted for
  `qntq_s_tu__o_q___eg_…_.org`. If the heading `* L6Nh` and its src
  child never appear in the upsert stream, the parser-to-upsert
  pipeline is at fault; if they appear but don't land in SQL,
  `SqlOperationProvider::prepare_create` or its event-driven
  QueryableCache subscriber is rejecting them.
- `assets/default/types/block_profile.yaml` — check whether the
  ingest pipeline skips user-authored profile blocks because it
  considers them "already installed".

### Hypotheses ranked

1. **Orphaned profile-specific handling.** A prior refactor moved
   entity profiles into a dedicated table; the old "skip-these-blocks"
   guard in OrgSync was left behind but the new ingest path never got
   wired. Net effect: profile blocks disappear from the `block` table.
2. **`content_type=source` + `source_language=holon_entity_profile_yaml`
   fails to serialize** through one of the `Block` `#[serde(…)]`
   boundaries. Less likely — the reference model constructs these and
   the assertion pretty-prints them cleanly, so the type seems
   round-trippable. But verify via a unit test that round-trips a
   profile block through `SqlOperationProvider::build_event_payload`
   and back.
3. **Block parent chain violation.** The heading `L6Nh` might be
   getting its parent_id rewritten to something the downstream
   subscriber rejects. The ref model expects
   `parent_id="block:8183a051-…"` (doc); if OrgSync rewrites this to
   something else and then the subscriber drops on mismatch, both
   blocks vanish.

---

## Bug #2 (original handoff) / "Org file diverged" variant

### Status: related failure surfaces in cross_executor after vfn11 fix

The original `assertions.rs:55` "Blocks differ … peer-created children
missing from SQL" didn't reproduce today, but a very similar
**"Org file diverged from reference: Blocks differ"** at the same
assertion site fires around `SyncWithPeer` in cross_executor. The
diff points to a single block whose content is *older* than the ref
model expects — strongly suggesting a peer merge that updated the
block didn't propagate all the way through the SUT's org-file
projection.

Same investigation order as the original handoff's Bug #2 applies —
start by instrumenting `LoroSyncControllerHandle::error_count()`
around `SyncWithPeer` / `MergeFromPeer` and turning any silent
advancement into a loud panic.

---

## Useful commands

```bash
# Repro sql_only variant
PROPTEST_CASES=50 cargo nextest run -p holon-integration-tests --test general_e2e_pbt \
  general_e2e_pbt_sql_only 2>&1 | tee /tmp/pbt-sql.log

# Repro cross_executor (Bug #1 surfaces most runs now that vfn11 is fixed)
PROPTEST_CASES=50 cargo nextest run -p holon-integration-tests --test general_e2e_pbt \
  general_e2e_pbt_cross_executor 2>&1 | tee /tmp/pbt-cx.log

# Full sweep
PROPTEST_CASES=50 cargo nextest run -p holon-integration-tests --test general_e2e_pbt \
  2>&1 | tee /tmp/pbt.log

# Slice the log around a panic
grep -n "panicked at\|Blocks differ\|vfn11\|left:\|right:" /tmp/pbt.log
```

Shrunk regression seeds live in
`crates/holon-integration-tests/tests/general_e2e_pbt.proptest-regressions`.

## Out of scope for this handoff

- Rework the outbound Loro reconcile to avoid regressing SQL (Bug #1's
  proper fix).
- `wait_for_loro_quiescence` architecture review (could collapse into a
  single test-side barrier that waits for both outbound dispatch AND
  downstream cache drain).
- Turso IVM matview CDC lag (tracked separately — see the
  `turso-chained-matview-hang` skill and related memories).
