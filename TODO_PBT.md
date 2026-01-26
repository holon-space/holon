# PBT Audit — Weak Assertions & Shortcuts

Audit date: 2026-02-19. Reviewed 12 PBT files across 6 subsystems.

## Applied Fixes (2026-02-19)

All items below marked [DONE] have been implemented and compile-verified.

### Cross-Cutting (pbt_infrastructure.rs + loro_backend_pbt.rs)
- [DONE] `verify_backends_match` — compares block fields (parent_id, content_type, content, source_language), not indented strings
- [DONE] `non_root_ids` — semantic filter via `is_document_uri` / `NO_PARENT_ID`, not positional `skip(1)`
- [DONE] `CreateBlocks` — each block picks independent parent
- [DONE] `DeleteBlocks` — deduplicates IDs via sort+dedup
- [DONE] ID mapping — tiebreaker for ambiguous `(parent_id, content)` matches
- [DONE] `WatchChanges` precondition — returns true (always valid)

### E2E Integration (general_e2e_pbt.rs)
- [DONE] Timeout on block count → panic instead of eprintln
- [DONE] Post-mutation spot-check → asserts query success
- [DONE] BulkExternalAdd file verification → `==` instead of `>=`
- [DONE] `max_shrink_iters` → 200 (was 0)
- [DONE] `_ext_event` renamed with FIXME comment about CRDT testing gap
- [TODO] `apply_concurrent_mutations` needs true concurrent external mutation from pre-merge state

### Petri-net Engine (pbt.rs)
- [DONE] `consume` — randomly generated true/false
- [DONE] `creates` — 0-2 CreateArc entries generated
- [DONE] `postcond` — optional status postconditions generated
- [DONE] Attribute equality — bidirectional `prop_assert_eq!(attrs(), attrs())`
- [DONE] `fire()` errors — `.expect()` instead of `let _ =`
- [DONE] Determinism test — token count assertion added
- [DONE] Clock — fixed deterministic value
- [TODO] Preconditions — placeholder ($var) and numeric comparison generation not yet added

### Turso Storage (turso_pbt_tests.rs)
- [DONE] Reverse direction check — queries all Turso rows, asserts count matches
- [DONE] `deleted_ids` tracking — maintains set, verifies absence, handles re-insert
- [DONE] `parent_id` filters — added Eq/IsNull/IsNotNull for parent_id
- [DONE] CDC dead code — removed broken verification block
- [DONE] Concurrent/Transaction batch verification — calls `verify_states_match` after execution
- [TODO] Multi-entity testing (only "entity" is used)
- [TODO] Higher cardinality ID space

### Flutter (flutter_pbt_*.rs)
- [DONE] Seed parameter — uses `TestRng::from_seed` with actual seed bytes
- [DONE] Cleanup deletes — `.expect()` + verification that Flutter is empty
- [TODO] `move_block` `after` parameter — still dropped (requires callback type change)

### Loro Backend (loro_backend_pbt.rs)
- [DONE] `UnwatchChanges` generation — added with weight 3 when watchers exist
- [DONE] `FieldsChanged` — compares field values, not just entity_id/origin
- [DONE] Test config — cases: 20, steps: 1..40, failure persistence enabled
- [TODO] Notification sync — still uses 50ms sleep (bounded retry not implemented)
- [TODO] `watch_changes_since(Version)` replays entire event log (SUT bug, not test bug)

### OrgMode (roundtrip_pbt.rs)
- [DONE] Source block IDs — deterministic format `src-block-{idx}`
- [DONE] Nested headings — reparenting strategy generates depth-2 headings
- [DONE] Body text — expanded to include safe punctuation `.,;:!?()-`
- [DONE] Block ordering — verifies text/source block order preservation after roundtrip

### Todoist (pbt_test.rs)
- [TODO] Sync operations return hardcoded empty results
- [TODO] Task ID mismatch between reference and actual Todoist

## Applied Fixes (2026-02-19)

### Cross-Cutting (`pbt_infrastructure.rs`)
- [x] `verify_backends_match` — compares block fields (parent_id, content_type, content, source_language) instead of indented content strings
- [x] `non_root_ids` — semantic filter via `is_document_uri()` / `NO_PARENT_ID` instead of positional `skip(1)`
- [x] ID mapping — tiebreaker for ambiguous `(parent_id, content)` matches, positional pairing for `CreateBlocks`
- [x] `CreateBlocks` — each block gets independently chosen parent
- [x] `DeleteBlocks` — deduplication via `sort()` + `dedup()` + `prop_filter`
- [x] `WatchChanges` precondition — returns `true` (always valid)

