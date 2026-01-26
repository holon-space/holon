# Handoff: Turso IVM emits empty CDC batches on no-op upstream writes

## Status (2026-04-27): FIXED

**Turso side:** the empty-batch CDC fix is squashed into commit `uxpnpluk`
("Add callback mechanism for materialized view change notifications") on
the `holon` branch of `nightscape/turso.git`. Cargo.lock now pins
`58a2b659…` (was `cdd46b1c…`).

**Verification:**

1. The reproducer `crates/holon/examples/turso_ivm_focus_roots_churn.rs`
   passes phase 1: each matview emits exactly the expected 2 batches per
   navigation, 0 of 8 batches contain zero items (was 11 of 19 empty).
2. Reproducer extended with two new phases that also pass:
   - **Phase 2** — cross-region IVM filter check (`WHERE fr.region =
     'main'` correctly suppresses deltas when only `region='left_sidebar'`
     navigation_history/cursor changes).
   - **Phase 3** — `editor_cursor` (production click target) writes
     correctly produce zero CDC on `region_main_view` / `focus_roots` /
     `main_panel_view`.
3. `general_e2e_pbt` (Full variant) passes all 25 pinned regression seeds
   plus 8 random cases with zero `inv16` firings.
4. `gpui_ui_pbt` had a residual `inv16` firing of a different shape
   (`[("region:main", 4..6)]`) — see "Test-side settle fix" below.

## Test-side settle fix (separate from the Turso fix)

`gpui_ui_pbt` continued to fire `inv16` after the Turso bump, but with
*real* `Created`/`Deleted` records on `seq > target_seq` rather than
empty batches. Root cause: the assertion sampled `target_seq` before
the inbound EventBus consumers and outbound Loro→SQL reconcile had
finished the round-trip from a prior peer transition.

The flow

```
SQL write → CDC event → `loro` consumer writes Loro → subscribe_root
→ on_loro_changed → more SQL writes
```

was only fully drained *after* `assert_cdc_quiescent` ran, so the
second-half SQL writes landed inside the inv16 grace window and
looked like spurious churn.

Fix is in `crates/holon-integration-tests/src/pbt/sut.rs`
(`apply_transition_async`): drain both directions of the Loro mirror
*before* sampling `target_seq`:

```
apply → drain CDC
      → wait_for_loro_quiescence(500ms)   ← outbound (Loro→SQL)
      → wait_for_consumers(500ms)         ← inbound  (SQL→{loro,org,cache})
      → wait_for_loro_quiescence(500ms)   ← outbound round-trip
      → drain CDC again
      → assert_cdc_quiescent              ← target_seq sampled cleanly
```

Verified on the deterministic-leak seed `PROPTEST_SEED=1777281499`:
the previously-failing step 20 (`ClickBlock(LeftSidebar)`) now passes.
Test reaches step 30 before hitting unrelated invariants (see
"Remaining residuals" below).

## Investigation infrastructure

Two helpers were added during this work and are worth keeping:

- **Env-var pause hooks** in
  `crates/holon-integration-tests/src/debug_pause.rs`:
  - `PBT_PAUSE_ON_FAIL=1` — sleep before a failing assertion's panic so
    MCP / debugger / sqlite client can attach. Wired into
    `assert_cdc_quiescent`.
  - `PBT_PAUSE_BEFORE_STEP=N` / `PBT_PAUSE_AFTER_STEP=N` — bracket a
    specific transition in the GPUI/FFI driver and headless phased
    paths.
  - `PBT_PAUSE_SECONDS=<n>` — duration override (default 900s). PID is
    printed in the banner.
- **Rich inv16 spurious-item dump** in
  `crates/holon-integration-tests/src/test_environment.rs`
  (`assert_cdc_quiescent`): when inv16 trips, prints every leaked
  change record (variant, entity_id, field set, origin) instead of
  just counts. This is what identified the round-trip path above
  without needing MCP attachment.

## Remaining residuals (separate from this handoff)

