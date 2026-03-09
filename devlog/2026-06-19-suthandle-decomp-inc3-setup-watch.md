# SutHandle decomposition INC 3 — `SetupWatch` → `SutWatchRegister` (2026-06-19)

Third transition decomposed off the `SutHandle` monolith (after INC 1 `NavigateFocus`
→ `SutFocusWrite`, INC 2 `NavigateHome` → `SutNavHistoryWrite`). The first cluster that
required a **prerequisite refactor**: flipping `E2ESut`'s watch state to interior-mut.
This retires the keystone-risk-row-A "watch cluster UNPROVEN flippable" unknown.

## Why this needed an interior-mut refactor first

INC 1/2 were cheap because their state was already behind `Arc`/interior-mut seams, so
the `&self` cap could delegate trivially. `setup_watch` writes three plain `HashMap`
fields on `TestEnvironment` (`active_watches` / `watch_queries` / `ui_model`), so a
`&self` `SutWatchRegister` cap was impossible without first making those interior-mutable.

## What landed

- **Interior-mut refactor** (`test_environment.rs`): the three watch `HashMap`s →
  `RefCell<HashMap<…>>`. `RefCell` (not `Mutex`) is sound: an Explore-pass audit confirmed
  **no borrow crosses an `.await`** (the two CDC drain loops `drain_cdc_events` /
  `assert_cdc_quiescent` poll with `now_or_never`, never awaiting mid-borrow), and `E2ESut`
  is never `Send`-bound (all its async cap impls are `?Send`, driven via `block_on`, never
  `tokio::spawn`). `setup_watch` flipped `&mut self`→`&self`; `remove_watch` too. The drain
  loops keep `&mut self` and just `.borrow_mut()` the watch cells — their guards drop
  before the `&mut self` `all_blocks_stream` access (sequential, not interleaved), so no
  `&self`-guard-vs-`&mut`-field conflict. Blast radius: `test_environment.rs` (~8 sites) +
  `sut_capabilities.rs:305` (`watch_row_count` → `.borrow()`); the many other
  `active_watches` hits are `MCPServerActorState`/`ReferenceState` (different structs).
- **New cap** `SutWatchRegister { register_watch(query_id, source, lang) }`
  (`holon-pbt-core/capabilities.rs`, `#[capmap_adapter]`). Takes the **compiled** query —
  pbt-core can't name the int-test `TestQuery`, so the `SetupWatch` transition compiles via
  `compile_for` at the boundary.
- **Rebind** `SetupWatch::apply_to_sut` `S: SutHandle` → `S: SutWatchRegister`
  (`transitions/setup_watch.rs`); macro dispatch blanket bound extended to
  `… + SutWatchRegister` (`transition_dispatch.rs`).
- **E2ESut**: `impl SutWatchRegister for E2ESut` forwards to `TestEnvironment::setup_watch`
  (now `&self`, reached via `Deref`). The dead `SutHandle::apply_setup_watch` was
  **removed** from the trait + impl (nothing delegates to it — unlike `apply_navigate_home`,
  which its `SutNavHistoryWrite` impl still calls).
- **Component**: `impl SutWatchRegister for HeadlessFrontendComponent` shares a new
  `register_watch_compiled` core with `register_query_watch`; registered as a
  selection-neutral write cap in `CapProvider::register`.
- **Teeth**: `frontend_slice_setup_watch_via_cap_makes_invariants_bite` drives `SetupWatch`
  through the composed `CapMap` (`apply_to_sut(&mut caps)`), then asserts
  `inv-watch-rows-match-ref` reaches `Ok` clean and `Fail` on a dropped child — the
  composed-drive payoff. (Uses the `ReferenceState`-owns-`Runtime` off-thread-drop trick:
  borrow `clean` for `apply_to_sut`, then move it into `run_with_seeded_ref`.)

## Validation

- `general_e2e_pbt_full` **PASSES** — exercises the E2ESut `setup_watch`/drain RefCell path
  at runtime with no borrow panic. (`general_e2e_pbt_sql_only` fails "SetupWatch: 32
  rejections" — the documented pre-existing sql_only **generation** flake, not this work.)
- INC 3 teeth green; `navigation_pbt` (INC1/2) + existing B5 watch teeth still green (7/7).
- Full lib: 134 pass / **same 4 pre-existing reds** (`every_body_file_has_a_registry_entry`,
  `now_query_compiles_to_canonical_sql`, the `displayed_text/viewmodel` load-flake, and the
  org `keyword_set_survives_sut_serialize_parse` bug). No new reds.
- `cargo check -p holon-integration-tests --features pbt --tests` green.
- `cdc_delivery_pbt` is **pre-existing red** (unrelated): its `component_pbt!` config has
  `preset lifecycle` but NOT `preset org_writes`, so nothing bootstraps `StartApp` → "no
  transition applicable" at init, before booting. (`split_block_content_pbt`, which has
  `org_writes`, passes 10/10.) Worth a separate fix: add `preset org_writes`.

## Next

Remaining `SutHandle` clusters: `start_app`/lifecycle (`&self` flip hard — `start_app`
*builds* fields), `switch_view`, `toggle_state`/`click_at_element` (need the windowed E4
component), and `navigate_forward`/`back` (E4 — headless prod doesn't mirror them). Then
the union-of-bounds keystone → E3/E5.
