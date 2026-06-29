# PBT Composition — Endgame Roadmap (synthesized 2026-06-25)

Output of a 5-agent parallel research sweep over the remaining γ-composition migration.
**Process** lives in the `pbt-composition` skill; **architecture** in `PbtCompositionDesign.md`;
this file is a sequenced **implementation roadmap** (tracking-adjacent — update freely).

Each stream produced a full per-step plan with file+line refs; this is the synthesis + the
cross-stream dependency graph and the one open decision.

## Cross-stream dependency graph

```
ready NOW (read caps, scaffolding ↓, no deps)
├─ E3-1  delete SutSqlProjection off E2ESut   ── DECISION: split_block/peer_conflict (see below)
├─ E3-2  delete SutEditorMirrorRead off E2ESut  (clean: 2 self-bodies, no slice consumer)
└─ MUT-0 cargo-mutants first run (editor_caret.rs) + fix the 2 stale mutants.toml

then
└─ E4    GpuiDriverComponent drives real input (wrap UserDriver): host SutBlockInteract +
         SutArrowNavigate → admits PressKey/ArrowNavigate/drag; relocate
         run_windowed_composed_check out of E2ESut into composed/windowed.rs
   └─ unblocks E3-3..5  delete SutViewModel + SutLayout + SutDriver (windowed; delete together)

independent (anytime; first real sut_absent / negative-selection consumer)
└─ D     degraded "shows source" twin + SutQueryResults cap

terminal (needs E3 + E4 done)
└─ E5    env-parameterize WideE2E (the keystone net-new work; read HOLON_PBT_WIRING_AXES,
         compose_sut(set)) → repoint BisectionStepper to compose_sut → delete native runner
         (run_proxy_registry / native_*_invariants / WideProxyCaps) → delete Subsystem/min_sut/
         PbtSuiteSpec → delete E2ESut + write caps (SutLoro/SutBlockTreeWrite/SutEditorMirrorWrite)
         + all slices + subsystem_seed spike. parity.rs is the static no-loss gate, retired LAST.
   └─ unblocks OC

post-E5 (extensibility payoff — CapMap is the SOLE SUT, so the open dispatch flip is unblocked)
└─ OC    open module contribution (Design §5.5): flip transition dispatch → inventory/typetag open
         registry (§8.9 Tier 2) + ReferenceState private fields → open per-module extension registry
         → a new subsystem (a future Loro+Iroh P2P module, a Flutter/web frontend) ships as a
         crate/submodule with ZERO central edits. NET-NEW (not scaffolding↓).
```

## Stream summaries (critical files + the essential move)

**E3 — cap-deletion map.** 8 impls remain in `pbt/sut_capabilities.rs`. Deletability is gated by 3
type-level requirement sources in `invariant_runner.rs`: the `WideProxyCaps` bound (L489), the
`native_proxy_invariants` list (L612), the `native_self_invariants` list (L635). Remove a body
from the list → add its id to `NATIVE_ONLY_EXCLUDED` (L655) → drop the cap from `WideProxyCaps`
→ delete the impl → add an `E1_RELOCATED` row to `composed/parity.rs` (L75). Verdicts:
- **READ + full_headless-hosted → delete now:** `SutSqlProjection` (consumer `inv-navigation-focus`, already WIDE_REQUIRED), `SutEditorMirrorRead` (2 editor self-bodies, no slice).
- **READ + windowed → delete after E4 (NO-branch; composed slice `run_windowed_composed_check`
  already exists):** `SutViewModel` (coupled to SutLayout via 2 dual bodies), `SutLayout`, `SutDriver`.
- **WRITE caps → E5** (drive the alphabet over E2ESut): `SutLoro`, `SutBlockTreeWrite`, `SutEditorMirrorWrite`.

**E4 — windowed input.** The cap gate is value-level in `aggregate_transitions`
(`transition_dispatch.rs:449`): `required_wiring().satisfied_by(wiring) && state.caps_available(required_caps())`.
`SutBlockInteract` + `SutArrowNavigate` already carry `#[capmap_adapter]`; the gap is only that
`GpuiDriverComponent::send_raw_keystroke` (`window_slice/components.rs:483`) is stubbed. FIX: add a
`driver: Arc<dyn UserDriver>` field, implement `SutBlockInteract`/`SutArrowNavigate` as pure
forwards to the production `UserDriver` (mirror `sut_handle.rs:347-521`), register them, add a
`window_input_wide` builder. Then PressKey/ArrowNavigate/drag stop auto-narrowing. **Backlog phrasing
"add GPUI axis to HOLON_PBT_WIRING_AXES" is STALE** — windowing is modeled via the windowed
`ComponentSet`/`has_window`, not a new wiring token; model it via the component list (recommended 3b).
Relocate `run_windowed_composed_check` (`invariant_runner.rs:375`, duplicated in
`sim_windowed_replay.rs:799`) into a shared free fn — scaffolding ↓.