These pre-date the fix and remain open:

- `[ClickBlock] focus did not propagate within 2s` — focus-propagation
  timing on the real input pipeline. Different from the empty-batch
  bug, which used to amplify it.
- `[inv14b] Frontend ViewModel contains 1 Error widget(s)` after
  `NavigateFocus` post-`CreateDocument` — render-side, not CDC.

Both surface during long `gpui_ui_pbt` runs after the inv16 fix. They
should be tracked separately.

## Problem

A materialized view chained on `current_focus → focus_roots` (UNION ALL of two
JOINs) fires `set_change_callback` for upstream transactions whose commit
**leaves the matview's output unchanged**. The callback receives a batch with
`items=0`, but the empty event still gets a fresh CDC sequence number and is
delivered to every subscriber.

This is wasteful by itself (subscribers wake up for nothing) and, in the
Holon PBT, trips a quiescence assertion that snapshots `cdc_emitted_watermark()`
and treats anything stamped after the snapshot as "spurious churn". The empty
batches arrive in the 50 ms grace window after a `navigation.focus` operation
finishes its three-statement sequence, looking exactly like backend churn even
though no consumer-visible state changed.

## Reproducer

**File**: `crates/holon/examples/turso_ivm_focus_roots_churn.rs`

**Run**:
```bash
cargo run --example turso_ivm_focus_roots_churn -p holon
```

**Result**: Exits with code 1. Output:

```
focus_roots:          5 batches (2 non-empty), 16 total items
region_main_view:     5 batches (2 non-empty), 16 total items
main_panel_view:      5 batches (2 non-empty), 28 total items
!!! BUG: focus_roots emitted 5 batches for two navigations (expected ≤ 4) — IVM is firing CDC callbacks on transactions that don't change the matview's output
[diagnostic] focus_roots and region_main_view fire in lockstep (5 matched batches, max skew 0 ms) — confirms shared upstream churn
[diagnostic] 11 of 19 CDC batches contain zero items — IVM is notifying for upstream writes that don't change matview output
```

**Timing of the test segment** (back-to-back navigations to two different docs):

| t (ms) | relation | items | what triggered it |
|---|---|---|---|
| 526 | `current_focus` | 2 | `INSERT OR REPLACE navigation_cursor` for nav 1 — real change |
| 526 | `focus_roots` | 10 | cascade of nav 1 — real change |
| 526 | `region_main_view` | 10 | cascade of nav 1 — real change |
| 526 | `main_panel_view` | 18 | cascade of nav 1 — real change |
| **527** | **`focus_roots`** | **0** | `INSERT INTO block ('journals', 'root', …)` — **no row joins focused doc** |
| **527** | **`region_main_view`** | **0** | cascade of the empty `focus_roots` event |
| **527** | **`main_panel_view`** | **0** | cascade |
| **528** | **`current_focus`** | **0** | `DELETE FROM navigation_history WHERE id > 2` — deletes 0 rows |
| **528** | **`focus_roots`** | **0** | cascade |
| **528** | **`region_main_view`** | **0** | cascade |
| **528** | **`main_panel_view`** | **0** | cascade |
| 535 | `current_focus` | 2 | `INSERT OR REPLACE navigation_cursor` for nav 2 — real change |
| 535 | `focus_roots` | 6 | cascade of nav 2 — real change |
| 535 | `region_main_view` | 6 | cascade |
| 535 | `main_panel_view` | 10 | cascade |

The four bold rows are the bug. Two upstream transactions changed nothing the
matviews care about, but every level of the chain still fired a callback.

## Schema needed to reproduce