### E2E Integration (`general_e2e_pbt.rs`)
- [x] Timeout on block count — `panic!` instead of `eprintln!` for `BulkExternalAdd`
- [x] Post-mutation spot-check — asserts query success instead of silently skipping
- [x] `BulkExternalAdd` file verification — exact count (`==`) instead of `>=`
- [x] `max_shrink_iters` — increased from 0 to 200
- [x] `apply_concurrent_mutations` — FIXME comment documenting that ext_event is not truly concurrent

### Petri-net Engine (`pbt.rs`)
- [x] `consume` — randomly generated (was hardcoded `false`)
- [x] `creates` — 0-2 CreateArc entries generated (was hardcoded `vec![]`)
- [x] `postcond` — optional status postconditions generated (was hardcoded empty)
- [x] Attribute equality — bidirectional comparison via `prop_assert_eq!`
- [x] `fire()` results — asserted with `.expect()` (was `let _ =`)
- [x] `determinism` — checks `sim1.tokens().count() == sim2.tokens().count()`
- [x] Clock — fixed deterministic value (was `Utc::now()`)

### Turso Storage (`turso_pbt_tests.rs`)
- [x] Reverse direction check — queries all Turso rows and asserts count matches reference
- [x] Deleted entity tracking — `deleted_ids` field, populated on delete, removed on re-insert, verified in `verify_states_match`
- [x] `parent_id` filters — added `Eq`, `IsNull`, `IsNotNull` for `parent_id` field
- [x] Dead CDC verification — removed (was always `None`, would panic)
- [x] Concurrent/Transaction batches — `verify_states_match` called after execution

### Flutter PBTs (`flutter_pbt_runner.rs`, `flutter_pbt_backend.rs`)
- [x] Seed parameter — uses `TestRng::from_seed()` with actual seed (was ignored)
- [x] Cleanup deletes — `.expect()` instead of `let _ =`, post-cleanup verification

### Loro Backend (`loro_backend_pbt.rs`)
- [x] `UnwatchChanges` — generated as transition when active watchers exist
- [x] `FieldsChanged` — compares `fields` payload (was only checking `entity_id` and `origin`)
- [x] Test config — `cases: 20`, `steps: 1..40`, failure persistence enabled
- [x] `non_root_ids` — semantic filter (matching shared infra fix)

### OrgMode (`roundtrip_pbt.rs`)
- [x] Source block IDs — deterministic `format!("src-block-{}", idx)` (was `Uuid::new_v4()`)
- [x] Nested headings — reparenting strategy for depth > 1
- [x] Body text — expanded to `[a-zA-Z0-9 .,;:!?()\\-]`
- [x] Block ordering — verified text block counts and source block ID sequences after roundtrip

## Remaining (not yet applied)

### E2E — needs design work
- [ ] `apply_concurrent_mutations` — external mutation should use pre-merge state for true CRDT conflict testing
- [ ] Pre-startup org parser — handle nesting in reference model, generate nested headlines
- [ ] `simulate_restart` — actually clear OrgAdapter known state

### Todoist — stubs need real implementation
- [ ] `FullSync`/`IncrementalSync` — return hardcoded empty results
- [ ] Task ID mismatch — reference IDs never match actual Todoist IDs

### Loro Backend — needs investigation
- [ ] `watch_changes_since(Version)` — replays entire event log ignoring version position

### Petri-net — additional coverage
- [ ] Preconditions — add placeholder (`$var`) and numeric comparison expressions

---

## Original Findings

These affect multiple PBTs via shared infrastructure in `crates/holon/src/api/pbt_infrastructure.rs`.

### `verify_backends_match` compares content strings, not block structure
**Affects: E2E, Loro Backend, Flutter** (`pbt_infrastructure.rs:256-286`)

Formats each block as `"{indent}{content}"` and diffs the joined text. Ignores `parent_id`, `content_type`, `source_language`, and `properties`. Two structurally different trees producing the same indented text pass silently. A `MoveBlock` bug that relocates a block to a different parent at the same depth is invisible.

