# LogSeq Right Sidebar — Follow-up Handoff

Branch: `logseq-right-sidebar` (jj worktree at `.claude/worktrees/logseq-right-sidebar`)
Plan: `~/.claude/plans/agree-with-all-matches-merge-maybe-playful-cosmos.md`
Commit graph (top-down):

```
ytouomrx   feat(logseq): Phase D + post-review cleanup (combined)
zknyxxqs   feat(logseq): Phase C — focus_roots tombstone + action layer
umsxqomm   feat(logseq): Phase B — flag chain + rules predicate evaluation
pvryopot   feat(logseq): Phase A — per-row LocalEntityScope routing
uqtvotzp   <baseline>
```

Workspace builds clean; `entity_view_registry` unit tests + `loro_paths_do_not_reference_navigation_tables` regression test pass. The flaky `layout_invariants_hold_for_random_scenarios` proptest is pre-existing (verified against baseline).

---

## What ships now

The right sidebar gains LogSeq-style behavior:

- **Shift-click on a block's bullet** → calls `focus_pin(region: "right", block_id: <id>)`. Move-to-top dedup: if already pinned, refresh timestamp; else insert. Plain click still drives drag (stop_propagation only on shift path).
- **Right sidebar query** (`assets/default/index.org:23-32`): GQL `MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..20]-(d:block) WHERE fr.region = 'right' AND root.id = fr.root_id RETURN d ORDER BY fr.added_ts DESC, d.sort_key`.
- **Render**: `tree(rules: [#{when: eq("level", 0), override: #{role: "page_title", show_bullet: false, show_chevron: false}}], item_template: render_entity())`. Level-0 rows render as h1 text via the `page_title` block_profile variant; descendants render normally; bullet/chevron suppressed on titles.
- **Navigation history** carries `closed_at TEXT NULL`; `focus_replace` closes prior open before insert; `close(history_id)` is the X-button op.

---

## Open follow-ups

Each entry is sized to be a focused single-session task. Estimates are in claude-coding-units (CCU); rule of thumb is 30-90 min wall-clock per CCU including PBT runs.

### FU-1 · Per-instance state on `ReactiveViewModel` (not row-scoped HashMap)

**Status**: tree_item collapse PARTIALLY DONE (this session). `editable_text` input state remains on `CacheKey::Ephemeral`.

**Architectural correction**: an earlier draft of this FU proposed adding a `row_caches: HashMap<String, EntityCache>` field to `ReactiveShell` keyed by row id. That's the wrong abstraction — keying by row id doesn't isolate same-id rows in the same shell, and re-creates the indirection the project is moving away from. The actual final plan was already established by `expand_toggle.rs`: **state-bearing handles live directly on the `ReactiveViewModel` instance** (or an associated struct held on it). Each Arc<ReactiveViewModel> owns its own `Mutable`s; two instances with the same id are two different instances → two different state cells. `with_update`'s `expanded: self.expanded.clone()` chain preserves the cell across structural rebuilds (the `Mutable` handle is itself an Arc so the cell survives even when the wrapping `ReactiveViewModel` is replaced).

**What landed (tree_item)**:

- `wrap_tree_item` in `crates/holon-frontend/src/mutable_tree.rs` now creates a fresh `Mutable::new(true)` (default expanded) and stores it on the `expanded: Option<Mutable<bool>>` field of the wrapping `ReactiveViewModel`.
- `frontends/gpui/src/render/builders/tree_item.rs::collapse_state` reads the `Mutable` via `node.expanded.as_ref().map(|m| m.get())` instead of `ctx.local.get_or_create_typed(CacheKey::Ephemeral("tree-collapse:{id}"), …)`.
- `collapse_chevron` takes a `Mutable<bool>` directly; the on_mouse_down handler does `m.set(!m.get())` and `window.refresh()`.
- `get_or_create_toggle` removed; `ToggleState` import dropped (the type is still used by `pie_menu.rs` so it stays in `entity_view_registry.rs`).

**`editable_text` — NOT migrating, design decision recorded**:

The earlier draft of this FU recommended migrating `editable_text` next. After tracing the layering, the migration doesn't make sense — and the analysis surfaces the actual boundary worth pinning:

- **VM owns plain-data state.** `Mutable<bool>` (`expanded`), `Mutable<HashMap<String, Value>>` (`props`), the per-row signal cell (`data`) — primitive types that every frontend can read/write the same handle. Cross-frontend by design.
- **GPUI's row-scoped `EntityCache` owns GPUI-specific entities.** `gpui::Entity<EditorView>`, `gpui::Entity<LiveBlockView>`, `gpui::Entity<ToggleState>` (still used by `pie_menu.rs`) — refcounted handles into the single-threaded GPUI app context. **Not Send+Sync.** Putting them on the shared `ReactiveViewModel` is a layering violation; TUI / Dioxus / headless drivers don't have GPUI types.

Three concrete reasons editable_text can't follow `tree_item`'s pattern:

1. `Entity<EditorView>` is a GPUI handle. The VM is in `holon-frontend` and stays frontend-agnostic. The shadow builder for `editable_text` already does the VM-level work it should: subscribes to the row's data signal (`editable_text.rs:24-41`) and derives the `content` prop on CDC writes. That's the clean half of the split — VM owns the data, GPUI owns the view.

2. The upstream `CacheKey::RenderEntity(row_id)` (in `frontends/gpui/src/views/reactive_shell.rs:776-799`) collapses two render_entity nodes with the same `row_id` to the **same** `RenderEntityView`. Their `editable_text` then necessarily resolves to the same `EditorView` because they share the same row-scoped `EntityCache`. Migrating editable_text alone wouldn't isolate them — the upstream `RenderEntity(row_id)` key is the deeper bottleneck.

3. "Same row_id + field → same EditorView" is the **correct** semantic for current use cases. Two editors on the same block.field should sync — that's what users expect.

**If a future feature needs same-id-twice editor isolation**: the change is to `RenderEntityView`'s cache key (make it position- or instance-aware), not `editable_text`. Defer until a concrete use case lands — at that point the test goal becomes specific enough to drive the design.

**Validation post-migration**:

- `cargo check --workspace`: clean.
- `cargo nextest run -p holon-gpui --lib entity_view_registry`: 5/5 pass.
- `cargo nextest run -p holon-integration-tests --test general_e2e_pbt`: both variants PASS (`general_e2e_pbt` 276.3s, `general_e2e_pbt_sql_only` 358.3s).
- `layout_invariants_hold_for_random_scenarios` proptest still flakes on the pre-existing `view_mode_switcher` reactivity scenario — unrelated to this change.

---

### FU-2 · Predicate Rhai constructors beyond `eq` — DONE

**Status**: LANDED. `crates/holon/src/render_dsl.rs:252-270` registers `eq`/`ne`/`gt`/`lt`/`gte`/`lte`/`is_not_null`/`and`/`or`/`not`/`always` via `register_binary_predicate` / `register_string_predicate` / `register_n_ary_predicate` (1..=6 arity overloads) and direct `register_fn` for `not`/`always`. 7 tests pass at `crates/holon/src/render_dsl.rs::tests::predicate_*` plus a full `tree(rules: [...])` round-trip at `rules_arg_round_trip_through_full_tree_call`.

**`var` deliberately not registered**: `var` is a Rhai reserved keyword — the tokenizer rejects it before dispatch even inside map literals. Users who want `Predicate::Var` write the verbose quoted-key form `#{"var": "field_name"}`; `predicate_var_via_quoted_verbose_form` test covers the fallback. Documented inline at render_dsl.rs:248-251.

**Validation** (run anytime):

```
cargo nextest run -p holon --lib render_dsl::tests::predicate
```

Expected: 7 passed (predicate_eq, predicate_ne_gt_lt_gte_lte, predicate_is_not_null, predicate_var_via_quoted_verbose_form, predicate_not, predicate_and_or_variadic, predicate_always).

---

### FU-3 · `Trigger::Click { modifiers: ClickModifiers }` refactor — LANDED (2026-05-08)

**Status**: SHIPPED. Future-proof: adding alt-click / cmd-click / ctrl-click semantics is now a one-line wiring in the shadow `selectable.rs` (`Trigger::Click { modifiers: ClickModifiers::alt() }`) plus a profile YAML entry. No GPUI changes, no API shape changes, no enum growth.