```sql
-- Base tables
CREATE TABLE block (
    id TEXT PRIMARY KEY,
    parent_id TEXT NOT NULL,
    content TEXT DEFAULT '',
    content_type TEXT DEFAULT 'text'
);
CREATE TABLE navigation_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    region TEXT NOT NULL,
    block_id TEXT
);
CREATE TABLE navigation_cursor (
    region TEXT PRIMARY KEY,
    history_id INTEGER REFERENCES navigation_history(id)
);

-- Level 1
CREATE MATERIALIZED VIEW current_focus AS
    SELECT nc.region, nh.block_id
    FROM navigation_cursor nc
    JOIN navigation_history nh ON nc.history_id = nh.id;

-- Level 2 (UNION ALL of two JOINs that read current_focus + block)
CREATE MATERIALIZED VIEW focus_roots AS
    SELECT cf.region, cf.block_id, b.id AS root_id
    FROM current_focus cf
    JOIN block b ON b.parent_id = cf.block_id
    UNION ALL
    SELECT cf.region, cf.block_id, b.id AS root_id
    FROM current_focus cf
    JOIN block b ON b.id = cf.block_id;

-- Level 3 — any consumer view chained on focus_roots replays the empty batches
CREATE MATERIALIZED VIEW region_main_view AS
    SELECT fr.root_id AS id, b.content, b.parent_id
    FROM focus_roots fr
    JOIN block b ON b.id = fr.root_id
    WHERE fr.region = 'main';
```

## What triggers the empty callbacks

After data is loaded and `navigation_cursor` points at a focused doc, the
following transactions each fire an empty CDC callback on `focus_roots` (and
all chained downstream views):

1. **`INSERT INTO block (id, parent_id, ...) VALUES (..., 'unrelated_parent', ...)`** —
   adds a row whose `parent_id` is not the focused block, so neither branch
   of `focus_roots`' UNION ALL matches. The expected delta on
   `focus_roots` is empty, but a batch fires anyway.

2. **`DELETE FROM navigation_history WHERE id > N` matching 0 rows** — the
   delete is a no-op. `current_focus` cannot have changed (no row was
   removed), but a batch fires on `current_focus` anyway, and that empty
   batch then cascades through the full chain.

In the production app these two patterns appear together because the
navigation provider does:

```rust
// crates/holon/src/navigation/provider.rs:35-110, focus()
DELETE FROM navigation_history WHERE region = $region AND id > $current_id;  // tx 1 — usually 0 rows
INSERT INTO navigation_history (region, block_id) VALUES ($region, $block_id);  // tx 2
SELECT max(id) FROM navigation_history;  // tx 3 — read-only, no CDC
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ($region, $new_id);  // tx 4
```

Only tx 4 actually changes the focused root set, but tx 1 also fires an empty
batch on every downstream view of `focus_roots`.

## Hypothesised root cause

