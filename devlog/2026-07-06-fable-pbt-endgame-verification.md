# PBT Endgame Verification + Three-Red Triage (Fable, 2026-07-06)

Verifier: senior test-architect pass over the Round-6 (2026-07-05) claims in
`docs/Testing/PbtComposition_EndgameRoadmap.md`. Read-only audit; no code changed.
Tree audited: detached HEAD `4ccbbd33e0` (post Petri-net-engine megamerge), working copy `@ d64eeb89`
(WIP diff = formatting only + CLAUDE.md project-tracking section — no behavior).

---

## 1. Verdict on "E2ESut monolith + native runner FULLY retired": **CONFIRMED**

### File-level evidence (`crates/holon-integration-tests/src/pbt/`)

All Round-6 "Deleted files" are actually gone from the tree:

| Claimed deleted | Status |
|---|---|
| `sut.rs` | ABSENT ✓ |
| `sut_handle.rs` | ABSENT ✓ |
| `sut_capabilities.rs` | ABSENT ✓ (`view_model_to_snapshot` relocated to `pbt/vm_snapshot.rs` — present ✓) |
| `sut_check_invariants.rs`, `sut_render.rs`, `sut_keybindings.rs` | ABSENT ✓ |
| `slice.rs` | ABSENT ✓ |

Deliberate keeps are present exactly as disclosed: `sut_loro.rs` (`LoroSut`), `sut_metrics.rs`
(`MetricsSut`), `sut_row_parsing.rs`, `state_machine.rs` (ReferenceMachine), `stepper.rs`
(BisectionStepper). The slice directories (`memory_slice`/`sql_slice`/`loro_slice`/`frontend_slice`/
`window_slice`/`sql_loro_slice`) remain — disclosed as composed-path assets (component impls +
cfg(test) catch triads), W5 scope, not E2ESut scaffolding.

### Symbol-level evidence

- `rg "struct E2ESut|E2ESut::new|E2ESut<"` over `crates/` + `frontends/`: **zero live code hits**.
  Every remaining `E2ESut` occurrence is a doc comment / historical note (~40 sites, e.g.
  `driver_input.rs` "faithful port of E2ESut::…", `tests/bisection_pbt.rs:226` stale doc,
  `frontends/tui/src/user_driver.rs:650` doc referencing the deleted `pbt/sut.rs` path).
- `StateMachineTest` impl inventory: the ONE E2E impl lives in
  `crates/holon-integration-tests/src/pbt/composed/harness.rs` (`ComposedSut`). The other impls
  are narrow component-level PBTs (loro_sync_controller, editor_pure, turso_storage, petri_e2e,
  loro_backend, sync_suite) — not E2E SUTs. **ComposedSut is the sole E2E SUT.** ✓
- The keystone entry `tests/general_e2e_composed_pbt.rs` is a 32-line shell:
  `prop_state_machine! { fn general_e2e_composed_pbt(sequential 1..40 => ComposedSut<WideE2E>) }`.
- Windowed harness repoints confirmed on disk: `frontends/gpui/tests/gpui_composed_windowed_loop.rs`
  (4b random loop), `frontends/gpui/tests/gpui_compose_sut_windowed.rs`,
  `frontends/tui/tests/common/pbt_main.rs` (TUI composed windowed runner). Old
  `HOLON_PBT_WINDOWED_CATALOG` gate confirmed deleted (only a historical mention in
  `sim_windowed_replay.rs` docs).

### Residue (cosmetic only, no action forced)