**What landed**:

- **`crates/holon-api/src/render_types.rs`**: new `ClickModifiers { shift, alt, cmd, ctrl }` struct (Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize) with `const` constructors `none()` / `shift()` / `alt()` / `cmd()` / `ctrl()` and `is_none()`. `Trigger::Click` and `Trigger::ShiftClick` collapsed into a single `Trigger::Click { modifiers: ClickModifiers }` variant. Added `OperationDescriptor::click_modifiers() -> Option<ClickModifiers>` (mirrors `key_chord()` for the keyboard case); `is_click_triggered()` / `is_shift_click_triggered()` now derive from `click_modifiers()` and remain backwards-compatible. `Trigger` now also derives `Hash` (KeyChord already did; ClickModifiers is bool-only).
- **`crates/holon-api/src/lib.rs`**: re-exports `ClickModifiers` alongside `Trigger`.
- **`crates/holon-frontend/src/shadow_builders/selectable.rs`**: `action:` emits `Trigger::Click { modifiers: ClickModifiers::none() }`; `shift_action:` emits `Trigger::Click { modifiers: ClickModifiers::shift() }`. Comment notes future modifier-bound aliases (`alt_action:`, `cmd_action:`) plug in here without growing the trigger enum.
- **`crates/holon-frontend/src/reactive_view_model.rs`**: new `ReactiveViewModel::intent_for_modifiers(ClickModifiers) -> Option<OperationIntent>` is the canonical accessor; `click_intent()` and `shift_click_intent()` become thin wrappers. Same `OperationIntent` shape as before — no caller-visible API change in the wrapper functions.
- **`frontends/gpui/src/render/builders/selectable.rs`**: rewritten to be modifier-agnostic. Pre-resolves a `HashMap<ClickModifiers, OperationIntent>` from `node.operations` at builder time; the on-mouse-down closure builds `ClickModifiers` from `event.modifiers` (mapping `event.modifiers.platform → cmd`, `control → ctrl`, etc.) and looks the intent up. `stop_propagation` fires on any non-empty modifier set so row-level click handlers don't double-fire. Adding alt-click is now zero GPUI changes.

**Validation**:

- `cargo check --workspace`: clean.
- `cargo nextest run -p holon-frontend --lib`: 200/200 pass (includes existing `click_intent_*` tests + new `intent_for_modifiers_disambiguates_shift_from_plain` test asserting plain/shift/alt routing).
- `cargo nextest run -p holon-gpui --lib`: 37/37 pass.
- `cargo nextest run -p holon-api --lib`: 117/118 pass (1 unrelated pre-existing failure at `render_eval::tests::test_state_display` — TODO state expects "warning" but renders "muted"; render_eval.rs unchanged in this session).
- `PROPTEST_CASES=1 cargo nextest run -p holon-integration-tests --test general_e2e_pbt general_e2e_pbt_sql_only`: PASS in 244s — confirms ClickBlock dispatch through `find_click_intent_in_region` → `OperationDescriptor::click_modifiers()` round-trip works end-to-end.
- `cargo check` in `frontends/holon-worker/`: clean.

**Caller-side migration is automatic**: profile YAMLs and DSL aliases didn't spell out `Trigger::Click` or `Trigger::ShiftClick` — they only wire `action:` / `shift_action:` on selectable, and the shadow builder constructs the trigger. So the refactor is invisible to userland configs.

---

### FU-4 · TUI/Dioxus modifier-click capability

**Cost**: 0.5 CCU
**Trigger**: any user that runs the TUI or Dioxus frontend hits a profile with a `shift_action` wiring. Today that's the `default` block_profile variant — so the trigger is "launch TUI."
**Why this isn't done**: deferred per user direction.

**Concretely**:

1. Add a `Frontend::supports_modifier_clicks() -> bool` on the frontend trait (default false). GPUI returns true; TUI/Dioxus return false.
2. At wiring-load time (or first dispatch), if a profile carries a `shift_action` and the frontend doesn't support it, emit a typed startup warning naming the profile + variant. **No silent no-op.**
3. Alternatively (cheaper), `unimplemented!("modifier+click on {frontend_name}")` panic at the dispatch site so it fails loud the first time the user actually shift+clicks.

**Validation**: smoke-launch TUI; assert no shift_action profile is silently dropped.

---

### FU-5 · Loro module exclusion plumbing (positive enforcement)

**Cost**: 0.5 CCU
**Trigger**: a future contributor wants to add a "tables to replicate" registry.
**Why this isn't done**: today there's no central "replicated tables" list. The compile-time test in `navigation/mod.rs::loro_exclusion_test` enforces exclusion negatively.

**Concretely**:

1. If/when a `LoroReplicatedTables` registry lands elsewhere, switch the test to assert that `navigation_history` and `navigation_cursor` are **not** in the registry (positive enforcement).
2. Until then: keep the negative-grep test.

**Validation**: replicated-tables registry tests stay green with the navigation entries excluded.

---

### FU-6 · Unified row pipeline + `rules:` on collection builders — LANDED (2026-05-08)

**Status**: SHIPPED for all five collection-shaped builders — `tree`, `outline`, `list`, `table`, `columns`, `board` — across **both static and streaming arms**. The "row iteration is duplicated across three sites" debt the user picked option (b) to pay down is paid: all per-row interpretation goes through `crate::row_pipeline`.

**Architectural win (the (b) path the user picked)**: previously three row-iteration sites duplicated the per-row work with inconsistent feature sets — `shared_tree_build` had `rules:` but skipped profile/ops resolution; the macro static arm did profile/ops but had no `rules:`; the streaming `signal_vec` driver did profile/ops via `row_render_context` but also had no `rules:`. New module `crates/holon-frontend/src/row_pipeline.rs` exposes:

- `parse_rules_arg(value) -> Vec<RuleSpec>` — moved out of `render_interpreter.rs` and made `pub`.
- `evaluate_rules_with_positional(rules, positional, row) -> HashMap<String, Value>` — replaces the old depth-only `evaluate_rules`. The positional map is opaque per-builder data merged into the predicate evaluation context so `eq("level", 0)` (tree), `eq("position", 0)` (list), `eq("status", "done")` (any row column) all work the same way.
- `apply_rules_and_interpret_with_ctx(...)` — lower-level: caller has already prepared a row context (used by streaming driver, which builds ctx via `row_render_context` + `parent_space` wiring).
- `apply_full_row_pipeline(...)` — higher-level: builds the row context (with profile/ops) + applies rules + interprets.

**Migrated sites**:

- `render_interpreter::shared_tree_build` (tree, outline static path) — uses `apply_rules_and_interpret_with_ctx` (preserves the existing "no profile/ops at tree-row level" behavior; profile/ops resolution happens downstream in `live_block`/`render_entity`).
- `widget_builder.rs:228-340` macro Collection-arm extraction — both static arm (uses `apply_full_row_pipeline` with `position`/`count`/`is_first`/`is_last`/`is_empty_collection` positional context) and streaming arm (passes `rules` through `CollectionData::Streaming`).
- `reactive_view::create_tree_driver` and `reactive_view::create_flat_driver` — both call `apply_rules_and_interpret_with_ctx` per VecDiff event, with empty positional (streaming has no count/is_last because rows arrive incrementally; column-only predicates fire as expected).
- `reactive_view::create_grouped_driver` (board) — applies rules per-card with positional `{lane: <lane title>}`, so `rules: [#{when: eq("lane", "Done"), override: #{role: "muted"}}]` fires per-lane.
- `CollectionData::Streaming { ..., rules }`, `CollectionConfig { ..., rules }`, `ReactiveViewInner::Collection { ..., rules }`, `ReactiveViewInner::Grouped { ..., rules }` — all carry rules through the construction chain.
- 6 builder call sites updated (`outline.rs`, `columns.rs`, `list.rs`, `table.rs`, `tree.rs`, `board.rs`) to extract rules from `ba.args.named.get("rules")` and pass to `streaming_collection` / `new_grouped`.

**Tests**:

- 4 new unit tests in `row_pipeline::tests`: `list_positional_rules_fire_on_first_and_last`, `tree_positional_rules_fire_on_level`, `rules_can_match_row_columns`, `later_rule_wins_per_override_key`.
- `cargo nextest run -p holon-frontend --lib`: 199/199 pass.
- `cargo check --workspace`: clean.
- 2-case PBT verification: 244s + 322s, both PASS (no regressions vs the FU-15 baseline).

**Streaming positional context**: `count`/`is_last`/`position` not available in streaming because the driver receives rows incrementally via VecDiff and the collection size shifts with each event. Position-aware rules in streaming would need an explicit position-tracker (re-evaluate visible items on count change). Documented but not implemented. For position-aware behavior in production today, use the static arm (eager `ctx.data_rows` interpretation). Board's lane positional IS available because the lane title is determined at bucket time, before the per-card interpret.

**Concretely** (per builder):

1. Identify per-row context fields meaningful for that builder. Suggested:
   - `list`: `position`, `count`, `is_first`, `is_last`, `is_empty_collection`.
   - `board`: `lane`, `column_count`, `position_in_lane`.
   - `chunks_by`: `chunk_index`, `chunk_size`, `position_in_chunk`.
   - `outline`: same as tree (`level`, `is_first_child`, `is_last_child`).
2. In the shared builder body, mirror `shared_tree_build`'s rule-eval block: read `rules:` arg, evaluate per row against the per-row context, merge into `ctx.flags` and (if applicable) into the row wrapper's props.
3. Update each builder's return type to carry overrides if the wrapper needs them.

**Validation**: per-builder unit test asserting a level/lane/position predicate fires correctly on the matching rows.

---

### FU-7 · Drop flags in resolved live_block body (defensive)

**Cost**: 0.3 CCU
**Trigger**: a profile variant with nested `live_block(other_id)` calls — none today.
**Why this isn't done**: Phase D + cleanup made `render_entity` clear flags on the variant body interpretation, which closes the practical leak path. The remaining theoretical leak is `live_block(A, #{role}) → A's render contains live_block(B) → B's render_entity sees role`. Today's `block_profile` doesn't nest live_block in its variants, so this can't fire.

**Concretely**:

1. In `shared_live_block_build` (`crates/holon-frontend/src/render_interpreter.rs:~540`), call `child_ctx.without_flags()` AFTER `pick_active_variant` is reached but BEFORE descending into nested live_block calls.
2. The cleanest place to do this is inside `render_entity` (already done — confirms the boundary). For belt-and-braces, also clear at `live_block` builder boundary.

**Risk**: clearing too eagerly means `live_block(id, #{role})` flag doesn't reach a `render_entity` inside id's resolved render. Verify on a fixture before committing.

**Validation**: PBT scenario with `live_block(A, #{role: "page_title"}) → A.render = render_entity()` → page_title variant fires; same scenario with `A.render = column(live_block(B))` → B does NOT see role flag.

---

### FU-8 · Cucumber feature for shift+click → right sidebar flow

**Cost**: 1 CCU
**Trigger**: end-to-end behavioral test of the LogSeq feature (today only unit + workspace check coverage).
**Why this isn't done**: user said cucumber isn't currently in active use.

**Concretely**:

1. Add `crates/holon-integration-tests/features/right_sidebar_pinning.feature`:

   ```gherkin
   Scenario: shift+click pins a block to the right sidebar
     Given a block "B1" exists
     When I shift+click on the bullet of "B1"
     Then the right sidebar shows "B1" as a page_title row

   Scenario: re-pinning moves to top
     ...

   Scenario: close removes from sidebar
     ...
   ```

2. Wire the steps via existing `cucumber.rs` step definitions. New steps needed: shift_click_block, assert_sidebar_titles, click_close_button.

**Validation**: `cargo nextest run -p holon-integration-tests --test cucumber right_sidebar_pinning` passes.

---

### FU-9 · PBT regression: pinned blocks ↔ open navigation_history rows invariant

**Cost**: 1 CCU
**Trigger**: PBT regression coverage of the new pin semantics.
**Why this isn't done**: Phase C/D PBT alignment landed only the semantic flip on `expected_focus_root_ids`. New invariant + new transitions are deferred.

**Concretely**:

1. New PBT transition `PinBlock { region, block_id }` mapping to `focus_pin` op + reference-state mirror (move-to-top dedup or insert). Add to the transition-gen weights.
2. New PBT transition `UnpinBlock { history_id }` mapping to `close` op + reference-state mirror.
3. New invariant in `sut.rs` (Invariant 9 or wherever the next slot is): the set of open `navigation_history` rows for a region equals the set of `focus_roots` rows for that region (deduped by block_id under move-to-top).
4. Update existing transitions that touch navigation history (`navigate_focus.rs:74`, `navigate_back.rs`, `navigate_forward.rs`, `navigate_home.rs`, etc.) to predict the `closed_at` UPDATE in the Replace path.

**Validation**: `cargo nextest run -p holon-integration-tests general_e2e_pbt` runs ≥ 50 cases without divergence.

---

### FU-10 · Land first-launch on `block:journals` — LANDED (2026-05-08, native + browser)

**Status**: SHIPPED on both native and browser worker. The today's-journal block creation is intentionally *not* part of FU-10 per user direction — `block:journals` itself can later display all journals and create today's entry as a page-level concern, separate from the seed.

**Browser worker change (2026-05-08, this session)** (`frontends/holon-worker/src/seed.rs`):