**Fix:** Compare blocks field-by-field: `(parent_id, content, content_type, source_language)`. Use tree-ordered comparison or compare parent_id chains explicitly.

### `non_root_ids = skip(1)` is positional, not semantic
**Affects: Loro Backend, Flutter** (`pbt_infrastructure.rs:520-521`)

Root exclusion relies on the first block in iteration order being the root. LoroBackend iterates a hashmap (arbitrary order), so `skip(1)` excludes a random block.

**Fix:** Filter by `is_document_uri(&block.parent_id)` or `parent_id == NO_PARENT_ID`, not position.

### ID mapping after create uses `(parent_id, content)` — ambiguous with duplicates
**Affects: Loro Backend, Flutter** (`pbt_infrastructure.rs:329-393`)

When two sibling blocks have identical content (easy with `[a-z]{1,10}` generator), `find()` picks an arbitrary match. Subsequent operations may target the wrong block in the SUT without detection.

**Fix:** Use creation order/position within parent's children list as tiebreaker, or ensure generated content is unique.

### `CreateBlocks` always uses a single shared parent for all blocks in batch
**Affects: Loro Backend, Flutter** (`pbt_infrastructure.rs:411-418`)

All blocks in a `CreateBlocks` batch share the same parent. Multi-parent batch creates (a valid common case) are never tested.

**Fix:** Generate each block with an independently chosen parent: `prop::collection::vec((prop::sample::select(all_ids), "[a-z]{1,10}"), 1..=3)`.

### `DeleteBlocks` can generate duplicate IDs in a single batch
**Affects: Loro Backend, Flutter** (`pbt_infrastructure.rs:442`)

`prop::collection::vec(prop::sample::select(...))` can generate the same ID twice. MemoryBackend deduplicates via a `seen` HashSet; if LoroBackend doesn't, the second delete would fail or diverge.

**Fix:** Use `prop::collection::hash_set` or deduplicate in `prop_map`.

### `WatchChanges`/`UnwatchChanges` transitions are generated but preconditions always return `false`
**Affects: Loro Backend, Flutter** (`pbt_infrastructure.rs:493-496`)

Watcher transitions exist in the `BlockTransition` enum but `check_transition_preconditions` returns `false` for them. The entire watch/unwatch feature is untested via shared infrastructure.

---

## E2E Integration PBT

File: `crates/holon-integration-tests/tests/general_e2e_pbt.rs`

### Critical

#### `apply_concurrent_mutations` ignores `_ext_event` — CRDT conflicts untested (Confidence: 98)
Lines 2876-2879, 2896. The `_ext_event` parameter is prefixed with underscore and never used. The external mutation is applied from `ref_state` (which already has both mutations merged), not from an independent pre-merge snapshot. The CRDT merge path this transition explicitly targets can never trigger a real conflict.

**Fix:** Apply external mutation from its own independent state snapshot (before UI mutation was applied).

#### Timeout on block count is `eprintln!`, never asserted (Confidence: 95)
Lines 2807-2822, 2905-2920. `wait_for_block_count()` returns the last seen rows on timeout. The result is only logged, never asserted. A complete block loss bug passes silently.

**Fix:** `assert_eq!(actual_rows.len(), expected_count, ...)`.

### Important

#### Navigation focus mismatch is `eprintln!`, not assertion (Confidence: 90)
Lines 2606-2613. When reference expects focus state but DB returns nothing, it's only logged. Broken navigation persistence passes all checks.

**Fix:** Change to `panic!` or `assert!`.

#### Pre-startup org parser doesn't handle nesting; generator avoids nesting (Confidence: 85)
Lines 1344-1411, 1619-1640. The reference model's regex-based org parser assigns all blocks to the document root regardless of heading level. The generator only produces level-1 headlines, so this never triggers. Multi-level org structures (a core feature) are excluded from E2E testing.

**Fix:** Support nesting in the reference model parser, and generate nested headlines.

#### `max_shrink_iters: 0` prevents finding minimal failing cases (Confidence: 85)
Lines 2960-2964. Shrinking is completely disabled. Failing cases are reported as the full 3-20 operation sequence with no reduction. Set to at least 100-500.

#### Post-mutation spot-check silently skips on query failure (Confidence: 85)
Lines 2831-2855. For Update/Move mutations, if the query fails, the spot-check is bypassed with no error. A systematic failure to persist updates would go undetected until `check_invariants`.