**E5 — terminal deletion.** Keystone net-new work: env-parameterize `WideE2E::build` /
`boot_and_seed_wide` (`wide_e2e.rs:181`) to read `wiring_axes()` and `compose_sut(set)` (builder
already takes a `ComponentSet`) — this is "start with just Loro → fast test." Keep + repoint the
bisector (`BisectionStepper` → `compose_sut`) — the subsystem-set minimizer survives. Keep the
`HOLON_PBT_INVARIANTS` toggle (rewire onto `run_selected`). Delete: native runner core, legacy
`Subsystem`/`min_sut`/`PbtSuiteSpec` (`registry.rs`), `E2ESut` + caps, all slices + `subsystem_seed`
spike (the known B5 `block:journals` decay dies with the spike). **§9 risk:** `parity.rs`
selection-diff is the deletion gate — run it before E5-delete steps, retire it LAST.

**Bundle D — degraded twin (first real `sut_absent`).** `SutQueryResults` is provided by the
existing Turso-backed `HeadlessFrontendComponent` (reads real `ReactiveRenderedRows` — honesty gate
passes); its ABSENCE is a new no-Turso `BlockQueryFrontendComponent` over
`holon_app::from_block_query_source` (real `source_editor` degraded render via
`loro_ui_watcher.rs::derive_render_expr`). Full half = edit `viewmodel_decompiled_rows_match_query`
to add `SutQueryResults` to `sut_present`; degraded half = new `viewmodel_shows_source_when_no_query`
with `sut_absent: [SutQueryResults]`, `ref_present: []` (§5.2: the Ref must NOT model query results
or you manufacture false divergence — assert SUT-internal `root_render_kind == "source_editor"` only).

