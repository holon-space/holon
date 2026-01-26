# C-5 audit — honest-empty cap methods vs vacuous-green invariants (2026-07-01)

Executed per Design §11 C-5 (read-only audit; fixes tracked separately). Orchestrator
spot-verified the four load-bearing claims (implementor sets, WIDE_REQUIRED lines,
caller greps); one agent claim corrected — see "Dead code" below.

## Resolution status (2026-07-02, pbt-target-arch)

- **TIER 1 — `inv-loro-no-errors`: RESOLVED.** `compose_sut` full mode now backs
  `SutLoroLog` with the live `LoroSyncControllerHandle` error counter
  (`LoroBackendComponent::new_shared_with_sync_handle` + `builder.rs`); pure-Loro stays
  honest-`false` (no controller). Real teeth in the keystone (WIDE_REQUIRED). Stale
  "teeth in the E2E slice" comment fixed.
- **TIER 1 — the three `SutViewModel` emission methods: RESOLVED (2026-07-02, verified via
  the windowed loop).** Split into a narrow `SutFrontendEmissions` cap
  (`live_vs_fresh_tree_diff` / `drain_vm_emission_toggles` / `provider_stability_report`),
  implemented FOR REAL on the windowed `GpuiFrontendEngineComponent` over its live
  `ReactiveEngine` (faithful ports of the deleted `E2ESut` bodies — a background emission
  collector spawned once over `engine.watch`, a persistent `HeadlessLiveTree` cell reused
  across transitions, and the twice-interpret provider-cache probe; no engine-side changes).
  Registered on the windowed composition via `overlay_windowed_caps`; the headless
  `HeadlessFrontendComponent` DROPPED its honest-`None`/`[]` stubs and does NOT register the
  cap, so the three invariants DESELECT honestly on every headless gate. Verified SELECT + RUN
  + GREEN on the windowed loop (grep `[windowed ran]` in `gpui_composed_windowed_loop` /
  `gpui_compose_sut_windowed`), 28 ticks each, zero divergence — real teeth, not cosmetic green.
- **TIER 2 — `inv-frontend-engine` / `inv-frontend-root-not-error`: RESOLVED (2026-07-02).**
  `frontend_root_vm`/`frontend_root_is_error` split into a windowed-only `SutFrontendEngine`
  cap (kept separate from `SutFrontendEmissions`: distinct concern — root-VM resolution, ALSO
  consumed by `inv-frontend-bounds-rendered`, and `frontend_root_vm` carries the `CachingProxy`
  memoization). The existing `required_invariants` cap_set filter auto-dropped both from the
  headless keystone floor with NO `WIDE_REQUIRED_INVARIANTS` edit (the keystone stays green
  because the filter no longer requires them); the windowed floor keeps them and they run over
  the live gpui root VM.
- **TIER 3 — `SutFocusProjection` split: RESOLVED.** `current_focus_rows` /
  `focus_roots_rows` / `nav_history_open_rows` moved off `SutSqlProjection` into a new
  `SutFocusProjection` cap, registered only where navigation is driven (frontend /
  `full_headless` / navigation slice). `sql_slice` + `sql_loro_slice` no longer register
  it, so `inv-navigation-focus` / `inv-focus-roots` now DESELECT there honestly instead
  of passing vacuously. Keystone floor unchanged (full_headless hosts the cap);
  `frontend_navigation_pbt` still runs both invariants over real focus data.
- **TIER 4 — `inv-view-selection`: audit claim STALE; effectively RESOLVED.** A wired
  `SwitchView` transition (`E2ETransition::SwitchView`, `required_caps = SutViewControl`,
  provided by `full_headless`) already drives `current_view` on BOTH sides
  (`apply_to_ref` sets `ui.user.current_view`; `apply_to_sut` calls
  `SutViewControl::switch_view`), so the invariant is NON-vacuous in the keystone — the
  view moves and both sides are compared. Remaining gap is FAITHFULNESS, not vacuity:
  `SwitchView` is a `SutViewControl` interior-mut set (dispatch shortcut), while the
  UI-adjacent `SwitchViewMode` click path does not move the observable `current_view`
  yet (the headless engine's view state isn't reflected in `current_view()`). Making the
  click path move the observable is the drive-interactions follow-up.