**Fix:** Assert `spec.is_ok()` and `!spec.data.is_empty()` for Update/Move.

#### CDC drain uses 20ms timeout; late events cause stale invariant check (Confidence: 83)
Line 2689. `drain_cdc_events` uses a 20ms timeout per stream and discards errors. Late CDC events cause the UI model to be stale, and the invariant check passes against stale expectations.

#### `BulkExternalAdd` file verification uses `>=` count via string matching (Confidence: 80)
Lines 2189-2204. Uses `content.matches(":ID:").count() >= expected_block_count` instead of `==`. Extra blocks from a previous file version would pass. Content containing literal `:ID:` causes overcounting.

#### `simulate_restart` doesn't actually clear OrgAdapter state (Confidence: 80)
`test_environment.rs:956-963`. Writes the same content back with a trivial temporary modification. Whether OrgAdapter's `known_state` is actually cleared depends on file watcher implementation details. Bugs manifesting only after a true restart are invisible.

---

## Loro Backend PBT

File: `crates/holon/src/api/loro_backend_pbt.rs`

### Critical

#### `watch_changes_since(Version)` replays entire event log, ignoring version (Confidence: 90)
`loro_backend.rs:833-835`. The LoroBackend's handler replays the entire `event_log` unconditionally instead of trimming by version position. Masked because watchers are only created at version 0 (no prior events). Any test creating a watcher after CRUD operations would see divergence.

**Fix:** Test sequences: create blocks → WatchChanges → verify only post-subscription events are received.

### Important

#### `UnwatchChanges` never generated (Confidence: 88)
Lines 122-129. `transitions()` only generates `WatchChanges` with monotonically increasing IDs. Watcher teardown, resource cleanup, and re-subscription are untested.

**Fix:** Add `UnwatchChanges` to the `transitions()` union when active watchers exist.

#### Only 5 cases × 20 steps (Confidence: 85)
Lines 711-722. Default proptest is 256 cases. With 5 cases and max 20 steps, total exploration is ~100 operations. Many important sequences are statistically unlikely. `failure_persistence: None` means shrunk cases aren't saved between runs.

**Fix:** Increase to at least `cases: 50`, `steps: 1..50`. Re-enable `failure_persistence`.

#### `FieldsChanged` variant doesn't compare field values (Confidence: 82)
Lines 658-691. Only checks `entity_id` and `origin` match. The `fields` payload (what actually changed) is never compared. A LoroBackend emitting wrong field values passes.

**Fix:** Compare `fields` vectors between reference and SUT.

#### 50ms `thread::sleep` for notification sync (Confidence: 82)
Lines 474-476. Creates a NEW runtime distinct from the one used to spawn notification tasks. Tasks on the SUT's runtime may not be polled during sleep.

**Fix:** Poll notification channel with bounded retry, or use `runtime.block_on` with `tokio::time::sleep` on the correct runtime.

#### ID mapping by content is ambiguous (Confidence: 80)
Lines 307-322. `populate_initial_id_map` matches by `(parent_id is document URI) AND content`. If two initial blocks have the same content, mapping is non-deterministic.

---

## Turso Storage PBT

File: `crates/holon/src/storage/turso_pbt_tests.rs`

### Critical

#### Deleted entities never checked for absence (Confidence: 100)
Lines 1266-1315. `verify_states_match` only iterates `reference.entities` (live entities). Deleted entities are `remove()`d from reference. A no-op `delete` implementation that returns `Ok(())` without executing SQL passes all checks.

**Fix:** Maintain `deleted_ids` set. Assert `turso.get(entity, id)` returns `None` for each.

#### CDC verification is dead code (Confidence: 100)
Lines 1910-1985. `state.cdc_connection` is `None` unconditionally (set in constructors, never assigned `Some`). The `EnableCDC` arm returns `Ok(None)` without setting this field. The `expect(...)` would panic if CDC were ever enabled. The entire CDC tracking (`cdc_events`, `cdc_enabled`) is untested.

#### Forward-only comparison — extra rows undetected (Confidence: 95)
Lines 1266-1315. Only checks "every reference row exists in Turso." Never checks "Turso contains exactly the reference set." A buggy delete that duplicates rows passes.

**Fix:** Query all Turso rows per entity and compare set sizes.

### Important