Added three Journals.org-equivalent blocks to the inline seed table (the org parser is unavailable on wasm32, so the parser's output has to be hand-bundled):

- `block:journals` (parent: `sentinel:no_parent`, name: "Journals", sort_key `d0`) — the page itself, sibling of `block:welcome`.
- `block:journals::src::0` (parent: `block:journals`) — PRQL listing children with `name != null`, matching `assets/default/Journals.org:2-7`.
- `block:journals::render::0` (parent: `block:journals`) — render expression: `list(#{sortkey: "-name", item_template: selectable(row(icon("calendar"), ..., text(col("name"))), #{action: navigation_focus(...)})})`.

Replaced the prior raw `INSERT INTO navigation_history (region, block_id) SELECT 'main', 'block:journals' WHERE NOT EXISTS ...` with `engine.execute_operation(&EntityName::from("navigation"), "focus", { region: Main, block_id: "block:journals" })` — same atomic close-prior + insert + cursor-update path the native version uses. The fresh-DB guard (`if !existing.is_empty() { return Ok(()); }` at line 40) ensures this only runs once.

The "Journal Auto-Create" subtree (`::trigger::0` + `::action::0`) is omitted — auto-create is a page-level concern that can be invoked from the rendered list once entries exist.

**Why the prior browser seed was actually broken**: the existing raw `INSERT INTO navigation_history` pointed at `block:journals`, but the block itself didn't exist in the inline seed. The main panel GQL `MATCH (fr:focus_root), (root:block) WHERE root.id = fr.root_id RETURN d` requires `block:journals` to exist as a block row; without it the JOIN yielded zero rows and the panel rendered empty. Adding the three blocks above + switching to `navigation.focus` makes the main panel render the journals list (empty initially) on first launch.

**Pre-existing collateral fix (same session)** (`frontends/holon-worker/src/lib.rs`):

Three `service.execute_operation(&entity_name, &op, params)` call sites passed `&String` where the upstream API now expects `&EntityName`. Wrapped them in `&EntityName::from(s.as_str())` and added `EntityName` to the `holon_api` import line. These errors were latent fallout from the `bd67177d fix: PBT panics + Turso IVM bug capture + replay tooling` parent commit's signature update; the worker wasn't part of `cargo check --workspace` so they hid until the worker was built directly.

**Verification**:

- `cargo check` in `frontends/holon-worker/`: clean (1 unrelated dead_code warning on `DbState.io`).
- `cargo check --workspace`: clean.
- Cargo.lock refresh: routine fluxdi → ferrous-di dep migration sync from upstream, not introduced by this change.

**Native change (already landed before this session)**:

**Native change** (`crates/holon-frontend/src/lib.rs::seed_default_layout`):

```rust
// Reached only on the fresh-DB path (after the early return).
let mut nav_params = HashMap::new();
nav_params.insert("region", Value::from(Region::Main));
nav_params.insert("block_id", Value::String(EntityUri::block("journals").as_str().to_string()));
engine.execute_operation(&EntityName::from("navigation"), "focus", nav_params).await?;
```

Goes through the navigation provider's `focus()` op (rather than raw SQL), which atomically does the close-prior + insert + cursor-update. So `navigation_history`, `navigation_cursor`, `focus_roots`, and `current_focus` all converge on `block:journals` for `region='main'` on first render.

**PBT mirror** (`crates/holon-integration-tests/src/pbt/transitions/start_app.rs::apply_to_ref`): pushes the same `(Region::Main, EntityUri::block("journals"))` pair into `state.navigation_history[Main]` + `state.open_pins[Main]` after the `seed_profile` setup, so reference state matches the SUT post-StartApp.

**Why the earlier reverted attempt failed (now fixed)**:

**Status (2026-05-08)**: attempted a minimal native subset (`seed_default_layout` INSERT into `navigation_history` pointing at `block:journals`) and **reverted**. The change broke the PBT (`general_e2e_pbt_sql_only` panicked at `sut.rs:4060` after StartApp) for two layered reasons:

1. **PBT ref-state divergence**: `start_app.rs::apply_to_ref` doesn't predict the new INSERT. Easy fix: mirror it by populating `state.navigation_history[Main]` + `state.open_pins[Main]` for `block:journals`. Tried this — pushed the panic to `sut.rs:3946` ("should have focus on 'block:journals' but not found in DB") instead.

2. **navigation_cursor not updated**: An open `navigation_history` row alone doesn't make `current_focus` matview return anything — the cursor must point at the row. Production `focus()` (`crates/holon/src/navigation/provider.rs:41-128`) does INSERT *and* `update_history.sql` UPDATEs the cursor. The seed only did the INSERT. So `current_focus` stayed empty and `inv-7` panicked.

**Still deferred**:

- **Browser worker** (`frontends/holon-worker/src/seed.rs`): doesn't have `block:journals` because the org parser is unavailable on wasm32-unknown-unknown. To land on Journals, the browser seed would need to also bundle `Journals.org`'s blocks (overview + src + render + trigger + action) inline. Today users land on `block:welcome` — works fine but lacks LogSeq feel.
- **Today's journal pre-creation**: `Journals.org` ships with a holon_sql trigger (`SELECT date('now', 'localtime') as name`) and an action (`block.create(#{parent_id: "block:journals", name: col("name")})`). On first launch the user sees the empty Journals overview and must manually fire the action to create today's entry. Auto-firing it during seed would require either:
  1. The seed running an action — which means wiring the action infrastructure into the seed path (heavier than spec implied), or
  2. Using `chrono` (workspace dep) to format today's date, then SQL-inserting `block:journal-{YYYY-MM-DD}` directly. Workspace-level: easy. Browser worker: needs adding `chrono` to its out-of-workspace `Cargo.toml`.

**Verification (2026-05-08)**:

- `cargo check -p holon-frontend` + `cargo check -p holon-integration-tests --tests`: clean.
- `PROPTEST_CASES=1 cargo nextest run -p holon-integration-tests --test general_e2e_pbt general_e2e_pbt_sql_only`: PASS in 217s. Reference state's `current_focus(Main)` and `expected_focus_root_ids(Main)` align with the SUT's matview state after StartApp without any divergence.

**Remaining for later**:

- **Today's-journal entry creation**: page-level concern handled by `block:journals` itself (it ships with a `holon_sql` trigger + `block.create` action that can fire on demand). Out of scope for the seed. Browser worker omits the trigger/action subtree from the inline bundle for the same reason.

---

### FU-11 · Ranked retrofits / cleanups (small but worth doing)

**FU-11a** · Drop the `frontends/waterui` workaround in root `Cargo.toml`. It was excluded from the workspace `members` because the worktree-create hook's `cargo check --workspace` blocked on a Swift compile error in `waterkit-screen` (CGWindowListCreateImage obsolete in macOS 15+ SDK). When/if the upstream waterui pin updates to use ScreenCaptureKit, restore `frontends/waterui` to `members`. Cost: 0.1 CCU.

**FU-11b** · Replace the synthetic-sort-key trick in earlier SQL drafts (now obsolete since GQL replaced it) — already done, no action needed. Recorded for context: future similar use cases (date-based ordering of synthetic root rows in tree builders) can use GQL → MCP-compile to verify the generated SQL is what you expected.

**FU-11c** · `OperationProvider::execute_operation` for `close` currently special-cases the op_name early (skips region extraction). If more region-less ops appear, refactor into a typed dispatch table. Today's two-region-less-op shape (`close`) doesn't justify it. Cost: 0.5 CCU when the second op arrives.

**FU-11d** · Periodic compile-check of out-of-workspace frontends. The `cargo check --workspace` gate skips `frontends/{waterui, dioxus, dioxus-web, holon-worker}` and `crates/holon-architecture-tests` (each excluded for a real reason — cocoa conflicts, wasm-only, naga/codespan, etc.). When upstream API signatures change (e.g. `execute_operation(entity: &str)` → `&EntityName`), in-workspace call sites get fixed by the migration; out-of-workspace ones rot silently until someone tries to build them. The 2026-05-08 FU-10 browser parity session hit this: `frontends/holon-worker/src/lib.rs` had three `&String → &EntityName` errors latent from the parent commit's signature update, and `frontends/dioxus/src/operations.rs:17` still has the same drift (not fixed because the cocoa version conflict prevents building dioxus from this worktree). When/if a `frontends/{...}` toolchain rotation lands (i.e. a worktree that can build dioxus alongside gpui), sweep the excluded crates with `cargo check` and fix the API drift in one pass. Cost: 0.5 CCU per rotation. Trigger: any commit that breaks `cargo check --workspace` would have broken excluded frontends silently — when adding follow-ups about API churn, also note "verify out-of-workspace frontends".

---

## Architectural touchpoints worth knowing

- **Flags vs. ctx vs. variants**: render-context flags (`role`, `view_mode`, `embed_depth`) participate in `pick_active_variant` evaluation alongside row columns and available_space. They're set via `live_block(id, #{...})` second-arg or via tree builder `rules:`. They're cleared in `render_entity` after variant dispatch — so flags don't leak into the variant body's render. **This is the load-bearing scope rule.**
- **`focus_roots` matview shape**: post-Phase C, it's a flat `WHERE closed_at IS NULL` SELECT (no JOIN). Turso CDC for closed_at predicate flips works (verified by the existing PBT runs not breaking; formal preflight repro PF1 from the plan was deferred since the new shape avoids the chained-matview landmine class).
- **`focus_pin` move-to-top dedup**: SELECT-first to detect existing open pin, then UPDATE timestamp or INSERT. The previous version was a lurking double-insert bug; if you see duplicate rows in production for `(region, block_id)` with `closed_at IS NULL`, it's a regression of FU-related code.
- **Rules: produces both ctx.flags AND tree_item props**: same merged map flows to two consumers without splitting. Keys consumed by tree_item chrome (`show_bullet`, `show_chevron`) are read off props; keys consumed by `pick_active_variant` (`role`, `view_mode`) come from ctx.flags. Same key in both is harmless duplication.
- **GQL vs hand-written SQL**: prefer GQL for hierarchical / focus_roots-anchored queries. `holon-direct` MCP `compile_query` is the verification tool: paste GQL → see compiled SQL → confirm shape matches expectations before committing to org file or schema. Pattern proven via FU's right sidebar query.

---

## Recommended next-session order

(Updated 2026-05-08 after FU-9 + FU-15 + FU-16 closure.)

**Pin/unpin + ref-state invariants are stable at 50 cases. Row pipeline is unified. Native + browser first-launch land on Journals overview. focus_roots NULL handling is clean end-to-end. Trigger refactor is future-proof for arbitrary modifier-click combinations.** The remaining items are pure feature/polish work, mostly gated on triggers that haven't arrived.

1. **FU-8** (Cucumber feature for shift+click → right sidebar) — feasible now that the underlying flow is stable; cucumber has no other regressions.
2. **FU-7 / FU-11x** — defer until concrete trigger.

**Items closed since the original Phase D landing**: FU-1 (partial w/ rationale), FU-2 (already done), FU-3 (ClickModifiers struct + GPUI modifier-agnostic dispatch), FU-6 (row pipeline + rules: on all 5 collection builders, both arms), FU-9 (≥50 cases pass), FU-10 native + browser (journals navigation seed both arches), FU-12 (sql_statements `;`-in-comments fix), FU-13 (LiveData<FocusRoot> NULL handling: matview-level filter + chained-matview CDC verified 1:1, test-side filter removed), FU-14 (intent resolution via snapshot_resolved), FU-15 (region-scoped intent lookup), FU-16 (subsumed by FU-15).

---

## How to resume

```bash
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/logseq-right-sidebar
jj log -r 'ancestors(@) & ~empty()' --limit 6 --no-graph
# you should see: ytouomrx Phase D + post-review cleanup → zknyxxqs C → umsxqomm B → pvryopot A
cargo check --workspace        # baseline: clean
cargo nextest run -p holon-gpui --lib entity_view_registry  # 5/5 pass
cargo nextest run -p holon --lib navigation::loro_exclusion_test  # 1/1 pass
```

Pick a follow-up from the list above. Each one is described with concrete file:line pointers and acceptance criteria.

---

## Mid-session handoff — FU-9 in progress (2026-05-07, pre-reboot)

Started FU-9 (PBT regression for pin/unpin semantics). Stopped before any code changes — only finished investigation. **No files modified.** Resume by re-reading this section and proceeding with the design below.

### What was learned

**Schema (verified):**

- `crates/holon/sql/schema/navigation.sql`: `navigation_history(id, region, block_id NULL, timestamp, closed_at NULL)` + `navigation_cursor(region PK, history_id FK)`.
- `crates/holon/sql/schema/matview_focus_roots.sql`: `SELECT region, block_id AS root_id, timestamp AS added_ts, id AS history_id FROM navigation_history WHERE closed_at IS NULL`. Pure projection of open rows — no JOIN, no dedup-in-SQL (move-to-top is enforced by `update_pin_timestamp.sql` UPDATE-instead-of-INSERT in `focus_pin`).

**Provider operations (all in `crates/holon/src/navigation/provider.rs`):**

- `focus(region, block_id)` (L41-128): clear_forward_history (DELETE id > current_id), close_open_in_region (UPDATE closed_at on all open in region), INSERT new open row, update cursor. Result: exactly one open row per region in `navigation_history` for Main.
- `focus_pin(region, block_id)` (L142-188): SELECT existing open `(region, block_id, closed_at IS NULL)`. If exists → `update_pin_timestamp.sql` (refresh `timestamp`); else → `insert_history.sql`. **Cursor untouched** — pins are not part of back/forward.
- `close(history_id)` (L192-207): UPDATE closed_at on one specific row (sidebar X). Region-less op, dispatched specially in `execute_operation` at L491-497.
- `go_back` / `go_forward` (L210-301): just walk the cursor. Don't touch closed_at.
- `go_home` (L304-306): aliased to `focus(region, None)` — same close-prior + insert-NULL semantics.

**Operation descriptors (L368-474):** `focus`, `focus_pin`, `close`, `go_back`, `go_forward`, `go_home` all registered. `close` is the only one without a `region` param — handled before region extraction in `execute_operation`.

**SUT/PBT integration:**

- PBT runs `Full` / `SqlOnly` / `CrossExecutor` variants (`tests/general_e2e_pbt.rs`) — all backed by `ReactiveEngineDriver` (headless), set up at `sut.rs:2624`. No GpuiUserDriver here.
- `send_leader_chord` at `sut.rs:3004-3033` already shows the precedent: real-input drivers go through `send_raw_keystroke`; headless falls back to `synthetic_dispatch("navigation", nav_op, params)`. Architecture rule (`archlint/smells/focus.toml`): the smell is `execute_op("navigation", ...)`, NOT `synthetic_dispatch`. So `synthetic_dispatch("navigation", "focus_pin", ...)` and `synthetic_dispatch("navigation", "close", ...)` are valid for headless PBT.
- Existing `expected_focus_root_ids(region)` at `reference_state.rs:1519-1527` returns just `current_focus(region)` (single block). **This is the key piece to refactor** — the new model needs to return all OPEN rows in the region.
- Existing inv-focus-roots check at `sut.rs:3896-3990+`: compares `expected_focus_root_ids` against the LiveData<FocusRoot> mirror, with truth-check fallback querying `focus_roots` matview directly. **This invariant already exists** — once `expected_focus_root_ids` is updated, it'll pick up Pin/Unpin behavior automatically. Devlog FU-9 §3 ("new invariant") may already be covered by 3896-3990; the work is in the reference model side.

**Existing reference state model (`reference_state.rs:520-554`):**

```rust
pub struct NavigationHistory {
    pub entries: Vec<Option<EntityUri>>,  // Vec of focused blocks; None = home
    pub cursor: usize,                     // index into entries
}
```

This is a back/forward stack with no notion of closed_at. The current `expected_focus_root_ids` cheats by returning current cursor. Works for Main (always 1 open row), wrong for any pin behavior.

**SUT-side pattern for navigation transitions:**

- `apply_navigate_focus` (sut.rs:722-756) drives via `driver.click_entity(resolved_id, "left_sidebar")` — clicks the LeftSidebar entry, which dispatches the bound `navigation.focus` intent.
- `apply_navigate_back/forward/home` use `send_leader_chord(...)`.
- For pin: there's no leader chord (it's shift+click). Headless ReactiveEngineDriver has no shift-click pipeline. Direct `synthetic_dispatch` is the right fit, paralleling the leader-chord fallback.