## Findings by severity

### TIER 1 — always-vacuous: teeth in NO shipping config (violates C-5 directly)
`inv-live-tree-matches-fresh`, `inv-value-fn-provider-identity`,
`inv-value-fn-provider-arg-variance-13` (all via `SutViewModel` methods
`live_vs_fresh_tree_diff` / `drain_vm_emission_toggles` / `provider_stability_report` —
honest-`None`/`[]` in BOTH real providers: `HeadlessFrontendComponent`
frontend_slice/components.rs:833–842 and `GpuiFrontendEngineComponent`
window_slice/components.rs:283–291), and `inv-loro-no-errors`
(`SutLoroLog::loro_had_errors` const-`false` in `LoroBackendComponent`
loro_slice/components.rs:81 — the "teeth in the E2E slice's real counter" comment is
STALE; that impl was deleted in the E-track).

**Fix (per fix-the-cap-not-withhold):** implement the three emission methods on
`GpuiFrontendEngineComponent` over its live `ReactiveEngine`, then SPLIT them off
`SutViewModel` into a narrow cap (e.g. `SutFrontendEmissions`) so headless deselects
disclosedly. For `inv-loro-no-errors`: register a `SutLoroLog` backed by the real
`frontend_sync_handle` error counter in `composed/builder.rs` full mode (resolved
~:371) instead of the const-`false` shadow.

### TIER 2 — hollow WIDE_REQUIRED in the headless keystone
`inv-frontend-engine`, `inv-frontend-root-not-error` (WIDE_REQUIRED_INVARIANTS,
wide_e2e.rs:181–182) read `frontend_root_vm`/`frontend_root_is_error`, honest-empty in
`HeadlessFrontendComponent` — so the keystone's "required" guarantee cannot fail there;
real teeth are windowed-only. **Fix:** same cap split (deselect headless, run windowed)
or drop from WIDE_REQUIRED with the windowed floor carrying them (Phase 1 windowed
proptest loop makes the windowed run a first-class harness — natural home).

### TIER 3 — vacuous in storage slices, teeth in navigation slice
`inv-navigation-focus`, `inv-focus-roots`: `sql_slice` registers the focus-projection
family honest-empty (components.rs:161,211,268–280) while `ReferenceState` registers
`RefFocus` unconditionally → selected + empty-vs-unnavigated-ref pass. **Fix:** split
`current_focus_rows`/`focus_roots_rows`/`nav_history_open_rows` into a
`SutFocusProjection` cap registered only where navigation is driven.

### TIER 4 — no driving transition
`inv-view-selection`: `current_view` real but constant `"all"`; no `SetViewMode`
transition exists. **Fix:** add the transition (preferred) or annotate identity-at-rest.

### Dead code (corrected by orchestrator)
`SutSqlProjection::watch_row_count` + `SutViewModel::drain_vm_emissions` have no
catalog-invariant readers, BUT `caching_proxy.rs:75` consumes `drain_vm_emissions` and
`:180` forwards `watch_row_count` — both die with `CachingProxy` (already on the
deletion path, Design §11), so delete them together, not before.

## Verified-SOUND (audit completeness)
`SutLayout::visual_content_fraction` (geometry siblings carry the signal);
`SutDriver::engine_focused_block` (body self-skips on NoEngine);
memory/loro `live_focus_root_rows` (invariant deselects — no `SutSqlProjection`);
`headless_error_node_count`; `loro_children_of`/`loro_block_snapshot`/
`loro_task_state_of`; all `SqlProjectionComponent` block-data methods.

## Structural root cause
Tier 1/2 are E-track porting residue: the invariants were ported from `E2ESut` whose
real impls were deleted, leaving honest-empty stubs as the ONLY providers. The C-5 rule
(honest-empty legal only when the cap's other methods carry real data AND no verdict
rests solely on the empty family) would have caught this at port time — enforce it in
the pbt-composition skill review checklist.