#### Only one hardcoded entity name `"entity"` (Confidence: 92)
Lines 1369-1379. Multi-table scenarios, `get_children`, `get_related`, cross-entity queries, and multi-view scenarios are untested.

#### Concurrent/Transaction batches skip result verification (Confidence: 90)
Lines 1856-1898. Only `BatchMode::Single` compares Query/Get results against the reference. Concurrent and Transaction batches only check for panics/errors.

#### Generated IDs have very low cardinality (Confidence: 85)
Lines 1393-1395. `[a-z]{1,5}` with 1-2 char most common gives ~702 values. With 1-15 operations, insert preconditions reject frequently due to collisions. Database typically has very few rows, reducing chances of exercising IVM and concurrent bugs.

#### Filters never use `parent_id` (Confidence: 82)
Lines 1318-1359. The recursive CTE join field (`parent_id`) is never used in any filter. `IsNull("parent_id")` (find root nodes), `Eq("parent_id", ...)` (find children) — the most important queries — are untested.

#### View change stream comparison doesn't verify deleted entity identity (Confidence: 82)
Lines 2143-2186. For `Deleted` events with ROWIDs never seen in the comparison loop, both `expected_entity_id` and `actual_entity_id` are `None`, so the assertion passes even when delete events refer to different entities.

---

## Flutter PBTs

Files: `frontends/flutter/rust/src/api/flutter_pbt_*.rs`, `pbt_proptest.rs`

### Critical

#### `seed` parameter is ignored — all cases run identical transitions (Confidence: 90)
`flutter_pbt_runner.rs:85-95, 189`. `run_single_proptest_case_native` creates `TestRunner` with `TestRng::deterministic_rng(RngAlgorithm::ChaCha)` which ignores the `seed: u64` parameter. Every test case generates the exact same sequence of transitions. N cases = N identical runs.

**Fix:** Use `TestRng::from_seed(RngAlgorithm::ChaCha, &seed.to_le_bytes().repeat(4))`.

### Important

#### `move_block` drops `after` param — sibling ordering untested (Confidence: 92)
`flutter_pbt_backend.rs:186-196`. The `after` parameter is `let _ = after;`. `apply_transition` also passes `None`. The entire ordered-insertion API surface is untested.

#### Cleanup deletes silently ignored (Confidence: 82)
`flutter_pbt_runner.rs:62-73`. `let _ = flutter_backend.delete_block(&block.id).await;` discards errors. No verification that Flutter is actually empty before starting the next case. No `flush_pending_writes` after deletions.

#### Optimistic ID mapping assumes Flutter uses the provided ID (Confidence: 81)
`flutter_pbt_state_machine.rs:42-65`. `update_id_map_after_create` maps reference ID to the optimistic SUT block ID returned by `create_block`, not the ID actually used by Flutter. If Flutter assigns its own ID internally, the map is wrong for all subsequent operations.

---

## Petri-net Engine PBT

File: `crates/holon-engine/tests/pbt.rs`

### Critical

#### `consume: false` hardcoded (Confidence: 100)
Line 76. Every `InputArc` has `consume: false`. Token removal in `fire()`, `Event.removed` serialization, and `History.replay()` removal path are entirely untested. The token-count invariant check trivially passes because marking never shrinks.

**Fix:** `consume: proptest::bool::ANY`.

#### `creates: vec![]` hardcoded (Confidence: 100)
Line 86. No `CreateArc` is ever generated. `Engine::fire()` create logic, `id_expr` evaluation, `Event.created` serialization, and `History::replay()` creation path are dead code in tests.

**Fix:** Occasionally generate a `CreateArc` with deterministic `id_expr`.

#### `postcond: BTreeMap::new()` hardcoded (Confidence: 100)
Line 79. Every `OutputArc` has empty postconditions. `Event.changes` is always empty. The Rhai postcondition evaluator is entirely unexercised. The `determinism` test's change comparison loop body never executes. The `event_sourcing_roundtrip` attribute comparison trivially passes.

**Fix:** Generate postconditions with safe constant expressions like `"done"`.

### Important

#### Attribute equality check is one-directional (Confidence: 85)
Lines 216-225. Iterates `live`'s attribute keys and checks `replayed` has same values. If `replayed` has extra attributes not in `live`, those go undetected.

**Fix:** `prop_assert_eq!(t_live.attrs(), t_replay.attrs())`.