The DBSP delta computed for these transactions is genuinely empty (the
matview's output is identical before and after), but the IVM commit pipeline
still calls `set_change_callback` rather than short-circuiting. Plausible
fix sites:

- The change-feed dispatcher: emit only when the resulting delta has at
  least one row.
- The UNION ALL operator: when both branches contribute no row, propagate
  "no-op" upstream rather than an empty batch.
- The cascade traversal: when an upstream operator returned a no-op delta,
  skip the downstream operator's commit entirely.

I haven't dug into Turso's IVM internals to determine which is the right
fix; flagging the symptom and a small repro feels more useful than guessing.

## What this is **not**

- Not a CDC ordering or sequence-number gap bug — sequence numbers are
  monotonic and consistent across views.
- Not a duplicate-subscription bug — `current_focus → focus_roots`
  cascade fires once per upstream commit; the issue is that the commit
  itself is firing the callback at all.
- Not the `turso_ivm_chained_matview_stale_rows` issue (that's about a
  matview returning *wrong* rows; here the rows are correct, just
  redundantly notified).
- Not the `turso_ivm_recursive_cte_join_repro` issue (recursive CTE not
  required — `region_main_view` is a plain JOIN; including the
  `main_panel_view` recursive variant only confirms the empty batches
  cascade through every shape of consumer).

## Production impact (Holon side)

`gpui_ui_pbt` (`crates/holon-integration-tests/tests/general_e2e_pbt.rs`)
panics with `[inv16] CDC not quiescent — spurious events: [("region:main",
9..11)]` during sequences of `ClickBlock` + `NavigateBack` transitions. The
empty batches stamp fresh sequence numbers that arrive in the 50 ms grace
window after `cdc_emitted_watermark()` is snapshotted, so the test
classifies them as backend churn. See
`crates/holon-integration-tests/src/test_environment.rs:1208-1339`
(`assert_cdc_quiescent`).

The same churn explains a downstream PBT failure mode where
`ClickBlock(Main)` reports `focus did not propagate within 2s` — every
empty batch tears down and recreates the EditorView entity for the
focused row, dropping the in-flight `set_focus` write before the test's
poll observes it.

## Side observations (not Turso bugs, recorded for context)

These came up while isolating the churn but are independent issues to
deal with on the Holon side:

- `crates/holon/src/sync/matview_manager.rs:180-183` happily registers
  the same `tx` twice for the same `view_name` if `query_and_watch` is
  called for the same SQL by two code paths. In the live app this
  produced `subscribers=2` for the production main-panel view because
  both `BlockDomain::render_entity` (backend) and
  `frontends/gpui/src/render/builders/live_block.rs:39` (`watch_live`
  via the GPUI live_block builder) subscribe. Test framework
  (`setup_region_watch`) uses a different SQL string and ends up on a
  different view, but the duplicate-subscription pattern is real
  latent waste.
- `crates/holon-integration-tests/src/pbt/phased.rs:103-115` now reads
  `PROPTEST_SEED` from env. It pins the early structure of a run but
  is not fully deterministic because `preconditions(...)` re-rolls
  when async timing changes which transitions are admissible — two
  runs at the same seed can diverge after a few steps.

## Next steps in the Turso repo

1. Drop in the schema and 3-statement sequence above into a Turso-side
   integration test under `turso/tests/integration/query_processing/`
   modeled on `test_ivm_join_cursor_corruption.rs` (referenced in
   `devlog/HANDOFF_TURSO_IVM_JOIN_PANIC.md`).
2. Hook `set_change_callback` and assert that an `INSERT INTO block`
   that matches no `focus_roots` predicate produces zero callbacks on
   `focus_roots`.
3. Same with a `DELETE` that matches zero rows — should produce zero
   callbacks on `current_focus` and downstream views.
4. If both fail, look at the IVM commit dispatcher / DBSP graph
   walker for whoever calls into the change-feed publisher.

## Discovery context

Found while diagnosing `gpui_ui_pbt` failures on the
`feat: rewrite ReactiveViewModel to persistent-node architecture`
commit (`08e314556`). The "Main panel renders empty + ClickBlock focus
doesn't propagate" symptoms in the screenshots at
`frontends/gpui/target/pbt-screenshots/gpui/` are downstream effects of
this churn — every empty batch causes the GPUI `ReactiveShell` to
rebuild the Main panel's tree, dropping in-flight focus updates.

Reproducer chain (smallest to largest):
- Single navigation, simple JOIN view: **does not** trigger the bug.
- Two back-to-back navigations with one no-op `INSERT INTO block`
  between them: triggers 1 extra empty batch per matview.
- Two back-to-back navigations with `INSERT INTO block` + matches-zero
  `DELETE FROM navigation_history`: triggers 3 extra empty batches per
  matview — what the example currently exercises.

## Files

- Repro: `crates/holon/examples/turso_ivm_focus_roots_churn.rs`
- Production navigation provider: `crates/holon/src/navigation/provider.rs`
- Production matview definitions: `crates/holon/src/storage/turso_ivm_navigation_cursor_repro.rs:75-98` (canonical reference)
- Production main-panel query: `assets/default/index.org:20-24`
- Existing related Turso handoffs: `devlog/HANDOFF_TURSO_IVM_*.md` (12 of them; this churn is distinct from the panic / stale-rows / chain-break bugs already filed)