### Design for resumption

**Step 1: extend reference state model.** In `reference_state.rs`, add a new field to `ReferenceState`:

```rust
/// Open navigation_history rows per region. Mirrors `closed_at IS NULL` rows.
/// NavigateFocus closes prior in region + inserts new; PinBlock dedups by (region, block_id);
/// UnpinBlock removes by history_id. NavigateBack/Forward don't touch this.
pub open_pins: HashMap<Region, Vec<OpenPinEntry>>,
pub next_history_id: i64,  // mirrors AUTOINCREMENT
```

With:

```rust
pub struct OpenPinEntry {
    pub history_id: i64,
    pub block_id: Option<EntityUri>,  // None = home (NULL block_id; not in focus_roots)
    pub added_ts_logical: u64,         // monotonic counter for move-to-top sort
}
```

Initialize in `ReferenceState::new` (line 570-ish): `open_pins: HashMap::new(), next_history_id: 1`.

**Step 2: rewrite `expected_focus_root_ids`** (reference_state.rs:1519-1527) to compute open rows:

```rust
pub fn expected_focus_root_ids(&self, region: Region) -> BTreeSet<EntityUri> {
    self.open_pins
        .get(&region)
        .map(|pins| pins.iter().filter_map(|p| p.block_id.clone()).collect())
        .unwrap_or_default()
}
```

**Step 3: update existing transitions to maintain `open_pins`:**

- `navigate_focus.rs:apply_to_ref`: after the existing entries+cursor update, also clear `state.open_pins[region]` and push `OpenPinEntry { history_id: state.next_history_id, block_id: Some(self.block_id), added_ts_logical: ... }`. Bump `next_history_id`.
- `navigate_home.rs:apply_to_ref`: clear `open_pins[region]` and push `OpenPinEntry { ..., block_id: None, ... }`.
- `click_block.rs:apply_to_ref` (LeftSidebar branch at L102-129): same as navigate_focus — close + push.
- `navigate_back.rs` / `navigate_forward.rs`: **no change to open_pins** (cursor moves, but open status unchanged).

**Step 4: add `PinBlock` + `UnpinBlock` transitions:**