#### `fire()` errors silently discarded in `wsjf_beats_random` (Confidence: 82)
Lines 251, 263. `let _ = engine.fire(...)` discards errors. If `enabled()` returns bindings that then fail to fire, the test silently skips the step.

**Fix:** Assert `fire()` returns `Ok(...)` since the transition was reported as enabled.

#### `determinism` doesn't check `sim2` has no extra tokens (Confidence: 80)
Lines 173-177. Only iterates `sim1`'s tokens and looks them up in `sim2`. Extra tokens in `sim2` go undetected.

**Fix:** Add `prop_assert_eq!(sim1.tokens().count(), sim2.tokens().count())` and iterate both directions.

#### `chrono::Utc::now()` breaks shrinking and reproducibility (Confidence: 80)
Line 142. Clock is seeded with real wall-clock time. The test is not reproducible from the same proptest seed alone.

**Fix:** Use a fixed clock: `chrono::DateTime::from_timestamp(0, 0).unwrap()`.

#### Preconditions only test exact string match (Confidence: 80)
Lines 69-76. Only exercises branch 3 of `guard.rs` (exact string match against `"status"`). Placeholder binding (`$var`) and Rhai comparison expressions (`>=`, `<=`) are untested.

**Fix:** Add precond entries with `$status` placeholder syntax and numeric comparisons.

---

## OrgMode PBT

File: `crates/holon-orgmode/tests/roundtrip_pbt.rs`

### Important

#### No nested headings tested — only depth-1 (Confidence: 85)
Lines 85-88. All generated blocks have `parent_id = "holon-doc://test.org"` and `level = 1`. Round-trip bugs specific to multi-level nesting are outside test scope despite being a core org-mode feature.

**Fix:** Generate nested heading structures with varying depths.

#### Source block IDs use `Uuid::new_v4()` in `prop_map` — breaks shrinking (Confidence: 85)
Lines 63-64. Each shrink attempt generates a fresh UUID, changing block identity. Minimal failing cases are much larger than necessary.

**Fix:** Use deterministic IDs like heading blocks do: `format!("src-block-{}", i)`.

#### Body text excludes org syntax characters (Confidence: 82)
Lines 49-55. Alphabet `[a-zA-Z0-9 .]` excludes `-` (list items), `#` (keywords), `*` (headings at line start), `:` (drawers). These are valid in real org body text and can cause parse ambiguity.

**Fix:** Include org syntax characters in the generator, at least `"-#*:"`.

#### Block ordering not verified after round-trip (Confidence: 82)
Lines 154-183. Only compares `render1 == render2`. If the parser re-orders blocks, both renders come from the same re-ordered list and the test passes. Ordering bugs are invisible unless they also change rendered content.

---

## Todoist PBT

File: `crates/holon-todoist/src/pbt_test.rs`

### Critical

#### `FullSync`/`IncrementalSync` return hardcoded empty results (Confidence: 100)
Lines 352-371. Both return `Ok((Some(vec![]), None))` with `// TODO` comments. The sync path — the most important path in a sync provider — has zero coverage.

#### Reference task IDs never match actual Todoist IDs (Confidence: 100)
Lines 253-284. Reference stores tasks under generated IDs like `"task-a3b2f1c9"`. Todoist assigns its own numeric IDs. The `_actual_task_id` returned from `create()` is discarded. All subsequent mutations (Update, Delete, Complete) target IDs that don't exist in Todoist.

### Important

#### `verify_states_match` silently skips when no projects exist (Confidence: 80)
Lines 411-416. Returns early with `eprintln!` instead of asserting. Should be an assertion.

---

## Top 5 Recommendations (highest impact)

1. **Fix `verify_backends_match`** to compare `(parent_id, content_type, content, source_language)` per block, not indented content strings. Single fix improves E2E, Loro, and Flutter PBTs.

2. **Petri-net: Generate `consume`, `creates`, and `postcond`** — the three core engine behaviors have zero PBT coverage. The existing tests are effectively no-op smoke tests.

3. **E2E: Make `apply_concurrent_mutations` actually concurrent** — apply external mutation from a pre-merge state snapshot, not the already-merged reference model.

4. **Turso: Add reverse direction check** — query all Turso entities and assert count matches reference. Maintain `deleted_ids` set and verify deletions.

5. **Convert `eprintln!` guards to assertions** — at least 4 locations across E2E and Loro PBTs silently pass on real invariant violations.
