# kitchen-b Inc B — adversarial verification

## VERDICT: CONFIRMED

Every claim was reproduced by me in this session. No defects found. The lane
report matches what I observed. Working copy left byte-identical to the original
commit (`jj diff --from 4a9923ccf3f2 --to @` = empty; probe reverts all
sha256-verified).

Identity asserted: change id `lvvymuvzqpqx`, parent `50394bb92454` (matches),
tree-marker `COOKABLE_RECIPES_SQL` in `crates/holon-kitchen/src/cookable.rs:51`.

---

## Checks

**1. Keystone 7/7 + both teeth mutations.**
- `cargo nextest -p holon --features test-helpers --test kitchen_cookable_now_e2e`
  → `7 tests run: 7 passed` (log `lane-logs/verify-b-keystone2.log`). 7 ran, not 0.
- Mutation A — deleted `EXISTS (SELECT 1 FROM ingredient_use k ...)` vacuity
  guard (`cookable.rs:55`): reds ONLY
  `a_recipe_with_no_known_ingredients_is_not_cookable` (6 passed, 1 failed,
  `verify-b-mut1.log`). Confirms the guard prevents a recipe with zero
  ingredient_use rows reading vacuously cookable. Restored, sha256 matches
  `24bb1527…`.
- Mutation B — deleted the same-unit clause
  `(p.unit = iu.unit OR (p.unit IS NULL AND iu.unit IS NULL)) AND` from
  `SATISFIES` (`cookable.rs:34`): reds ONLY
  `an_unconvertible_unit_blocks_the_recipe_by_name` (`verify-b-mut2.log`).
  Restored, sha256 matches.

**2. Fail-loud conversion.** `an_unconvertible_unit_blocks_the_recipe_by_name`
stocks `sugar 500 kg` against a recipe needing `100 g` (number deliberately
larger, so only the unit rule can block). Recipe is NOT cookable AND surfaces
`("sugar", Unconvertible)` — a named blocker, never silently satisfied, never
dropped. `COOK_BLOCKERS_SQL` emits the reason via a CASE that distinguishes
missing / unconvertible / insufficient, parsed into an enum (`CookBlockReason`)
that fails loud on any fourth string. Consume side also refuses unconvertible
units (`consuming_in_an_unconvertible_unit_is_refused` green).

**3. Unwritable-types fix is real (inversion).** Stripped `properties` +
`property_kinds` from `recipe.yaml`, ran the recipe-creating test: create is
refused at the write boundary with
`field '_provenance' is not a column of 'recipe_raw' and 'recipe' declares no
'properties' overflow column, so this write has nowhere to land`
(`verify-b-inv.log`). Restored, sha256 matches `14b4c553…`. With the columns
present the create succeeds (keystone green). All three types (recipe,
ingredient_use, pantry_item) carry both columns.

**4. Id-minting.** Replaced `minted("recipe", recipe_id)` with the bare
`recipe_id` in `require()`: the `iu.recipe_id = r.id` join silently matches
nothing — `blockers()` returns `[]` instead of `[(flour,Missing),(milk,Missing)]`,
no error raised (`verify-b-mint.log`). Confirms the minted form (`recipe:...`)
is load-bearing and the helper is used at every cross-table reference
(require/consume/blockers). Restored, sha256 matches `98de470c…`.

**5. A/B on the 5 e2e_backend_engine_test failures.** At the lane tree the 5
failures are `test_operation_triggers_stream_update`, `test_basic_query_execution`,
`test_multiple_operations_sequence`, `test_query_and_watch_stream`,
`test_create_and_delete_workflow` (1 passed, 5 failed —
`verify-b-e2e-lane.log`). The lane's `lane-logs/ab-base.1788279215.log`
(run at restored base 50394bb9) shows the IDENTICAL 5 names, same 1-passed/5-failed,
same cause `cannot modify materialized view block`. Set-equal, nextest is
process-isolated, no NEW failure name at the lane tree. Pre-existing, not the
lane's.

**6. F4 — NOT EXISTS + asserted disclosure.** `COOKABLE_RECIPES_SQL` is the
`NOT EXISTS(unsatisfied)` anti-join shape, NOT a `GROUP BY / HAVING COUNT(col)`
tautology. `the_cookable_list_updates_live_and_discloses_its_degraded_mode`
asserts `batches.iter().any(|b| b.metadata.degraded.is_some())` — degraded mode
cannot go silent without reding the test (it passed).

**7. Gates.** `cargo fmt --all -- --check` clean (exit 0). `cargo check
--workspace --all-targets` clean (Finished, 0 error lines,
`verify-b-gates.log`). `just keystone-smoke` → `4 passed; 0 failed`
(`verify-b-smoke.log`). The `0/40` invariants in the smoke engagement summary
(editor-caret/text mirror, sql-budget) are unexercised/deselected in the smoke
profile, not failures.

## Notes (non-defects)
- Pantry `consume` registers only `if dispatcher.has_provider("pantry_item")`
  (operation_dispatcher.rs). Benign: the type yaml registers the generic
  provider, so the guard is true and consume registers — proven by the consume
  tests passing. It only adds consume beside an existing authority.
- `pantry_operations.rs` builds `consume` SQL by string interpolation with
  manual `'`-escaping (`read_stock`, UPDATE). Internal/PrivateOnly op, ids from
  params; not part of the claim's teeth. Noted, not blocking.