**MUT — cargo-mutants quality gate (the new philosophy's actual teeth).** Two stale `mutants.toml`
(`.cargo/mutants.toml` + `crates/holon/mutants.toml`) pin `petri_e2e_pbt` — wrong test. Add a
workspace-root `mutants.toml` gating on `general_e2e_composed_pbt`, `exclude_globs` the entire PBT
instrument (`holon-pbt-core`, `holon-integration-tests`, macros), `examine_globs` start at
`holon-frontend/src/editor_caret.rs`. First run: mutate editor_caret against the editor lib slices
(Tier A, seconds/mutant). Survivor → work (Recipe 1: widen a generator or add a `wire()`), never a
tautological assertion. `general_e2e_composed_pbt` hardcodes `cases:16` — add a `PROPTEST_CASES`
env-honor edit so mutation runs can drop cases; add the binary to `.config/nextest.toml`'s 20-min
override (else 2-min cap → false TIMEOUT="caught").

**OC — open module contribution (post-E5; Design §5.5).** The endgame's *extensibility* payoff,
distinct from deletion: let a new subsystem (a not-yet-existing `Loro+Iroh` P2P module; a future
Flutter/web frontend) ship its generators + transitions + invariants + SUT component **+ its private
reference-state** as a self-contained crate/submodule with **zero central edits**. The correctness
rule "one coupled core" constrains only *shared* data — it never mandated a *closed* `ReferenceState`;
that's an implementation fact, not a §5.1 consequence. Two enablers, both unblocked once E5 makes
`CapMap` the **sole** SUT:
- **(a) Flip transition dispatch to the open registry.** The closed `E2ETransition` enum +
  `aggregate_transitions` are load-bearing *only* while E2ESut+CapMap coexist. `cap_transition!`
  (§8.9, `transition_dispatch.rs`) is the authoring seam already in tree (split_block/nothing
  migrated); the Tier-2 `inventory`/`typetag` open encoding (`experiments/open-registry-poc/`) needs
  E5 (a trait object must erase to one SUT type). The flip is one macro body, never a 52-file rewrite.
- **(b) `ReferenceState` private fields → open per-module extension registry.** The ref is *already*
  `impl CapProvider` (§8.8, `reference_capabilities.rs`) — a typemap that *can* host multiple
  providers. Work: relocate subsystem-*private* state off `reference_state.rs`'s hardcoded fields into
  a per-module extension registry; a module registers its private state + impls of **its own** `Ref*`
  traits **for** `ReferenceState` (orphan rule permits local-trait-for-foreign-type). The coupled
  cross-cutting core (block/editor/focus, single-homed) stays — only private state moves. A module
  hands *intent* to the core's write interface; the core (commute/LWW/Loro-from-intent, §5.4) computes
  shared-data outcomes, so the module needs no merge knowledge.

**Boundary (§5.5):** additive subsystems whose new ref-state is *private* only (P2P peers/clocks; the
synced tree is already core). A module changing the *shared semantics* of existing core data still
edits the core — a real coupling no registry should hide. **Litmus (≠ §8.10):** OC's success test is
"a new subsystem **adds** files and edits **none**," not "scaffolding ↓" — it is net-new extensibility,
not deletion.

## The one cross-stream DECISION

`tests/split_block_content_pbt.rs` + `tests/peer_conflict_pbt.rs` consume `SutSqlProjection` over
E2ESut and block E3-1. The E3 stream says **delete them** (§8.10 YES-branch — composed-covered).
The E5 stream notes they carry **unique fixture/gherkin regression replays** worth preserving via
`impl FixtureAssertable for ComposedSut<S>`. Per memory `composed_gherkin_bridge_2026-06-25`, that
bridge **already exists** → migrating is cheap and keeps real past-bug coverage. **Recommendation:
migrate, don't delete** (regression replays test the SYSTEM, not an invariant — not "tests-of-tests").
Decide before E3-1.

## Recommended implementation order
1. **Now, parallel (worktree-isolate — these touch shared `invariant_runner.rs`/`parity.rs`):**
   E3-2 (SutEditorMirrorRead, cleanest), MUT-0 (config fix + editor_caret first run).
2. Resolve the split_block/peer_conflict decision → E3-1 (SutSqlProjection).
3. E4 (windowed input) → then E3-3..5 (windowed caps).
4. Bundle D (independent, anytime).
5. E5 (terminal), gated on E3+E4, `parity.rs`-diff before each delete.
6. **OC (post-E5 extensibility)**, gated on E5 (CapMap sole SUT): (a) flip dispatch to the
   `inventory`/`typetag` open registry, (b) `ReferenceState` private fields → open extension registry.
   Acceptance: add a new throwaway subsystem module *out-of-tree* and wire it in with zero edits to
   `reference_state.rs` / the transition enum.

---

## Round 1 + 2 outcome (LANDED — all squashed into jj `kqmowyup` / git `4527c93e`)

> **Provenance note (verified 2026-06-27):** the Round-1/2 work was **squashed into the single
> commit `kqmowyup`** (`jj squash --from @ --into kqmowyup`) and the per-stream divergent revs
> (`zqurskxy`/`oyyzwtnk`/`wxtznwwy`/`mvmmpoqr`/`xspwmswn`) were **abandoned** — so those rev names
> no longer resolve, but every artifact is present in the tree. Confirmed in code: root
> `mutants.toml` exists; `impl SutViewModel/SutEditorMirrorRead/SutSqlProjection for E2ESut` are
> all gone; Bundle D twin `viewmodel_shows_source_when_no_query` exists; `peer_conflict_pbt.rs`
> was deleted (the cross-stream decision resolved — `split_block_content_pbt.rs` kept/migrated to
> `ComposedSut<WideE2E>`, peer_conflict dropped).

Streams landed: E3 (SutEditorMirrorRead + SutSqlProjection, −2 tests) · E4 (real windowed input via
UserDriver; `run_windowed_composed_check` single-sourced) · Bundle D (degraded twin) · cargo-mutants
gate · E3 SutViewModel. Tree compiles green (`cargo build --lib --features pbt`); only pre-existing
`block:journals` scaffolding reds remain.

**Caps still on E2ESut:** `SutLoro`, `SutBlockTreeWrite`, `SutEditorMirrorWrite` (write/apply),
`SutLayout`, `SutDriver` (windowed shell). **`SutViewModel` deleted Round 2.**

## Round 3 (2026-06-27) — PARTIAL E5 step-1 **SUT-side seam** landed (behavior-preserving)

The SUT-side parameterization seam for `WideE2E` is in place (`composed/wide_e2e.rs`), proven
identical for `full_headless` so it cannot perturb today's keystone run:
- **`set_for_wiring(&Wiring) -> ComponentSet`** — the wiring→headless-set normalizer (strip
  `Actor::UI`; force `Loro` when Turso absent, mirroring `storage_selector_for_wiring`; `ViewModel`
  only with Turso, `EditorState` always). Idempotent; `set_for_wiring(&full_headless().wiring) ==
  full_headless()` (unit-tested).
- **`cap_set_for_wiring(&Wiring) -> CapSet`** — per-distinct-`ComponentSet` cached boot (linear-scan
  cache; `Wiring` is `Eq`-not-`Hash`). `full_headless_cap_set()` is now a thin alias over it.
- **`boot_and_seed_wide` now reads `set_for_wiring(&ref_state.wiring)`** for the SUT (was hardcoded
  `full_headless()`).

### Round 3b (2026-06-27) — seed generalization ✅ DONE + ref generalization

Sub-steps 1 and 2 below are now **landed and validated** (Loro-only lib test + full_headless keystone
PASS 290s):

- **Seed (sub-step 1) — done WITHOUT leaking the backend.** `compose_sut_seeded` gained a
  `seed_tree: &[NewBlock]` param: the **builder** creates the working tree directly into the canonical
  Loro backend (`CoreOperations::create_block`) for non-frontend configs — symmetric with the frontend
  org-boot, backend never escapes the builder (an earlier `pub loro_backend` field on `ComposedSut` was
  reverted — the CapMap's caps are read/edit-only, there is no create cap, so the initial-tree seed is
  a *boot* concern the builder owns). `boot_and_seed_wide` passes `wide_seed_tree()` (frontend ignores
  it; Loro-only seeds from it). Key insight: `set_for_wiring` makes **Turso ⟹ ViewModel ⟹ frontend**,
  so the only non-frontend config reachable is **Loro-only** — one seed path, not three.
- **Scaffold union (the `block:journals` fix).** `boot_and_seed_wide` now returns
  `scaffold = (booted ∪ ref_block_ids) − {parent,c1,c2}`. A non-frontend SUT lacks the
  oracle-modeled boot layout (journals/index.org from `build_started_ref`'s `seed_booted_layout_into_ref`),
  so those ids must come from the **ref side** to be seed-injected and filtered — otherwise they
  false-diverge (`block:journals present in ref but missing from SUT`). For full_headless the union is a
  no-op (ref ⊆ booted), so the keystone is unchanged.
- **Initial focus gated** to frontend configs (a Loro-only SUT has no `SutFocusWrite`).
- **Ref (sub-step 2) — `wide_e2e_ref_for(wiring)` added** (behavior-preserving; `wide_e2e_ref` = thin
  alias). The ref-side subsystem wiring **stays `wide_ref()`'s `{Loro, EditorState}` for every draw** —
  it turned out NOT to need per-wiring tuning: the editor is always present (`set_for_wiring` always
  adds `EditorState`), focus/Turso invariants are SUT-cap-gated out, and the editor is closed by the
  boot's `NavigateFocus(page)` blur. Only `wiring` + `cap_set` vary per draw.

### Round 3c (2026-06-28) — sub-step 3 ✅ DONE + `init_state` FLIPPED (keystone parameterized)

3. **`required_invariants` is now per-draw (✅ DONE).** The non-vacuity floor moved from a static
   `const REQUIRED_INVARIANTS` consult to a `ComposedSlice::required_invariants(&ReferenceState)`
   trait method (default = the full static const, typed as `Vec<InvariantId>`). `WideE2E` overrides it
   to intersect `WIDE_REQUIRED_INVARIANTS` with the invariants the draw's caps can actually select:
   for each id it looks the invariant up in `composed_invariant_catalog()` and keeps it iff
   `Needs::selected_against(sut_caps, ref_caps)` holds — the SAME selector the runner uses. The SUT
   cap_set is the draw's EXPECTED set (`ref_state.cap_set`, carried by `wide_e2e_ref_for`), so the
   floor keeps teeth: a wiring that claims a cap the boot fails to wire → required-but-deselected →
   floor REDs. The returned ids are **parsed `InvariantId`s sourced from the catalog** (`inv.id()`),
   not raw strings — the const string is just a selector validated against the live registry (the
   `panic!`-on-miss is the parse). `check_invariants` compares against `report.ran` (typed), not
   `ran_ids()` strings. (`harness.rs` ComposedSlice + check loop; `wide_e2e.rs` override.)

   **`init_state` FLIPPED** — draws `any_valid_wiring()` → `wide_e2e_ref_for(w)` (was fixed
   `Just(wide_e2e_ref())`). The keystone now exercises the FULL valid-wiring space each run, shrinking
   toward Loro-only.

   **Validated:** `general_e2e_composed_pbt` PASS at 16 cases in **46.9s** (down from the ~290s
   all-`full_headless` baseline — most draws are now cheap Loro-only), 6-case smoke PASS, and a forced
   `HOLON_PBT_WIRING_AXES="Loro;;"` run (EVERY draw Loro-only `{Loro, EditorState}`, no SQL/ViewModel
   caps) PASS 8/8 in 6.8s — the airtight proof the floor narrows (under the old static floor all 8
   would hard-fail on `inv-block-content-matches-ref`). The pre-existing `block:journals` reds on the
   non-frontend lib slices (`memory_slice::structural_pbt`) are UNCHANGED — they fire the oracle-
   divergence assert (`harness.rs` `failures().is_empty()`), untouched by this floor work, and die
   with the B5 `subsystem_seed` spike in E5.

**§8.10 scaffolding-DOWN payoff — first deletion DONE (2026-06-28):** `subsystem_convergence_pbt`
(the E2ESut-backed convergence harness) + its now-dead `declare_pbt_convergence!` macro (`slice.rs`)
DELETED — the parameterized keystone subsumes it (both generate+shrink the wiring; the keystone over
the `CapMap`, the north-star target SUT). Stale "covered by subsystem_convergence_pbt" comments
repointed to `general_e2e_composed_pbt` (`invariant_runner.rs` NATIVE_ONLY_EXCLUDED, `registry.rs`,
`block_ids_match_ref.rs`, `wiring.rs`, `invariant_dispatch_smoke.rs`). Gate `parity.rs`
(`composed_catalog_covers_e1_relocated_caps`) + keystone both PASS post-delete. `run_invariant_registry`
(native runner core) is STILL used by 7 src files → it stays until the headless slices die.

  **`general_e2e_pbt` subsumption AUDIT (2026-06-28, live `swap_probe_full_headless_narrowed_alphabet`
  probe at `builder.rs:959`):** the composed keystone narrows out only **7 of 53** `E2ETransition`
  variants, across **two cap families**: `SutSeamMutate` (ApplyMutation, BulkExternalAdd) +
  `SutFixtureFs` (WriteOrgFile, CreateDirectory, GitInit, JjGitInit, CreateStaleLoro). The keystone
  header (`general_e2e_composed_pbt.rs:11-16`) is **STALE** — peer/E4-gesture/watches/ToggleState are
  ALL already un-narrowed and driven by the composed keystone (peer: `builder.rs:572`; E4 via the
  frontend `ReactiveEngineDriver` `builder.rs:240`; ToggleState=`SutMutate`, watches=`SutWatchRegister`
  both feasible). `general_e2e_pbt_sql_only` adds nothing the keystone's Turso-only `any_valid_wiring`
  draws don't already cover (DELETE it together with the full variant).

  **Verdict: NOT cleanly deletable yet.** The unique residual coverage is PBT-style *interleaving* of
  external-org writes (`SutSeamMutate`) with random editor/nav sequences (CDC echo-suppression /
  file-sync races) — example-based `bidirectional_sync.rs` / `phantom_loro_exists_repro.rs` cover the
  seam functionally but not interleaved. `GitInit`/`JjGitInit` (`SutFixtureFs`) is a real but negligible
  gap (pre-startup, sets a ref flag, 0 SQL). **§8.10-clean path:** host `SutSeamMutate` (+ optionally
  `SutFixtureFs`, dropping git/jj) on the composed CapMap → un-narrows those families → THEN delete
  `general_e2e_pbt(_sql_only)`. (`local_caps.rs:30` already frames this convergence.) Then the native
  runner core (see "Refined E5 plan").

## ★ Key finding (Round 2 / E5 research): windowed-shell deletion is DEFERRED, not a permanent fork

`SutLayout` + `SutDriver` are **not** mere read caps — they are load-bearing for E2ESut's
**windowed input / transition-apply shell**: `apply_split_block_input_pipeline_to_sut<S: SutLayout
+ SutDriver>`, `SutBlockInteract` (click/drag/slash/press_key), Gherkin `widget_contains` fixtures
(`fixtures/assert.rs`), and `sut_check_invariants::engine_focused_block`. They differ from every
other cap because they need live geometry + real platform input (keymap, coord hit-test, drag
bounds) that a headless reactive tree has no equivalent for. Today `compose_sut` asserts
`!has_actor(UI)` (`builder.rs:91`), so the windowed checks run through a *separate* runner
(`run_windowed_composed_check`) with no `SutBackend`.

**One SUT *shape*, but two *harnesses* — by thread-affinity necessity** (decision 2026-06-26,
refining the 2026-06-25 "GPUI axis" call). `UserDriver` stays the single input interface —
`ReactiveEngineDriver` (headless) and `GpuiUserDriver`/`SimUserDriver` (windowed) were never forked;
`GpuiDriverComponent` only *wraps* whichever production `dyn UserDriver` the window installs and adds
a geometry precheck + cap gating (`window_slice/components.rs:835-848`). The windowed `CapMap` is
**already** a `Config`-composed artifact: `window_input_wide(geometry, engine, driver)` (now wrapped
by the set-validated `compose_windowed_sut`) and `run_windowed_composed_check` already run the **one
shared catalog** via `run_selected` over it. So the GPUI-axis composition *exists*.

What does **not** unify is the construction entry: `compose_sut` boots its components on the **tokio
runtime** (`.await`), but a GPUI window has **thread affinity** — it must be launched on the gpui
thread by the windowed harness, which then hands its live `geometry`/`engine`/`UserDriver` to
`compose_windowed_sut`. You cannot `.await`-launch a window inside `compose_sut`; its `!has_actor(UI)`
assertion (`builder.rs`) is therefore **correct and permanent**, not a deferral — it fail-louds the
unbuildable headless path and points at the sibling entry. Likewise the windowed StateMachineTest
**harness** (window launch + gpui-thread loop) is thread-bound and **cannot** fold into WideE2E's
tokio loop. End state: **one SUT shape** (a `ComponentSet`-described `CapMap`) and **one catalog**,
driven by **two harnesses** (headless tokio + windowed gpui-thread). What *deletes* is E2ESut's
windowed **cap impls**, not a second SUT shape:

- **PARTIAL E5 (do next, no new blockers):** env-parameterize `WideE2E` + repoint the bisector, then
  delete the **headless** native runner core, the legacy `Subsystem`/`min_sut`/`PbtSuiteSpec`
  selection, and the headless scaffolding slices + `subsystem_seed` spike (this retires the 11
  pre-existing `block:journals` reds). The windowed `E2ESut` shell survives as the last tenant.
- **Close E4's `expand_toggle`/`collapse_toggle` gap — ✅ DONE (2026-06-26):** `UserDriver::set_block_expanded`
  drives the chevron's view-local `Mutable<bool>` honestly per frontend (headless gate-flip; windowed
  real chevron click). `GpuiDriverComponent` no longer `unimplemented!()`.
- **Repoint the windowed transition-apply off E2ESut (the real remaining blocker):** route
  `apply_split_block_input_pipeline_to_sut` (`sut_capabilities.rs:871`), `engine_focused_block`, and the
  Gherkin `widget_contains` fixtures through the windowed `CapMap`'s caps instead of `<E2ESut as
  SutLayout+SutDriver>`. The windowed harness still **owns** the window lifecycle + drives the loop;
  it just delegates all cap-work to the composed `CapMap`.
- **Delete E2ESut's `SutLayout` + `SutDriver` cap impls** once nothing consumes them. The windowed
  harness survives (thread-bound, owns the window); only the monolithic cap impls die — §8.10 litmus
  (scaffolding goes DOWN) holds.

## Refined E5 plan (verified vs integrated code; full detail from the E5 research agent)

1. **Env-parameterize `WideE2E`** (`composed/wide_e2e.rs` `init_state`/`wide_e2e_ref`/
   `boot_and_seed_wide`): draw `any_valid_wiring()`, map via a new `set_for_wiring(&Wiring)` →
   `ComponentSet`. **Gotcha:** add `Projection::ViewModel` ONLY when Turso is present (`compose_sut_seeded`
   asserts `!has_frontend || has_turso`) — Loro-only draw → `[Loro, EditorState]` = the fast path.
2. **`WIDE_REQUIRED_INVARIANTS` must become wiring-conditional** (per-draw intersection) or it
   false-REDs the non-vacuity floor on a Loro-only draw.
3. **`HOLON_PBT_INVARIANTS` toggle is NET-NEW work**, not a free survivor: it's consulted only in the
   native `run_one`, never in `run_selected`. Thread it via `composition.rs` `Config` (or a wrapper)
   BEFORE deleting `registry.rs`, or explicitly drop it.
4. **Repoint `BisectionStepper`** (`stepper.rs`) → `compose_sut(set_for_wiring(w))` + `run_selected`
   for headless draws (removes its last `E2ESut` dependency). UI draws go through the windowed harness
   + `compose_windowed_sut`, not the bisector's headless loop.
5. **`parity.rs` is the deletion gate** (`composed_catalog_covers_e1_relocated_caps`) — run before
   every delete, retire it LAST (it depends on the very native machinery being deleted).
6. Deletion order (PARTIAL E5): native runner core → `Subsystem`/`min_sut` (+ the `structural_ref_wired`/
   `structural_pbt.rs` consumers in the same step) → headless slices + `subsystem_seed` (relocate
   `build_started_ref` out first; `wide_e2e.rs` imports it) → `subsystem_convergence_pbt`.
7. **Windowed cap-impl deletion (after PARTIAL E5; NOT a fork-collapse — two harnesses are permanent):**
   the `compose_sut` `!has_actor(UI)` assertion **stays** (thread affinity — a window can't boot on
   tokio). The windowed `CapMap` is built by `compose_windowed_sut(set, geometry, engine, driver)`
   from the gpui-thread harness's live handles (DONE 2026-06-26). Remaining: ✅ `set_block_expanded`
   verb (DONE) → repoint `apply_split_block_input_pipeline_to_sut` (`sut_capabilities.rs:871`),
   `engine_focused_block`, and the Gherkin `widget_contains` fixtures off `<E2ESut as SutLayout+SutDriver>`
   onto the windowed `CapMap`'s caps → delete E2ESut's `SutLayout` + `SutDriver` **cap impls**. The
   windowed **harness** survives (thread-bound, owns the window lifecycle + drives the loop); only the
   monolithic cap impls die. `UserDriver` is never forked; `ReactiveViewModel` is the shared render
   tree, not a competing driver. (See Design §8.10.)

## ★ Layer-localization — reframes the windowed-shell work (2026-06-26)

A design pass (full model in Design §8.11) reframes the `SutLayout`/`SutDriver` work as one rung of a
broader, higher-value goal: **localize a bug to an interaction layer** (geometry vs view-model vs
engine), reusing the existing subsystem bisector with **no new dimension**.

Key realization: the three drivers form a faithful-refinement ladder — `GpuiUserDriver` ⊐
`ReactiveEngineDriver` ⊐ `DirectUserDriver` — and they **are subsystems**. Drive each transition
against the **highest available** `UserDriver`; ordinary subsystem bisection peels UI then ViewModel
(ordered by the `Actor::UI ⟹ ViewModel` validity), the driver auto-descends, and the layer where the
pinned failure stops reproducing localizes the bug. This is *more* correct than an independent driver
axis (which would allow unfaithful "drive-low-while-high-present" configs that fabricate bugs).

User directive driving it: **the PBT drives user interactions through the UI-adjacent logic layer
(`ReactiveEngineDriver` = the same `find_click_intent` + `InputState`/`MutableText` editing prod runs),
never `OpDispatchWriter` dispatch — even headless**; the dispatch floor exists only as the diagnostic
bottom rung.

**Consequence for the in-flight work:** the windowed driver-backed component generalizes into the
**VM rung** — a `UserDriver`-backed input component over **any** `dyn UserDriver` + **optional**
geometry, so the *same* component backs headless (`ReactiveEngineDriver`, no geometry) and windowed
(`GpuiUserDriver`, geometry). Step 1 (geometry-optional `with_input_headless` + headless-safe bounds
precheck) is **DONE** (jj working copy). Next: wire a `ReactiveEngineDriver` into `compose_sut` via
this component (admits UI-gesture transitions headless, driven UI-adjacently), then transitions prefer
the driver caps over `SutBlockTreeWrite`, then the `DirectUserDriver` dispatch floor + bisector
signature-pin. Tracked as **LL-1..5** in the Backlog.