- File: `crates/holon-integration-tests/src/pbt/transitions/pin_block.rs`. Generator picks `region: Region::RightSidebar` (the only place pin is wired in the default layout) and a `block_id` from `state.focusable_rendered_block_ids(Region::Main)` or similar (a block with a bullet that can be shift-clicked). `apply_to_ref`: SELECT-existing-pin in `open_pins[RightSidebar]`; if found, bump `added_ts_logical` (move-to-top); else push new with new history_id. `apply_to_sut`: route via new `apply_pin_block(region, block_id)` on `SutHandle` → `synthetic_dispatch("navigation", "focus_pin", ...)`.
- File: `crates/holon-integration-tests/src/pbt/transitions/unpin_block.rs`. Generator: pick a history_id from `state.open_pins[RightSidebar]` (precondition: at least one open pin exists). `apply_to_ref`: remove by history_id. `apply_to_sut`: → `synthetic_dispatch("navigation", "close", #{history_id})`.
- `mod.rs`: add the `mod pin_block; mod unpin_block;` lines + `pub use` + register in `declare_e2e_transitions!`. Arch test enforces file presence.
- `transition_dispatch.rs`: add `apply_pin_block` and `apply_unpin_block` methods to `SutHandle` trait. Implement on `E2ESut<V>` in `sut.rs` near `apply_navigate_focus`.

**Step 5: existing `expected_sql` budgets in navigate_focus / navigate_home likely need adjustment.** Phase D §"Open follow-ups" hints existing nav transitions should "predict the closed_at UPDATE in the Replace path". Check `transition_budgets.rs` for the `NAV_DML_READS` constant; the `close_open_in_region` UPDATE adds 1 write. Run with `--features otel-testing` to hit those code paths. Without otel-testing they're dead code so probably skippable for first pass.

**Step 6: validate.** `cargo nextest run -p holon-integration-tests general_e2e_pbt` (8 cases × 3-20 transition-len). Devlog target: ≥50 cases.

### Carried-over open issues

- `transition_dispatch.rs:170` defines a top-level `navigate_back` method on `SutHandle` (not the typical `apply_*` form). Pattern asymmetry — the existing close/back pattern doesn't use `apply_navigate_back`; the SutHandle has a bare `navigate_back`. Match the existing convention when adding methods.
- `synthetic_dispatch` requires a `Value` for `history_id` which is typed as `Number` in the operation descriptor (provider.rs:419) but read as `as_i64()` (provider.rs:494). `Value::Integer(history_id)` is the right shape (matches `update_history.rs` style at `provider.rs:196`).
- The PBT generator should weight Pin/Unpin lower than common transitions to avoid drowning out other invariant checks. Suggest `(2, ...)` for PinBlock and `(2, ...)` for UnpinBlock — same magnitude as NavigateFocus's `(3, ...)`.
- After the reboot, run `cargo check --workspace` first to confirm nothing else has shifted.

### Files NOT touched yet (next session can resume freely)

- `crates/holon-integration-tests/src/pbt/reference_state.rs`
- `crates/holon-integration-tests/src/pbt/transitions/mod.rs`
- `crates/holon-integration-tests/src/pbt/transitions/navigate_*.rs`
- `crates/holon-integration-tests/src/pbt/transitions/click_block.rs`
- `crates/holon-integration-tests/src/pbt/transition_dispatch.rs`
- `crates/holon-integration-tests/src/pbt/sut.rs`

No new files created; no edits made; no commits.

---

## FU-9 implementation handoff (2026-05-07, evening — infrastructure landed)

**Status**: FU-9 infrastructure complete. PBT exposes pre-existing baseline bugs unrelated to pin/unpin semantics; those are documented as new follow-ups (FU-12, FU-13 below).

### What landed

1. **Reference state model** (`crates/holon-integration-tests/src/pbt/reference_state.rs`):
   - New struct `OpenPinEntry { history_id, block_id: Option<EntityUri>, added_ts_logical }` (ref_state.rs:558).
   - New fields on `ReferenceState`: `open_pins: HashMap<Region, Vec<OpenPinEntry>>`, `next_history_id: i64`, `next_pin_ts: u64`.
   - `expected_focus_root_ids(region)` rewritten to iterate `open_pins[region]`, filter out home rows (block_id None). Pure reference: no SUT-side dependency.

2. **Existing transitions updated** to maintain `open_pins`:
   - `navigate_focus.rs`: clears region's pins, pushes new with fresh history_id (mirrors provider.rs `focus(region, Some(block_id))`).
   - `navigate_home.rs`: clears region's pins, pushes home row (block_id=None) with fresh history_id (mirrors `focus(region, None)`).
   - `click_block.rs` (LeftSidebar branch only): same as navigate_focus.
   - `navigate_back.rs` / `navigate_forward.rs`: untouched — cursor moves but `closed_at` unchanged in production.

3. **New transitions** (sibling files registered in `transitions/mod.rs`):
   - `pin_block.rs` (PinBlock): `weighted_generator` picks Region::RightSidebar + a `focusable_rendered_block_ids(Region::Main)` block. `apply_to_ref` does move-to-top dedup; `apply_to_sut` calls `synthetic_dispatch("navigation", "focus_pin", ...)`.
   - `unpin_block.rs` (UnpinBlock): generator picks an open pin's `history_id`. `apply_to_ref` removes by history_id; `apply_to_sut` calls `synthetic_dispatch("navigation", "close", ...)`.
   - SutHandle gained `apply_pin_block` and `apply_unpin_block` (transition_dispatch.rs:386, sut.rs:2543).
   - Both registered in `declare_e2e_transitions!` (mod.rs:194).

4. **Validation status**:
   - `cargo check --workspace`: clean.
   - `cargo nextest run -p holon-integration-tests --lib pbt::transitions::arch_tests`: 2/2 pass (every variant has a file, every file registered).
   - `cargo nextest run -p holon --lib navigation`: 4/4 pass (incl. `loro_paths_do_not_reference_navigation_tables` regression).
   - `cargo nextest run -p holon-integration-tests --test general_e2e_pbt`: blocked by FU-12 / FU-13 below.

### Pre-existing baseline bugs uncovered (NOT introduced by FU-9)

These were latent because earlier sessions didn't run `general_e2e_pbt` end-to-end on the Phase D worktree.

#### FU-12 · `sql_statements` splits on `;` inside SQL line comments

**Cost**: 0.2 CCU. **Status**: FIXED in this session. **Trigger**: any schema SQL file with `;` in a `--` comment.

`crates/holon/src/storage/mod.rs:36` `sql_statements()` was a one-liner `content.split(';').map(...).filter(...)`. Phase C/D added explanatory `--` comments to `crates/holon/sql/schema/navigation.sql` (the `closed_at TEXT NULL` block, lines 6-10) that contain inline `;` (e.g. `"...focus_roots matview); set ="`). The split truncated the surrounding `CREATE TABLE` mid-statement, and Turso parsed it as "incomplete input".

Fix: rewrote `sql_statements` as a stateful char-by-char split that suppresses `;` while inside `--` line comments. Returns `impl Iterator<Item = &str>` (same signature). Test: `cargo nextest run -p holon --lib navigation` covers schema init.

#### FU-13 · `LiveData<FocusRoot>` panics on home rows (NULL `root_id`)

**Cost**: 0.3 CCU (test side) + upstream Turso fix needed for matview-level filter. **Status**: TEST-SIDE FIXED; UPSTREAM BUG CONFIRMED + FILED (2026-05-08). **Trigger**: `NavigateHome` runs (insert NULL block_id row) while `LiveData<FocusRoot>` is subscribed.

`focus_roots` matview includes home rows (`closed_at IS NULL` only — no block_id filter), projecting `block_id AS root_id`. NULL block_id propagates to NULL root_id. Production GQL filters NULL roots via `JOIN block ON root.id = fr.root_id`, but the PBT's `LiveData<FocusRoot>` watcher (`sut.rs:3195`) issued raw `SELECT region, root_id FROM focus_roots` and panicked at the `id_fn` (live_data.rs:163) when reading NULL root_id.

Test-side fix landed: `LiveData<FocusRoot>` watch SQL now reads `WHERE root_id IS NOT NULL` (sut.rs:3207). The matview itself is left unchanged (production GQL handles it).

**Upstream bug verified (2026-05-08)**: `WHERE col IS NOT NULL` in a matview projection with column aliases (`SELECT block_id AS root_id, ...`) did NOT filter NULL rows. Confirmed on `tursodb` CLI v0.6.0-pre.23. Two failure modes:

1. INSERT with NULL block_id → row appears in matview despite `WHERE block_id IS NOT NULL`.
2. UPDATE block_id value → NULL on existing matview row → row stays instead of being removed.