1. ~40 stale doc-comments naming `E2ESut` / the deleted `sut_capabilities.rs` / `pbt/sut.rs` paths
   (worst offenders: `driver_input.rs`, `op_write_cap.rs:211`, `tui/src/user_driver.rs:650`,
   `tests/bisection_pbt.rs`, `general_e2e_composed_pbt.rs:11-16` module doc still says it "runs
   ALONGSIDE the E2ESut-backed general_e2e_pbt" — that twin no longer exists). Per the
   "code is the strongest signal" principle these are worth one mechanical sweep, since they
   actively point future agents at deleted architecture.
2. `crates/holon-org-format/src/models.rs.orig` — a stray merge artifact tracked in-tree.
3. Git-history oddity (disclosure, not residue): the endgame commit `ec02629897` and the
   taskstate fix `8fa13da9cb` are NOT `--is-ancestor` of HEAD — the Petri-merge history rewrite
   (`30127e8e12 MERGE`) rebased/squashed them. The **tree content** of both is fully present
   (verified at symbol level + by test run below); only the commit graph lineage is rewritten.

**Verdict: the Round-6 claim is accurate.** One SUT shape (ComponentSet-described CapMap), one
catalog (`composed/catalog.rs`), two harnesses (headless tokio keystone + windowed gpui/tui-thread),
exactly the §8.10 end state. The `!has_actor(UI)` split is the PERMANENT two-harness decision
(thread affinity), not an unfinished fold.

---

## 2. Phase-4 remainder: env-selected ONE-PBT wiring parameterization for the windowed runners

### What already exists (don't re-plan it)

- **Headless keystone is fully parameterized.** `WideE2EMachine::init_state`
  (`composed/wide_e2e.rs:630`) draws `holon_pbt_core::any_valid_wiring()` per case;
  `HOLON_PBT_WIRING_AXES` (3 `;`-separated axes, `holon-pbt-core/src/wiring.rs:321`, parses
  `UI`/`MCPServer`/`ActionEngine` actors) overrides the axes; `HOLON_PBT_FORCE_FULL=1` pins
  `full_headless`. Shrinking minimizes BOTH steps and wiring (subset shrink toward Loro-only).
- `set_for_wiring` (`wide_e2e.rs:512`) normalizes a drawn wiring to a bootable headless
  `ComponentSet` — today it **strips `Actor::UI`** (headless-by-construction).
- `cap_set_for_wiring` (`wide_e2e.rs:531`) computes per-wiring cap sets via throwaway
  current-thread boot, cached by `Wiring`.
- `ComponentSet::needs_window()` (`component_set.rs:203`) — runner is DERIVED from wiring
  (ADR 0009 §5): `nUI` present ⇒ windowed. The dispatch mechanism exists; nothing consumes it yet.
- Windowed side: `WideE2EWindowedMachine` (`wide_e2e.rs:829`) exists but `init_state` **fixes**
  the oracle to a OnceLock cap set captured from ONE throwaway window boot
  (`set_windowed_cap_set`, gpui loop lines 171-178), narrowed by `narrow_to_windowed_alphabet`
  (6 disclosed EXCLUDED rows). No wiring draw, no env axes, windowed base hardwired to
  `full_headless` + overlay.

### The gap, concretely

The windowed runners run the SAME catalog and transition enum but a FROZEN wiring. "Collapse into
the single keystone" cannot mean one test fn (two-harness decision is permanent); it means the
windowed harnesses become thin env-parameterized hosts of the SAME `WideE2EMachine`-style drawn
wiring, with `Actor::UI` as a first-class drawn axis instead of a stripped one.

### Task breakdown

| # | Task | Key files | Effort |
|---|---|---|---|
| 4.1 | `set_for_windowed_wiring`: the `full_gpui`-shaped sibling of `set_for_wiring` that KEEPS `nUI` (+ FrontendBounds projection), still forcing Loro-when-no-Turso etc. Property test: identity on `full_gpui()`. | `wide_e2e.rs`, `component_set.rs` | 0.5–1 d |
| 4.2 | Per-wiring windowed cap-set derivation. `cap_set_for_wiring` fail-louds on UI actors (compose_sut asserts `!has_actor(UI)`); the windowed variant must derive cap sets per drawn wiring — either statically (headless cap set ∪ overlay caps, verified once against a live boot — cheap) or by throwaway window boot per distinct wiring on the gpui thread (expensive, ~10s each; cache by Wiring as the headless one does). Recommend static-∪-overlay + one live-boot assertion, disclosed. | `wide_e2e.rs`, `window_slice/builders.rs` | 1–2 d |
| 4.3 | `WideE2EWindowedMachine::init_state` draws wiring: replace the OnceLock fixed cap set with `any_valid_wiring()` filtered/normalized through 4.1, oracle = `wide_e2e_windowed_ref_for(&wiring)`. Keep `narrow_to_windowed_alphabet` applied per draw. Shrinking then minimizes the windowed subsystem set too. | `wide_e2e.rs:822-860` | 1 d |
| 4.4 | SUT-side: `with_windowed_wide_sut` / `drive_windowed_case` boot per-case from the DRAWN wiring (today they boot the fixed windowed base). Threads the `ComponentSet` through the window boot path (`compose_windowed_sut(set, geometry, engine, driver)` already validates a set). | `gpui_composed_windowed_loop.rs:92-144`, gpui `pbt_harness` | 1 d |
| 4.5 | Env selection of the runner: keystone axes default to no-UI (`HOLON_PBT_WIRING_AXES` without `UI`; today `set_for_wiring` silently strips UI draws — after 4.3 make the keystone's axes explicitly UI-free and delete the silent strip, fail-loud instead), gpui/tui runners pin UI present. One doc + justfile recipe (`just pbt-windowed` etc.). This is what makes it "env-selected ONE PBT": same machine, axes decide harness. | `wide_e2e.rs:512`, justfile, docs | 0.5 d |
| 4.6 | TUI runner parity: `pbt_main.rs::run` drives ONE sequence per invocation via a bespoke loop; fold it onto the shared strategy + multi-case proptest runner shape of the gpui 4b loop (reuse `drive_windowed_case`'s shape; TUI-specific renderer pump stays). | `frontends/tui/tests/common/pbt_main.rs` (431 lines) | 1–2 d |
| 4.7 | (Separate track, gates full-alphabet windowed) Un-exclude rung-audit rows 19–24: drive `SutNavHistoryWrite/Drive`, `SutViewControl`, `SutHistoryWrite` through real window gestures so `narrow_to_windowed_alphabet` (`wide_e2e.rs:778`) can die. | frontends + `driver_input.rs` | ~1 wk, independent |

**Total for the parameterization proper (4.1–4.6): ~5–7 focused days.** 4.7 is a distinct
Phase-3-blocker track and should not gate calling Phase 4 done — the narrowing is disclosed and
audit-sanctioned. Watch the case budget: windowed cases are ~9.8s each measured; wiring-drawn
windowed cases with per-case boots stay in that envelope only if 4.2 avoids per-case cap-set boots.

---

## 3. Triage of the three open reds (model divergence = prod-bug candidate first)

### (a) Toggle-state `task_state_category` gap — **ALREADY FIXED in this tree; was a REAL PROD BUG**

- **Round-6 diagnosis was right and prod-side**: org boundary `OrgBlockExt::set_task_state`
  (`holon-org-format/src/models.rs:545-563`) writes BOTH `task_state` + `task_state_category`;
  the prod cycle path wrote only the keyword, so a UI cycle dropped/staled the category (a DONE
  keyword could read back Active — user-visible ranking/filter corruption, not a test artifact).
- **Fix is present in the current tree at every write boundary** (commit lineage
  `8fa13da9cb`/`2a53aad479` "task_state_category paired at every write boundary", content merged
  via the Petri megamerge even though the commit isn't an ancestor):
  - SQL provider `set_field("task_state")` pairs the sidecar in the SAME UPDATE and `Null`
    removes both keys (`crates/holon/src/core/sql_operation_provider.rs:999-1033`);
    `cycle_task_state` routes through it (`:1153-1189`).
  - Loro `BlockCellRegistry::write_field` pairs it in the same commit
    (`holon-loro/src/block_cell_registry.rs:339-366`).
  - Loro CRUD provider `set_field` pairs it (`holon-loro/src/loro_block_operations.rs:259-282`).
  - Reader tolerates legacy bare-keyword data via `TaskState::from_keyword`
    (`models.rs:525-543`) — disclosed legacy fallback, panics on a CORRUPT category. Correct
    per fail-loud policy.
- **Verified empirically**: `cargo nextest run --lib --features pbt -E
  'test(wide_frontend_toggle_state_lockstep_stays_green)'` → **PASS (2.8s)**,
  log `/tmp/fable_toggle_state_test.log`. The Round-6 red is stale against this tree.
- **Action**: none code-wise. Strike it from the open-reds list; optionally re-run the full lib
  slice suite to confirm 157/157 (memory `wrapup_round2` claims exactly that).

### (b) TUI drawer-toggle geometry — **HARNESS MODEL BUG first, PROD FEATURE GAP second**

- **Failure mechanics** (all verified in-tree): the composed windowed alphabet draws
  `ToggleDrawer` because the ref's `LayoutRefState::drawer_handles`
  (`crates/holon-integration-tests/src/pbt/layout_bridge.rs:34-71`) is **hardcoded** to the
  default GPUI layout's two sidebars whenever `app_started` — frontend-blind. The transition
  clicks `drawer_toggle_id_for(block_id)` (`holon-layout-testing/src/transitions/toggle_drawer.rs:53-62`)
  → `DriverInputComponent::click_at_element` → TUI `click_entity` = nav_to + Enter
  (`frontends/tui/src/user_driver.rs:611-629`) → registry lookup fails: the TUI deliberately
  does NOT register the layout wrappers (`frontends/tui/src/render/mod.rs:399-405` — "containers,
  not user-facing rows") and renders **no drawer-toggle affordance at all** (no collapse feature;
  Tab region-hopping only, `app_main.rs:583`). Fail-loud panic at `driver_input.rs:511`. ✓ honest.
- **Is it prod?** Two layers. The IMMEDIATE bug is the ref model: it asserts a geometry fact
  ("this frontend renders toggleable drawers") that is GPUI-true and TUI-false. That's a
  harness/model divergence — the SUT correctly has no such widget. Underneath sits a real prod
  **parity gap**: TUI renders the left sidebar but offers no way to collapse it (GPUI does,
  `frontends/gpui/src/render/builders/drawer.rs` / `columns.rs`). Users of the TUI genuinely
  lack the feature.
- **Fix-the-cap-not-withhold faithful fix** (two steps, both needed):
  1. **Model honesty (unblocks the runner now, no withholding)**: derive `drawer_handles` from
     the live rendered frontend instead of the hardcoded list — walk the VM/registry for actual
     drawer-toggle widgets (exactly the TODO pattern already written for `switchable_handles`
     at `layout_bridge.rs:56`). TUI then yields `[]` → generator narrows via the existing
     `Validated::fail(NoDrawerHandles)` — a DISCLOSED structural narrowing (like
     `NoTogglableStates`), not a cap subtraction: `SutBlockInteract` stays fully present. This
     is the sanctioned analog of the cap_set-narrowing the windowed alphabet already does.
  2. **Prod parity (the real fix)**: implement sidebar collapse in the TUI — a registered,
     Enter-activatable toggle row per sidebar driving the SAME `set_widget_open` /
     `ui.tab.drawer_open` state through `app_handle_input_event`. Once it renders, step 1's
     derived handles automatically re-admit `ToggleDrawer` for TUI — the alphabet widens by
     itself. File it on the TUI track; don't block the Phase-4 work on it.
  - **Anti-pattern to refuse**: hardcoding a "TUI has no drawers" special case in the generator,
    or `.without::<SutBlockInteract>()` — both fake the surface instead of fixing the model.

### (c) Window-slice root-layout "ghost row" — **HARNESS ORACLE GAP (boot-scaffold under-modeling), not a prod bug**

- **Failure mechanics** (verified): `gpui_window_slice.rs::capmap_hosts_windowed_sutlayout_over_real_geometry`
  boots a REAL `TestEnvironment` + `start_app(true)` (line 121) — which seeds the bundled
  `index.org` layout incl. the well-known `block:root-layout`
  (`crates/holon-app/src/seed.rs:41`, `wiring.rs:311`). It then runs `run_selected` with
  `window_ref_caps()` — the "minimal honest oracle" whose block universe is EMPTY
  (`window_slice/builders.rs:235`). `inv-matview-consistent-with-ref/root_layout`
  (`composed/correspondences.rs:684-751`) is a subset check `matview_data_rows ⊆
  (all_block_ids ∪ layout_block_ids ∪ profile_block_ids)`; with an empty ref universe every
  legitimately-booted row is "extra" → GHOST ROW fires on `block:root-layout`.
- **Why it's NOT a prod-bug**: the matview row corresponds to a block that REALLY exists in
  SQL (the boot-seeded layout), at boot time, with no deletes having run — the matview is
  faithful; the ORACLE under-models boot. The prod-suspect class from the dogfood triage
  (stale rows after chained-matview/CDC DELETEs) is a different signature: transient rows for
  blocks that no longer exist. The keystone already runs this same invariant GREEN because
  `boot_and_seed_wide` seed-injects the booted scaffold ids into the oracle
  (`wide_e2e.rs:330-339` scaffold-union classification). The window-slice mini-ref never got
  that treatment. Pre-existing at `f92ca119`; unrelated to the Round-6 deletions. ✓
- **Faithful fix**: give `window_ref_caps()` the same scaffold classification the keystone
  uses — after `start_app`, read the booted layout/profile ids (the `RefLayout::layout_block_ids`
  / `profile_block_ids` channels the invariant already consults) and seed them into the mini-ref,
  reusing the keystone's scaffold-union primitive rather than a second hand-rolled list.
  **Refuse**: filtering `block:root-layout` out of the matview extraction or allowlisting ids
  inside `compare_no_ghost_rows` — that would blind the ONE ghost-row detector that exists for
  the real CDC-delete prod-suspect. Keep the comparator strict; fix the universe.
- **Residual watch-item**: once fixed, if the test STILL reds intermittently with ids that are
  NOT boot scaffolding, that IS the dogfood-triage CDC-delete suspect — promote to prod-bug
  immediately. The strict comparator is the net for it; don't soften it.

---

## Gates / evidence log

- `sut*.rs` deletion: `ls crates/holon-integration-tests/src/pbt/` (this session).
- `E2ESut` grep: comments only; sole E2E `StateMachineTest` = `composed/harness.rs`.
- Toggle-state test: PASS, `/tmp/fable_toggle_state_test.log` (nextest, 1/1, 156 skipped).
- No builds/tests beyond that single targeted test (read-only mandate).
