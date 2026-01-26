---
date: "2026-06-19"
session: "f8f988d3"
project: "holon"
---

## SutHandle decomposition — increment 1: `NavigateFocus` onto `SutFocusWrite`

**Context:** Bundle E (`docs/Testing/PbtCompositionBacklog.md`) dissolves the
monolithic `E2ESut`. The named keystone: the bulk SUT caps can't be deleted from
`E2ESut` without dropping their invariants from the *full proptest exploration*
(`native_proxy_invariants`), because the composed slices are only *static*. The
unblock is to drive a composed `CapMap` through the **full transition alphabet** in a
`StateMachineTest`. This increment starts that decomposition with one vertical slice:
the `NavigateFocus` transition (the first non-structural/non-editor cluster).

**Step 0 (make-or-break, green):** a `#[tokio::test]` proved the production
`navigation.focus` op, driven through the **windowless** `FrontendSession`, updates the
`current_focus` / `focus_roots` matviews with no window/driver/geometry. (Doc blocks
pin to `block:ref-doc-N` via `#+ID:`; matviews are standard `NavigationSchemaModule`
matviews present headlessly.)

**Step 0.5 (endgame union probe, green):** compile-only
`assert_three_cap_union::<HeadlessFrontendComponent>` confirms one concrete type
satisfies `SutFocusWrite + SutSqlProjection + SutBackend` at once — the miniature of
the real endgame keystone (union of ~50 bounds on one type). Tractable.

**What changed:**
- **Transition rebind** (`transitions/navigate_focus.rs`): `apply_to_sut` bound
  `S: SutHandle` → `S: SutFocusWrite`; `Region::Main` → `CapRegion::Main`.
- **SutHandle trait** (`transition_dispatch.rs`): removed `apply_navigate_focus` from
  the trait (the `NavigateFocus` variant carries the cap directly). The macro dispatch
  `impl` gained `+ SutFocusWrite` rather than folding it into `SutHandle` as a
  supertrait — a supertrait would clash with `SutHandle`'s remaining
  `apply_focus_editable_text`.
- **E2ESut green** (`sut_handle.rs` / `sut_capabilities.rs`):
  `SutHandle::apply_navigate_focus` flipped `&mut self` → `&self` (V1: the navigation
  state — driver/ctx/engine/geometry — is all behind interior-mut/`Arc` seams; the only
  `&mut` was `ctx.drain_region_cdc_events`, swapped for the `&self`
  `drain_delivery_barrier`, with the region_data mirror drain left to the shared
  `check_invariants` prep that re-drains region CDC anyway). New `impl SutFocusWrite for
  E2ESut` delegates to it.
- **Catalog** (`composed/`): `RefFocus` got its missing `#[capmap_adapter]` + a
  `caps.insert(... as Arc<dyn RefFocus>)` in `ReferenceState::register`; ported
  `InvNavigationFocus` (`Needs SutSqlProjection + RefFocus`) and `InvFocusRoots`
  (`Needs SutBackend + SutSqlProjection + RefFocus`) into the catalog.
- **Caps realized** (`frontend_slice/components.rs`): `HeadlessFrontendComponent`
  implements `SutFocusWrite` (the op + focus-matview fixed-point settle) and
  `SutSqlProjection` (focus rows + block reads); `live_focus_root_rows` now reads the
  `focus_roots` matview (same source as `focus_roots_rows`, so mirror==matview and the
  focus_roots teeth produce a real `Fail`, never a CDC-lag `Skipped`, V4).
- **Slice** (`frontend_slice/navigation_pbt.rs`): a `{NavigateFocus}` `StateMachineTest`
  over a real headless Turso session, checked against a **`RefFocus`-only** ref CapMap
  (only the two focus invariants select — no block-tree alignment; focus reads come from
  `navigation_history`/`open_pins`). `SutSqlProjection` is wired in
  `frontend_navigation_wide`, NOT in `register`, so other frontend-slice tests don't
  newly select `block_content_sql`. Ref seeded to `block:journals` initial focus to match
  the SUT boot. Teeth: lockstep green + non-vacuity (`ran_ids` for both focus invariants);
  SUT-only navigate trips both with `Fail`.

**Selection-safety:** memory_slice's `selects_exactly_the_full_catalog` updated (the two
focus invariants disclosed-deselect there — no `SutSqlProjection`). Parity oracles
unaffected (focus bodies stay in the native runner; the catalog only gained copies).

**Verification:** probe green; navigation slice + teeth green (3/3); lib green except the
2 pre-existing reds (`every_body_file_has_a_registry_entry`,
`now_query_compiles_to_canonical_sql`); native test build compiles green; V3 parity
oracles green; windowed `gpui_window_slice` 1/1.

**Caveat (disclosed):** the native `general_e2e_pbt_sql_only` run hit a **pre-existing
nondeterministic** settle-race flake in `apply_type_chars` (Loro `content_raw` not landed
before TypeChars — the error itself says "increase pre_inv16_settle"; re-run failed on
different random block ids, always in the editor path, never navigation). The plan's
native verification (step 4) is compile-only, which passes; a full native run was not
required and is gated by this independent flake.

**Next:** `navigate_forward`/`home`/`back` (leader-chord surface), then the heavier
`SutHandle` clusters. The endgame keystone (union CapMap + mixed-alphabet StateMachineTest
+ E3/E5) comes once enough of the alphabet is decomposed.
