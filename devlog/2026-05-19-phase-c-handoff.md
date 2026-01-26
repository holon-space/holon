# Phase C — Handoff & TODOs

**Worktree**: `.claude/worktrees/phase-c-focus-edit-2`
**Commits**: `vpvytxws d2fdf539` (Phase C #1–#7) + `ukknutys 2f267a57` (Phase C #8)

## What Phase C delivered

- **5 new capability primitives** on `SutLayout` / `SutDriver`:
  `wait_for_bounds`, `wait_for_widget_kind`, `click_entity`,
  `wait_for_engine_focus`, `send_raw_keystroke`.
- **5 transitions migrated** to user-faithful input pipelines:
  `apply_focus_editable_text`, `apply_click_block`, `apply_split_block`
  (input pipeline only), `apply_toggle_state`, `apply_trigger_slash_command`.
- **3 transitions deleted** as frontend-integration-tests-in-disguise:
  `apply_edit_via_display_tree`, `apply_edit_via_view_model`,
  `apply_trigger_doc_link`. Their structural assertions are covered by
  pre-existing or newly added unit tests in `holon-frontend`.
- **1 deferred invariant promoted to live**:
  `inv-viewmodel-editable-text-triggers` (with companion
  `WidgetSnapshot.props["trigger_count"]` IR extension).
- **2 archlint smell rules**: `pbt-transition-apply-intent` and
  `pbt-sut-handle-frontend-simulation` lock the cleanup permanent.
- **Frontend companion change**: popup overlay items now register in
  `BoundsRegistry` as `popup_item` / `popup_item_selected` widget kinds.
- **Net**: ~670 LOC removed from `sut_handle.rs`; all 4 `apply_intent`
  smells in PBT bodies eliminated.

---

## TODOs

### 🔴 Validation gap (highest signal)

- [ ] **Wide PBT generator tuning so migrated transitions actually fire.**
  Wide PBT run on this commit: all 5 migrated transitions had 14–44
  rejections each across 8 cases × 2 tests (`general_e2e_pbt` and
  `general_e2e_pbt_sql_only`). 0 executions. The generator never reaches
  states where state_toggle / slash menu / editable_text widgets render
  long enough for these transitions to satisfy preconditions. Options:
  (a) bump generator weight on these transitions; (b) inject seeds in
  `.pbt-regressions/` that drive the test session into qualifying states
  early; (c) tune `StartApp` / `BulkExternalAdd` to produce text blocks
  more aggressively. Without this, migration correctness rests on slim
  slices + unit tests; wide-PBT exercise is unproven.

- [ ] **Pre-existing wide-PBT flakes (NOT Phase C — but blocking
  wide-PBT signal).** Two failures observed during Phase C validation:
  1. `general_e2e_pbt`: `inv-org-render-fixed-point` — org file would
     be rewritten on next `re_render_all_tracked` pass. Known per
     MEMORY.md "Org renderer matview lag fix (May 2026 — LANDED)" but
     likely a regression of that fix or a sibling lag.
  2. `general_e2e_pbt_sql_only`: three distinct panics —
     `inv-editable-text-has-draggable` CDC churn, nav focus mismatch
     (`block:journals` vs `block:ref-doc-0`), org file block ordering
     mismatch. Looks like multiple unrelated infra bugs.
  Until these are fixed, wide PBT cannot validate anything.

### 🟡 Remaining fat transitions in `sut_handle.rs`

After Phase C, three fat transitions still carry inline business logic
(but none have `apply_intent` smells):

- [ ] **`apply_bulk_external_add` (288 LOC).** Bulk add via org file
  write. Likely shares surface with `WriteOrgFile`. Migration would
  extract a `SutOrgFileWrite::write_org_file(path, content)` primitive
  if not already present, plus the bulk-block construction logic. Could
  benefit from a `wait_for_blocks_appeared(expected_ids, timeout)`
  capability on `SutSqlProjection`.

- [ ] **`apply_start_app` (211 LOC).** Lifecycle setup. Has a dedicated
  `SutLifecycle` trait already with `apply_start_app(&mut self)`, but
  the implementation in `sut_handle.rs` is still inline. Migration is
  mostly moving the body into a free helper bound on `SutLifecycle +
  SutSqlProjection` (or similar).

- [ ] **`apply_split_block` (~115 LOC remaining after Phase C #3).**
  The input-pipeline portion was extracted in Phase C #3; the
  adapter still carries pre-bounds SQL probe diagnostic, pre-
  children-settled gate from `pre_ref_state`, pre/post SQL probes,
  block-count assertion, synthetic-id mapping, post-Enter focus
  barrier. Most of this is genuinely test-infrastructure (it reads
  `E2ESut`-internal state). Extracting further requires capability
  surfaces for: SQL probe diagnostic helper, `wait_for_blocks_synced`,
  synthetic-id mapping. Whether to migrate depends on whether a
  second slice needs them. Defer until a slice asks.

### 🟡 Deferred invariant bodies (`pbt/invariants/bodies/`)

Phase C #7 promoted `inv-viewmodel-editable-text-triggers` to live.
Per `PbtSlicing.md` and a fresh audit, the remaining `Skipped` bodies
that are still true deferrals:

- [ ] **`matview_consistent_with_ref`** (~3–4 hr). Needs
  `SutViewModel::root_layout_data_row_ids()` plus 4 ref-side caps
  (`RefBlockTreeReadAll`, `RefLayoutBlocks`, `RefFocusRoots`,
  `is_descendant_of_any`, `expected_focus_root_ids`). Plus
  `RunMode::Warn` + a `Skipped`-path classifier for the soft-check
  semantics that the inline version had.

- [ ] **`viewmodel_tree_virtual_slots`** (~1 day). Needs `display_tree`
  wired into `WidgetSnapshot` and virtual-slot entity IDs threaded
  through the IR. Larger architectural lift.

Re-audit the remaining 3 `Skipped` bodies — capability surface has
grown via Phase C (now has `wait_for_widget_kind`,
`wait_for_engine_focus`, etc.), some may be promotable now:

- [ ] **`backend_blocks_match_ref`** — audit if it can use existing caps.
- [ ] **`watch_rows_match_ref`** — audit.
- [ ] **`viewmodel_decompiled_rows_match_query`** — audit.

### 🟡 Drop-assertion follow-ups (catalogue, not blockers)

Phase C dropped these structural concerns when deleting transitions
or removing apply_intent paths. Most are covered elsewhere; the
unchecked ones below could become invariants if a regression surfaces:

- [ ] **`cycle_task_state` op has keychord joined from keybinding
  registry.** Was inline in `apply_toggle_state` pre-#4. If a regression
  in keychord-join surfaces, add `inv-state-toggle-keychord-bound`
  bound on `SutRenderer`. Today: not covered.

- [ ] **`render_entity` contract tests for Text blocks** (set_field op
  on operations, non-empty triggers from render-DSL not from VM
  snapshot). Implicitly exercised by every Text block PBT run; explicit
  contract tests would catch regressions earlier. Belongs in
  `crates/holon-frontend/tests/render_entity_contract.rs`.

- [ ] **`inv-viewmodel-editable-text-triggers` could be strengthened**
  to assert specific trigger ACTION names (e.g. "doc_link",
  "command_menu") rather than just `trigger_count > 0`. Extend
  `WidgetSnapshot.props["triggers"]` to encode action names comma-
  separated; tighten the invariant body. Catches: doc_link trigger
  silently disappearing.

### 🟢 Pure-modularity cleanup (no behavior change)

- [ ] **`check_invariants_async` decomposition.** 8 of 14 sections
  still inline in `sut_check_invariants.rs`; Phase D5 extracted 6 of
  14 into named `check_inv_<name>` methods. Per PbtSlicing.md,
  remaining are sections `1`/`1b`/`2`/`2b`/`3`/`7`/`8`/`9/10`. Each
  extraction is ~10 LOC of moving the local `resolve` closure into
  a `self` method. ~2–3 hr total. Pure narrative win.

### 🟢 New-architecture opportunities

Phase C extracted capability primitives that enable new slices and
invariants. Some forward-looking ideas:

- [ ] **In-memory + GPUI slice** (Phase 9, deferred per H7 audit).
  Now blocked on `BuilderServices` matview-required count > 2 and
  LOC budget; revisit if cross-frontend (Flutter, web) consumers
  materialise. Would let GPUI layout PBT run in microseconds.

- [ ] **CLI-driven slice** for the migrated transitions. With
  `SutDriver::send_raw_keystroke` and `click_entity` as proper caps,
  a TUI-like slice that drives only via key chords becomes feasible.
  Test the TUI frontend's input pipeline as a PBT consumer.

- [ ] **Promote `Cell<T>` registry assertions to invariants.** Phase 2
  Cells-as-universal-primitive landed; per-field cell backing structs
  were deferred. A `RefCellMirror` capability + invariant could check
  that every `BlockCellRegistry::live_field` value matches SQL/Loro.

- [ ] **archlint smell: ban new fat SutHandle methods.** The current
  smell forbids specific anti-patterns (`apply_intent`, frontend-sim
  constructs) but doesn't cap method body LOC directly. Could implement
  via a Python script (archlint custom rule) that counts lines between
  `async fn apply_*` markers and warns above 60 LOC. ~30 min.

- [ ] **Migrate thin transitions opportunistically.** 40 thin
  transitions in `sut_handle.rs` (4–20 LOC each) are pure chord/driver
  dispatches. Per PbtSlicing.md: "not worth migrating until a slice
  asks for them." When a new slice consumer needs one, migrate that
  one in isolation.

### 🟢 Documentation

- [ ] **Update `PbtSlicing.md`** with the Phase C section: the 5 new
  capability primitives, the 5 transitions migrated, the 3 deleted,
  the invariant promoted, and the 2 archlint smell rules. The doc
  currently stops at Phase 10 / Stage B work.

- [ ] **Update `MEMORY.md` index** with a Phase C entry pointing to
  this handoff.

- [ ] **`TUI TODO A6` marker.** TUI's TODO had A6 marked `[x]` (done)
  but it wasn't — the transitions were still apply_intent shortcuts.
  Phase C #5 + #8 actually completed the deletion. Update the checkbox
  comment to reference Phase C #5/#8.

---

## Recommended next sprint

If the goal is "Phase C delivers proven value":

1. **Fix the two pre-existing wide-PBT flakes** (`inv-org-render-fixed-point`
   and SqlOnly CDC quiescence + nav focus). Without these, wide PBT can't
   validate ANYTHING.
2. **Generator tuning** so migrated transitions actually fire end-to-end
   in the wide PBT. One full-coverage wide-PBT pass would close the
   validation loop.
3. **Re-audit deferred bodies** — capability growth in Phase C may have
   unlocked promotions cheaply.

If the goal is "compounding Phase C's win":

1. Pick one of the remaining fat transitions (`apply_bulk_external_add`
   is the highest LOC; `apply_start_app` is the most architecturally
   distinct).
2. Strengthen `inv-viewmodel-editable-text-triggers` to assert action
   names — quick win, catches more regressions.
3. Sketch the CLI-driven slice using the new primitives — could unlock
   TUI frontend testing.

---

## Files of interest for the next session

- `crates/holon-pbt-core/src/capabilities.rs` — 5 new caps live here
- `crates/holon-integration-tests/src/pbt/sut_capabilities.rs` —
  E2ESut impls of the new caps (delegate to existing `pub(super)` fns
  in `sut.rs`)
- `crates/holon-integration-tests/src/pbt/transitions/` — 5 free helpers
  added: `focus_editable_text.rs`, `click_block.rs`, `split_block.rs`,
  `toggle_state.rs`, `trigger_slash_command.rs`. Canonical templates
  for future migrations.
- `crates/holon-integration-tests/src/pbt/invariants/bodies/viewmodel_editable_text_triggers.rs`
  — promoted invariant. Templates for future invariant bodies.
- `archlint/smells/pbt_transitions.toml` — the two new smell rules
- `frontends/gpui/src/views/editor_view.rs:889` (`render_popup`) — popup
  items now register in BoundsRegistry; pattern for future
  popup-overlay observability
