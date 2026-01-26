# Handoff — `SutSqlProjection` E3 deletion via composed gherkin (2026-06-25)

**Goal:** delete `impl SutSqlProjection for E2ESut` (next E3 step toward dissolving
`E2ESut`). Blocked by two standalone slices that dispatch `InvBlockContentMatchesRef`
over `E2ESut`: `tests/split_block_content_pbt.rs` and `tests/peer_conflict_pbt.rs`.
The user chose the **faithful migration** (preserve the gherkin/regression coverage by
moving it onto the composed `ComposedSut`, per Design §8.10 "NO branch" — these slices
carry deterministic regressions + the assert vocabulary that the random `WideE2E` does
not cover, so they're justified composed scaffolding, not redundant).

## ✅ DONE this session (committed in jj `@` = `nkyynntk`, "feat(e3-wip): composed gherkin bridge")

The **whole architecture is proven end-to-end** — `split_block_content_composed_gherkin`
is GREEN (11.5s, 2 scenarios). What landed:

1. **`crates/holon-integration-tests/src/pbt/fixtures/assert.rs`** — added
   `evaluate_assertion_caps(assertion, ref_, caps: &CapMap, resolver: &IdResolver)`, the
   SUT-agnostic composed counterpart of `evaluate_assertion`. Reads:
   - widget → `caps.widget_tree_snapshot()`/`widget_tree_for()` (the `SutRenderer` cap,
     already hosted by `HeadlessFrontendComponent` — both traits have `#[capmap_adapter]`,
     so `CapMap` implements them directly);
   - focus → `caps.current_focus_rows()` (`SutSqlProjection`, region `"main"`);
   - id resolution → `resolve_via(resolver, id)` over the harness `IdResolver`.
   **The `&E2ESut` path is untouched** (the GPUI replay binary still uses it).
2. **`crates/holon-integration-tests/src/pbt/composed/harness.rs`** —
   `impl FixtureAssertable for ComposedSut<S>` calling `evaluate_assertion_caps` with
   `self.rt/caps/resolver`. This is the ONLY gap the migration needed (the runner
   `run_feature_strict<M,S>`/`replay_steps<M,S>` is already SUT-agnostic; `check_invariants`
   runs the full composed catalog every tick — gherkin invariants run for free).
3. **`tests/fixtures/composed_split_gherkin/split_routes_prefix_suffix.feature`** — a
   born-booted feature (NO `Given org file`/`app is started`) on the wide seed.
4. **`tests/split_block_content_pbt.rs`** — additive test `split_block_content_composed_gherkin`
   driving `run_feature_strict::<WideE2EMachine, ComposedSut<WideE2E>>`. GREEN.

### The KEY discoveries (don't re-derive these)

- **Born-boot is reused for free.** `run_feature_strict` → `replay_one` builds the SUT via
  `ComposedSut::<WideE2E>::init_test(&ref_state)` → `WideE2E::build` → `boot_and_seed_wide`
  (born-boots `full_headless` from `WIDE_TREE_ORG`). So **no** custom runner, no `from_parts`,
  no org-parameterized boot was needed — just re-author features onto the wide seed.
- **Born-boot is a HARD constraint** (`StartApp` composed-impl is `unimplemented!()`;
  `WriteOrgFile` needs `SutFixtureFs` which is E2ESut-only). So features MUST drop the
  `Given org file`/`app is started` Background and start from the born-booted seed.
- **Wide seed** (`composed/wide_e2e.rs`): page `block:structural-page` (`page_root()`) →
  children `block:parent`, `block:c1`, `block:c2` (text blocks, content = their title).
- **`NavigateFocus` targets the PAGE** (a sidebar doc — its precondition is
  `predicts_navigation_focus(id, LeftSidebar)`), **not** a leaf. **`SplitBlock` targets a
  CHILD** (precondition: `is_descendant_of_any(block_id, focus_roots(Main))` +
  `renders_block_interactively`). So the pattern is: `focus block:structural-page`, then
  `split block:c1`. (Splitting routes prefix→original, suffix→`block::split-0`; the
  per-tick `inv-block-content-matches-ref` — in `WIDE_REQUIRED_INVARIANTS`, non-vacuous —
  catches mis-routing. This IS the regression reproducer; no brittle render-string assert
  needed for it.)
- **Gherkin step phrasings** (`fixtures/matchers.rs`): `I focus block "<id>" in region "<r>"`,
  `I split block "<id>" at position <n>`; asserts `block "<id>" contains "<text>"` /
  `the widget contains "<text>"` / `focus is on block "<id>"` (optionally `within <N> seconds`).

## ⏳ REMAINING (mechanical — the template is proven)

### Step 1 — re-author the rest of `split_block_content_pbt`'s gherkin onto the wide seed
Map each old `tests/fixtures/_gherkin_*`/`split_block_content_pbt/*.feature` to a born-booted
composed feature in `tests/fixtures/composed_split_gherkin/`:
- `split_then_address_new_block.feature` (VT1) → focus page, split `c1`, then
  `Then ... block "block::split-0" contains "..."`. **NOTE:** `widget contains` is wired
  but NOT yet exercised over composed (only `focus is on` is proven). When you author the
  first `widget contains` assert, dump the actual `snapshot_text` (it appends entity_ids +
  prop values) to pick a substring that's present — `c1`'s content after split-at-1 is `"c"`,
  but the haystack also contains the id `block:c1`, so choose distinctive content or type
  into the block first. Verify the bridge's widget path with one real assert.
- `widget_and_focus.feature` → the assert-vocabulary demo (widget-contains + focus-on).
- `split_corrupt_id.feature` (negative, `#[should_panic(expected = "preconditions FAILED")]`)
  → reference a non-existent block id so `SplitBlock` preconditions fail under strict replay.
- `split_outline_positions.feature` (`split_block_content_pbt_gherkin_outline_expands`) is
  **pure parse, no SUT** — KEEP as-is (doesn't touch E2ESut).
- `then_before_startup.feature` (`..._assert_before_startup_panics`) — **DELETE**: it tests
  the un-booted→assert-vacuous lifecycle path, which is meaningless when born-booted
  (composed is always `app_started`). Note the deletion in the parity/commit message.

Re-point each test fn from `run_feature_strict::<SplitBlockContentPbtMachine, SplitBlockContentPbtSut>`
to `run_feature_strict::<WideE2EMachine, ComposedSut<WideE2E>>`.

### Step 2 — migrate `peer_conflict_pbt`
It's **JSON** replay (`run_fixtures`, `tests/fixtures/peer_conflict_pbt/*.json`), NOT gherkin,
already `full_headless().wiring`. The JSON is a recorded transition sequence — likely includes
lifecycle steps that won't replay born-booted. Cleanest: re-author the peer-merge regression
as a born-booted **gherkin** feature (`AddPeer`/`PeerEdit`/`MergeFromPeer` — `full_headless`
admits peer transitions, proven by `full_headless_cap_set_admits_peer_transitions`) over the
wide seed, driven by `run_feature_strict::<WideE2EMachine, ComposedSut<WideE2E>>`, relying on
`inv-block-content-matches-ref` to catch the conflict divergence. Check whether the gherkin
`matchers.rs` has phrasings for the peer transitions; if not, either add them or keep
peer_conflict as a small literal-sequence composed test (a `#[test]` building `ComposedSut`
and applying the peer transitions directly, like the relocated task-state teeth pattern).

### Step 3 — delete the E2ESut halves
Once both slices' coverage is on composed: delete the `component_pbt!`/`declare_pbt_slice!`
invocations (the random PBT halves — redundant with `WideE2E`), the generated
`SplitBlockContentPbtMachine`/`Sut` types' usages, and the old `_gherkin_*`/`split_block_content_pbt*`
fixture dirs. Confirm `run_feature_strict`/`run_fixtures`/`FixtureAssertable for E2ESut` are
still needed by the GPUI replay binary (grep `replay_steps`/`run_fixtures` consumers) — if the
GPUI path still uses them, KEEP the E2ESut `FixtureAssertable` impl; only the two test files go.

### Step 4 — delete `impl SutSqlProjection for E2ESut` (task #9)
Per the standard E3 mechanic (see the `SutBackend`/`SutErrorLog` entries in
`PbtCompositionBacklog.md` §E3):
1. Remove `SutSqlProjection` from `WideProxyCaps` (supertrait + blanket impl, `invariant_runner.rs`).
2. Drop its native bodies from `native_proxy_invariants` (+ imports).
3. Add the freed ids to `NATIVE_ONLY_EXCLUDED` + a `SutSqlProjection` row in
   `E1_RELOCATED_CAP_COVERAGE` (`composed/parity.rs`).
4. Delete `impl SutSqlProjection for E2ESut` (`sut_capabilities.rs`). Verify
   `SutSqlProjection ∉ SutHandle` first (else it's E5, not E3).
5. **Gates:** `native_runner_dispatches_exactly_the_registry` + `composed_catalog_covers_e1_relocated_caps`
   + `general_e2e_pbt` (E2ESut) + `general_e2e_composed_pbt` (composed) + the new composed gherkin
   tests, all green.

After `SutSqlProjection`, the remaining E3 cap is **`SutViewModel`** (WORST — needs the E4
windowed `frontend_no_error_widgets` wire; `inv-frontend-no-error-widgets`/`-bounds-rendered`
are windowed, not composed-covered). Then E4 (`GpuiWindowComponent`) and E5.

## Gotchas / environment
- Worktree is **jj**, not git. Run bash from this worktree dir. A concurrent fleet process
  squashed the earlier TaskStateSlice cleanup into `main` ~37 min into this session (op log:
  "abandon commit" / "move changes") — that's why `@-` is `main` and `@` holds only the
  migration. Nothing was lost (verified by ground-truth file checks).
- Composed gherkin tests boot a real Turso+Loro `FrontendSession` per scenario (~5-7s each).
- `tmp` for scratch: `/Users/martin/.claude/jobs/d3349863/tmp` (logs from this session there).

## The convergence-rule check (Design §8.10)
This migration IS justified scaffolding (the "NO branch"): it moves the deterministic-regression
+ assert-vocabulary capability onto the composed SUT (the ONE-PBT architecture) and removes
`E2ESut` as its last `SutSqlProjection` consumer. After E5 the composed gherkin features remain
as deterministic steering of the ONE PBT — exactly the user's stated model ("gherkin is one way
to steer PBTs to use certain transitions, plus explicit assertions").
