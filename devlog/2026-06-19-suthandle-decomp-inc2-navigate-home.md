# SutHandle decomposition INC 2 — `NavigateHome` → `SutNavHistoryWrite` (2026-06-19)

Second transition decomposed off the `SutHandle` monolith (after INC 1's
`NavigateFocus` → `SutFocusWrite`). `navigation_pbt` is now the first composed
`StateMachineTest` driving a **2-transition** alphabet (`{NavigateFocus,
NavigateHome}`) through a `CapMap` — a step toward the full-alphabet keystone that
unblocks the bulk E3 cap deletions.

## What landed

- **New cap** `SutNavHistoryWrite { async fn apply_navigate_home(&self, CapRegion) }`
  (`holon-pbt-core/capabilities.rs`, `#[capmap_adapter]`). Holds `go_home` now;
  `apply_navigate_back`/`forward` deferred to E4 (the windowed component) — headless
  prod does not mirror `go_back`/`go_forward` (`maybe_mirror_navigation_focus`,
  `reactive.rs:2391-2396`).
- **Rebind** `NavigateHome::apply_to_sut` `S: SutHandle` → `S: SutNavHistoryWrite`
  (`transitions/navigate_home.rs`); macro dispatch blanket bound extended to
  `SutHandle + SutFocusWrite + SutNavHistoryWrite` (`transition_dispatch.rs`).
- **E2ESut green:** mirrored INC 1 — kept `apply_navigate_home` on `SutHandle` but
  flipped it `&mut self`→`&self` (swap `ctx.drain_region_cdc_events()` →
  `&self` `drain_delivery_barrier`), added `impl SutNavHistoryWrite for E2ESut`
  delegating via `<Self as SutHandle>::apply_navigate_home`. (Deviated from the plan's
  "remove from trait + inherent method" — keeping it in the trait as `&self` is the
  proven INC 1 pattern, lower-risk and functionally identical.)
- **Component realization:** `impl SutNavHistoryWrite for HeadlessFrontendComponent`
  drives `navigation.go_home(region)` through the windowless `FrontendSession` +
  `settle_focus_matviews()` (exact shape of the `SutFocusWrite` impl); registered in
  `CapProvider::register` (selection-neutral write cap, like `SutFocusWrite`).
- **Slice:** `navigation_pbt.rs` alphabet → `prop_oneof![NavigateFocus, NavigateHome]`;
  both `apply` arms wired; Step-0.5 union probe widened to the 4-cap union; two new
  teeth.

## The make-or-break (H4) — PASSED

`NavigateHome` *clears* focus (`go_home` = `set_focus(None)` + `pins.clear()`),
exercising a focus-**clear** settle path INC 1's focus-**set** never touched (risk: the
empty-matview race that bit `SutWatchRows`). The probe
`navigate_home_lockstep_stays_green` drives lockstep `go_home` on SUT+ref and reaches
green — the headless clear settles deterministically. No empty-race.

## Teeth correctness (H1/H2 — corrected during planning)

A senior-review pass mis-hypothesised that (a) `NavigateHome` moves only current focus,
and (b) `navigation_ref()` is an unnavigated seed needing a lockstep-`NavigateFocus`
preamble. Reading the merged code falsified both:
- `navigate_home.rs:84` does `pins.clear()`, and open pins feed
  `expected_focus_root_ids`, so `go_home` moves **focus roots too**. Teeth assert
  **both** `inv-navigation-focus` and `inv-focus-roots` Fail.
- `navigation_ref()` applies `navigate(JOURNALS_ID)` (`navigation_pbt.rs:96`), so the
  seed boots focused on journals **with journals as a focus root** — a SUT-only
  `NavigateHome` bites directly from the seed (no preamble). `sut_only_navigate_home_is_caught_by_focus_invariants`
  trips both invariants; it also deterministically dispatches `NavigateHome` through
  the real `apply_to_sut` path = the H5 coverage guarantee (robust to nextest's
  per-test process isolation, where a cross-test counter can't observe the random run).

## Verification

- `navigation_pbt` 5/5 green (2 pre-existing + 2 new teeth + generated 2-transition
  `StateMachineTest`).
- `cargo check -p holon-integration-tests --features pbt --tests` green.
- `gpui_window_slice` 1/1 (deselects — no `SutNavHistoryWrite`).
- Full lib: baseline 127/3 → now 128 pass + the 2 new teeth. Pre-existing reds
  unchanged (`every_body_file_has_a_registry_entry`, `now_query_compiles_to_canonical_sql`,
  and the org-keyword `keyword_set_survives_sut_serialize_parse` proptest, which
  flakes as fail-or-shrink-timeout). `frontend_slice_displayed_text_viewmodel_bites_on_nested_content`
  is a pre-existing warm-loop **load-flake** (passes isolated in 3.3 s; tipped over only
  under the full-suite parallel saturation) — not an INC 2 regression (the only touch to
  that path is a selection-neutral `Arc` insert).

## Next

Remaining `SutHandle` clusters: `start_app`→`SutLifecycle`, `setup_watch`→`SutWatchRows`,
`switch_view`→`SutLayout`/`SutViewModel`, `toggle_state`→`SutLayout`+`SutDriver`,
`click_at_element`→`SutDriver`; plus closing the nav trait-split (back/forward onto
`SutNavHistoryWrite` in E4). Then E2 (attended parity) → E3 (bulk cap deletion) → E4/E5.