Control: same `IS NOT NULL` matview without aliases worked correctly. The bug needed the alias-shaped projection.

Filed upstream at `bigdata/turso/bugs/holon_focus_roots_null_filter_2026-05-08.{md,sql}`. Holon-side repro at `crates/holon/examples/turso_ivm_focus_roots_null_filter.rs`.

**Upstream fix landed (2026-05-08)**: nightscape@holon commit `aff40a84` fixes both failure modes. Verified by re-running the Rust repro on the bumped pin: Modes 1, 2, 3 all PASS. (Mode 4 — minimal `IS NOT NULL` with NULL-first INSERT via Rust API path — still fails on the holon fork; tracked separately, doesn't affect production.)

**Holon changes landed**:

- `crates/holon/sql/schema/matview_focus_roots.sql` now includes `AND block_id IS NOT NULL` in the WHERE. Home rows live only in `navigation_history`; `focus_roots` only contains blocks the panel can actually render. Verified by `count(*)` and CDC `set_change_callback` observation: Turso's matview-level filtering is correct end-to-end on `aff40a84`.
- `crates/holon-integration-tests/src/pbt/sut.rs::live_focus_roots`: chained-matview CDC verification at `crates/holon/examples/turso_ivm_chained_matview_null_cdc.rs` rules out Turso as the source of the residual panic. The 3-table repro shows that:
  - Inner matview (`focus_roots`) emits CDC events ONLY for rows that pass the WHERE filter — NULL block_id rows produce zero CDC.
  - Outer matview (`watch_outer = SELECT region, root_id FROM focus_roots`) propagates CDC events 1:1 from the inner matview — same count, same payloads, never with NULL root_id.
  - `event.columns` for both matviews matches the projection (inner: `region, root_id, added_ts, history_id`; outer: `region, root_id`).
  - `change.parse_record()` returns the correct non-null typed values for every event.

  The original "panic on test-side filter removal" diagnosis was a stale-`Cargo.lock` race (the worktree's lock had reverted to `290fbb4f` — pre-fix — due to a `cargo update` cwd confusion when I was in `/Users/martin/Workspaces/bigdata/turso` instead of the worktree). With the confirmed-`aff40a84` pin AND the matview-level filter, the test-side filter is genuinely redundant. **Test-side filter removed** (sut.rs:3210 now reads `SELECT region, root_id FROM focus_roots`). PBT 1-case PASSED in 210s; 2-case sweep verifying.

PBT verification: 1-case `general_e2e_pbt_sql_only` PASSED in 322s; 2-case sweep (`general_e2e_pbt` + `_sql_only`) both PASS on `aff40a84` (254s + 373s). No regressions from the Turso bump or the matview filter addition. The test-side filter retention is the right call until the holon-side broadcast translation issue is investigated separately.

#### FU-14 · `current_focus` invariant timing on NavigateFocus

**Cost**: 1 CCU. **Status**: OPEN — newly exposed.

After NavigateFocus in PBT, the invariant 7 check at `sut.rs:3946` panics with "Region 'main' should have focus on 'block:ref-doc-0' but not found in DB" — meaning `current_focus` matview returns empty for region=main when ref_state expects a focused block. Likely Turso IVM CDC lag for the JOIN matview (`navigation_cursor JOIN navigation_history`). The existing inv-focus-roots invariant has a truth-check + WARN downgrade pattern (sut.rs:3924-3967); the inv-7 cursor check at 3946 doesn't have this — it panics directly.

Pattern to replicate from inv-focus-roots:

1. After mismatch, re-query `current_focus` directly (one round trip) to distinguish CDC-lag vs real disagreement.
2. If matview agrees with ref → emit `[invN WARN] CDC lag` and continue.
3. If matview disagrees → real bug, panic.

**Diagnostic SQL bug also fixed**: `dump_nav_tables` was probing `SELECT region, block_id, root_id FROM focus_roots` but focus_roots only projects `root_id` (renamed). Updated to `SELECT region, root_id, history_id`.

### Still open from original FU-9 list

- **FU-9 Step 5 (budget adjustments)**: skipped per design — only matters for `--features otel-testing` runs. NavigateFocus / NavigateHome `expected_sql` should add 1 write for the `closed_at` UPDATE in the Replace path (transition_budgets.rs `NAV_DML_READS`). Picked up if anyone enables otel-testing.
- **FU-9 Step 6 (≥50 cases validation)**: blocked by FU-14. Once FU-14 lands the truth-check downgrade, run `cargo nextest run -p holon-integration-tests --test general_e2e_pbt` to confirm pin/unpin transitions exercise the `expected_focus_root_ids` invariant without false-positive panics.

### Files modified this session

- `crates/holon-integration-tests/src/pbt/reference_state.rs` (open_pins + OpenPinEntry)
- `crates/holon-integration-tests/src/pbt/transitions/navigate_focus.rs` (open_pins maintenance)
- `crates/holon-integration-tests/src/pbt/transitions/navigate_home.rs` (open_pins maintenance)
- `crates/holon-integration-tests/src/pbt/transitions/click_block.rs` (open_pins maintenance, LeftSidebar branch)
- `crates/holon-integration-tests/src/pbt/transitions/pin_block.rs` (NEW)
- `crates/holon-integration-tests/src/pbt/transitions/unpin_block.rs` (NEW)
- `crates/holon-integration-tests/src/pbt/transitions/mod.rs` (mod + pub use + macro variant)
- `crates/holon-integration-tests/src/pbt/transition_dispatch.rs` (SutHandle methods)
- `crates/holon-integration-tests/src/pbt/sut.rs` (E2ESut impls + watch SQL filter + dump_nav_tables fix)
- `crates/holon/src/storage/mod.rs` (sql_statements: skip `;` in `--` comments — FU-12)
- `devlog/2026-05-07-164740-logseq-sidebar-followups.md` (this section)

---

## FU-14 root cause + fix (2026-05-07, post-Turso-bump session)

**Status**: FIXED. PBT `general_e2e_pbt` + `general_e2e_pbt_sql_only` both pass (2/2).

### Hypothesis the previous session ran with — wrong

The earlier handoff guessed FU-14 was "Turso IVM CDC lag for the JOIN matview (`navigation_cursor JOIN navigation_history`)" and recommended a truth-check downgrade pattern mirrored from inv-focus-roots. That would have papered over the symptom; the real bug is upstream of the matview entirely.

The user landed `nightscape@holon 05c3326752ff` ("IVM LEFT JOIN drops null-padded row on redundant UPDATE") and asked whether it fixed FU-14. **It did not — and could not**: `current_focus` is an INNER JOIN of two base tables, no LEFT JOIN, no chained matview, no IVM bug class that fix touches.

### Actual root cause

`apply_navigate_focus` at `sut.rs:722` drove the SUT via:

```rust
driver.click_entity(resolved_id.as_str(), "left_sidebar").await
```

The `UserDriver::click_entity` trait default at `crates/holon-frontend/src/user_driver.rs:185` hardcodes `synthetic_dispatch("navigation", "editor_focus", ...)`. That op only writes `editor_cursor` — it never touches `navigation_history` or `navigation_cursor`. So the SQL writes the inv-7 check expects (an `INSERT INTO navigation_history` + `UPDATE navigation_cursor`) never fire, and the matview correctly returns nothing.

The `nav_probe` confirms: after a NavigateFocus transition, `navigation_history` had `0 row(s)` and all three `navigation_cursor` rows still had `history_id = NULL` (the seed defaults from `init_default_region.sql`). Nothing to lag — there was nothing to project.

`ReactiveEngineDriver` does **not** override `click_entity`, so the headless PBT path always hit the trait default. The existing comment at the call site claimed the click would "dispatch `navigation.focus(region: "main", block_id)` via the bound action" — that is true only for the GPUI driver, whose `selectable` shadow builder reads `node.click_intent()` and dispatches it from the `on_mouse_down` handler. There is no equivalent path through the headless trait default, and `click_entity_with_tree` (the version that *does* read bound intents via `find_click_intent_oneshot`) was not used here.

### Fix

The right fix isn't to special-case the test — it's to make the headless driver share the same intent-resolution that GPUI's `selectable` shadow builder does. ViewModels already carry the bound click intent (`node.click_intent()`); GPUI's renderer is just a forwarder. The infrastructure was already in place:

- `BuilderServices::snapshot_resolved(&root_uri)` recursively interprets every nested `live_block` and returns a fully-resolved `ViewModel`.
- `focus_path::find_click_intent_in_view_model(&resolved, entity_id)` walks that tree to find the bound intent — its docstring even calls out "the headless-test situation" as the intended use case.

`ReactiveEngineDriver` now overrides `click_entity` to use them, mirroring the GPUI `selectable + render_entity` priority (bound action first, fall through to `editor_focus` for cursor placement):

```rust
async fn click_entity(&self, entity_id: &str, region: &str) -> Result<()> {
    let root_uri = holon_api::root_layout_block_uri();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let resolved = self.engine.snapshot_resolved(&root_uri);
        if let Some(intent) = focus_path::find_click_intent_in_view_model(&resolved, entity_id) {
            return self.apply_intent(intent).await;
        }
        if Instant::now() >= deadline { break; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // No bound action — fall through to cursor placement.
    let mut params = HashMap::new();
    params.insert("region".into(), Value::String(region.to_string()));
    params.insert("block_id".into(), Value::String(entity_id.to_string()));
    params.insert("cursor_offset".into(), Value::Integer(0));
    self.synthetic_dispatch("navigation", "editor_focus", params).await
}
```

The 2s poll handles the brief window where nested `live_block` watches haven't streamed their initial rows yet — same pattern `send_key_chord` uses for its router fallback.

`apply_navigate_focus` then reverts to its original shape — `driver.click_entity(resolved_id, "left_sidebar")` — and the right thing happens. Same call site for sidebar clicks, main-panel clicks, and any future bound-action click; the driver disambiguates from the resolved tree.

### Verification

- `cargo check --workspace`: clean
- `cargo nextest run -p holon-integration-tests --test general_e2e_pbt` (post-shared-path):
  - `general_e2e_pbt`: **PASS** in 274.308s
  - `general_e2e_pbt_sql_only`: **PASS** in 416.554s
- No `[invN WARN]` downgrades in the run log; no panics.

This unblocks **FU-9 Step 6** (≥50 cases validation): the PBT now exercises pin/unpin transitions alongside NavigateFocus without false-positive panics.

### Lesson — ViewModels carry the intent, drivers dispatch it

Earlier I wrote that "the GPUI selectable wiring is purely a UI path the headless test cannot share." That was wrong, and the user pushed back correctly: the architectural rule is "ViewModels carry as much logic as possible; UIs are dumb forwarders." The bound click intent already lives on the `ReactiveViewModel` node thanks to the `selectable` shadow builder; GPUI's `selectable` builder just reads `node.click_intent()` and dispatches it from `on_mouse_down`. The headless `ReactiveEngineDriver` has the same engine and can read the same resolved tree — there's no UI-specific anything in the resolution path.

The take-away for future drivers: anything `selectable.rs` (the GPUI builder) does at click time should also work in `ReactiveEngineDriver::click_entity` via `snapshot_resolved` + `find_click_intent_in_view_model`. The trait default's hardcoded `editor_focus` is the **fallback** for clicks that *don't* hit a bound action (e.g. clicking inside a main-panel block to place the cursor) — not the primary path.

Files modified:

- `crates/holon-frontend/src/user_driver.rs` — `ReactiveEngineDriver::click_entity` now resolves the bound intent via `snapshot_resolved` + `find_click_intent_in_view_model`, mirroring GPUI. Falls through to `editor_focus` only when no bound action exists (matching GPUI's `render_entity` click handler).
- `crates/holon-integration-tests/src/pbt/sut.rs` — `apply_navigate_focus` uses plain `driver.click_entity(resolved_id, "left_sidebar")` again. Adds `drain_region_cdc_events` before the nav probe to match `apply_navigate_back/forward/home` and the pin/unpin transitions.

---

## FU-9 Step 6 — 50-case sweep result (2026-05-08) — CLOSED

**Status (final)**: Both PBT variants PASS at 50 cases after FU-15 fix.

- First sweep (before fix): exposed FU-15 cross-region intent leak (and what initially looked like a separate FU-16 sql_only race). Log: `devlog/runs/2026-05-08-fu9-step6-50case.log`.
- Second sweep (after fix): `general_e2e_pbt` PASS in 1100s, `general_e2e_pbt_sql_only` PASS in 1335s. Log: `devlog/runs/2026-05-08-fu15-50case.log`.

The pin/unpin transitions (`PinBlock`, `UnpinBlock`) and the rewritten `expected_focus_root_ids` invariant exercise correctly across 50 random sequences with no false-positive panics. FU-9's "≥50 cases without divergence" goal is met.

### FU-15 · ClickBlock cross-region bound-action leak — FIXED (2026-05-08)

**Status**: LANDED. The earlier "ref-state apply_to_ref needs to mirror nav.focus on Main clicks" diagnosis was **wrong**. The real bug was on the SUT side: `find_click_intent_in_view_model` walked the WHOLE resolved tree and returned the FIRST entity_id match in DFS order, ignoring which region the click happened in. Same entity (e.g. `block:journals`) appears in BOTH the LeftSidebar list (selectable wrapper with `action: navigation_focus`) AND the Main panel (default block_profile, only `shift_action`). A `ClickBlock(Main, block:journals)` would walk the tree, hit the LeftSidebar's wrapper first, and dispatch its `navigation.focus` — silently turning a Main click into a sidebar click.

**Production GPUI doesn't have this bug**: it dispatches via `on_mouse_down` on the specific element the cursor hit. The element belongs to one region's subtree; bound actions can't leak across regions. The PBT's tree-walking fallback was the seam that broke parity.

**Fix landed**:

- New `find_click_intent_in_region(root, entity_id, region)` in `crates/holon-frontend/src/focus_path.rs` — DFS-finds the panel node by region's panel id (`block:default-{left,main,right}-sidebar`), then walks ONLY that subtree for the click target.
- `ReactiveEngineDriver::click_entity` (`crates/holon-frontend/src/user_driver.rs:383`) now uses the region-scoped variant.
- `apply_click_block` (`crates/holon-integration-tests/src/pbt/sut.rs:2056`) likewise.

**Verification**:

- 2-case sweep: both variants PASS (248s, 293s).
- 10-case sweep: both variants PASS (467s, 552s).
- **50-case sweep: both variants PASS** (`general_e2e_pbt` 1100s, `general_e2e_pbt_sql_only` 1335s — log at `devlog/runs/2026-05-08-fu15-50case.log`). **This closes FU-9 Step 6** (the original ≥50-case target).
- `cargo nextest run -p holon-frontend --lib focus_path`: 8/8 pass including the new `find_click_intent_in_region_scopes_to_panel` regression test that asserts a Main click on `block:foo` does NOT pick up a LeftSidebar selectable's bound action.

**Why the earlier diagnosis missed it**: I read the failing seed's `[ClickBlock] dispatched bound action (entity=block:journals)` log line and assumed the production semantic was "Main click on journal-list item fires nav.focus" (true in production, but only when Main is rendering a list-with-action; in the PBT, no Journals.org parsing means Main shows default block_profile with no regular click action). Once I traced the SUT's actual click code path (rather than the production org-asset semantics), the unscoped tree walk popped out as the obvious culprit.

### FU-16 · sql_only first-NavigateFocus race — RESOLVED by FU-15 (2026-05-08)

**Status**: CLOSED. Original diagnosis was wrong. The sql_only "navigation_history: 0 rows after NavigateFocus" symptom was not a timing race — it was the same FU-15 cross-region intent leak. The unscoped `find_click_intent_in_view_model` was hitting some non-navigation entity match in the wrong subtree and dispatching the wrong (or no) action, leaving navigation_history empty. After FU-15's region scoping landed, the 50-case sweep of `general_e2e_pbt_sql_only` passed in 1335s with no `sut.rs:3946` panics. No separate FU-16 fix needed.

**Lesson**: when a fix narrows the search space (e.g. region-scoping), it can silently resolve adjacent bugs that were "different" symptoms of the same underlying confusion. Always verify the supposed adjacent bug is still reproducible *after* the primary fix lands; don't open a new follow-up for what may already be gone.
