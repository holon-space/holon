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

### Round 3d (2026-06-29) — `ApplyMutation` SOURCE-ROUTING prototype ✅ (Loro arm) + a sanctioned exception to "one cap per transition"

`ApplyMutation` multiplexes 4 ingress SOURCES (UI / External-org / Action / LoroPeer) that, on the
split-component composed `CapMap`, target DIFFERENT caps (driver / file-seam / `SutLoro`). Rather than
split it into per-source transitions, we kept it as ONE transition with `source` as a **shrinkable
field** and route `source → cap` at the transition level — so the subsystem/source shrinker can
**localize** "diverges via org but not via Loro" (the same delta-debug mechanism as the driver ladder,
Design §8.11, but for a *categorical ingress axis*, NOT a fidelity ladder — there is no "highest
available" source). **DECISION:** the skill's "exactly one cap per transition" is a guideline, not a
framework rule (`required_caps()` returns a `Vec`); a **source-routed transition** is a sanctioned
exception. The routing lives in a hand-written `impl SutApplyMutation for CapMap` — the one place that
can reach every sub-cap (`self.expect::<dyn …>()`); `impl … for E2ESut` is a **no-op** (its
`block_tree_post_action` seam still owns the work, so E2ESut is byte-for-byte unchanged).

**Landed (prototype, Loro arm):** new `SutApplyMutation` trait (`transitions/apply_mutation.rs`) +
E2ESut no-op impl + CapMap routing impl (LoroPeer → `SutLoro` apply_peer_*); `ApplyMutation` bound
flipped to `SutApplyMutation`; `required_caps` → `[SutLoro]` (gate = the implemented arm); generator
gates the UI/External/layout/profile arms behind `!composed` (`state.cap_set.is_some()`) so the
composed alphabet draws only the implemented LoroPeer arm; `SutApplyMutation` added to the `SutHandle`
bundle; guard + builder cap-feasibility tests flipped. **Validated:** keystone PASS on `Loro;;` axis
(24 cases, 5 confirmed composed LoroPeer routes — non-vacuous) AND default mixed-wiring axis; guard +
3 builder/cap tests green.

**External (org) arm + `BulkExternalAdd` — IMPLEMENTED, non-regressive, but NOT YET EXERCISED
(2026-06-29).** Per the "don't proliferate traits" review: the External arm reuses the EXISTING
`SutSeamMutate` (its `apply_mutation(event)` has the same signature as a bespoke cap would) rather than
a new trait — and implementing `SutSeamMutate` fully on `HeadlessFrontendComponent` (both
`apply_mutation` = External org-write + `bulk_external_add` = born-with-ids write) ALSO un-narrows
`BulkExternalAdd` (its gate is `SutSeamMutate`). Both methods: resolve mutation ids (oracle→SUT) →
`Mutation::apply_to` on `all_blocks()` → rewrite the seeded USER docs (`documents` excludes
`index.org`, so a full rewrite is safe) → settle via the live `FileSyncController`. Registered on the
frontend (`components.rs` `CapProvider`); CapMap routing's `External` arm calls
`self.expect::<dyn SutSeamMutate>().apply_mutation(event)`; generator gates the composed External arm
on `seam_present`. Keystone stays GREEN (default, Loro+Turso, and forced-full axes) with both hosted;
`full_headless_capset_admits_toggle_apply_mutation_and_bulk` confirms cap-feasibility.

  **✅ BLOCKER CLEARED + KEYSTONE GREEN (2026-06-29).** Enabling the External/Bulk arms on the
  keystone surfaced **six** sequential defects in the composed headless seam (each exposed by the
  prior fix), all now fixed; `general_e2e_composed_pbt` is GREEN on **both** axes — default
  (272s/24 cases) and `HOLON_PBT_FORCE_FULL=1` (416s/24 cases), 0 divergences — exercising the full
  External `ApplyMutation` (org), `BulkExternalAdd`, and `CreateDocument`+`BulkExternalAdd`-to-a-fresh-doc
  combination. The six fixes:
  1. **Wide ref registered NO document.** `structural_ref_wired` (`wide_e2e.rs`) left
     `state.files.documents` EMPTY; both arms gate on `!files.documents.is_empty()`, so they never
     generated. **Fix:** `files.documents.insert(page_root(), "structural-page.org")` — key = the SUT
     doc key (org parser maps `#+ID: structural-page` → `EntityUri::block("structural-page")` =
     `page_root()` = `HeadlessFrontendComponent.documents` key).
  2. **Seam-mutate org rewrite WIPED the tree.** `apply_mutation`/`bulk_external_add` rebuilt the doc
     org from `all_blocks()` = base `block_raw` (**no `tags` column**), losing the doc's `Page` tag →
     `blocks_by_document` found no page → wrote an EMPTY org → re-ingest wiped the tree. **Fix:** source
     from the `block` MATVIEW (`live_block_snapshot()`), which carries tags/requires.
  3. **Per-tick reconcile mismatch on born-equal Creates.** An External `Create` writes the block WITH
     its oracle id (`:ID:` drawer), so the SUT mints the SAME id the oracle holds; the harness counted it
     as a newly-minted real with no matching synthetic and panicked. **Fix:** `harness.rs` reconcile
     excludes `real_new` ids already in `ref_state.blocks` (born-equal — like the existing peer-id skip).
  4. **`inv-displayed-text` vs SOURCE blocks.** `generate_mutation`'s `create_source` arm makes code
     blocks (`content_type=Source`); a source block correctly renders as an execution-result widget
     (`text "[no result]"`), but the invariant assumed displayed-text == raw content for *every* text
     widget. **Fix:** both displayed-text arms skip non-text blocks via the existing `is_text_block` ref
     cap (`displayed_text.rs`). (Surfaced only now because the composed headless keystone runs
     displayed-text over a real reactive widget tree.)
  5. **Created docs not tracked.** `BulkExternalAdd`/External can target a `CreateDocument`-minted doc
     (`block:ref-doc-N`), but `self.documents` was boot-fixed → "no file for doc" panic. **Fix:**
     `documents` is now `Mutex<…>` and `create_document` appends the new doc (its title==stem page id =
     the same real id the harness reconcile maps `ref-doc-N` to).
  6. **`/org` missed created-doc files.** `org_block_snapshot` (the `/org` SUT reader) iterated the
     boot-fixed `org_paths`, never the created-doc files → bulk blocks reached `block_raw` but not the
     on-disk `/org` comparison. **Fix:** read `union(org_paths, tracked-document-paths)`.
  Also **removed** the speculative `settle_viewmodel_content` helper (added mid-debug for a text-block VM
  lag that never actually occurred — source blocks were the real cause; it also stalled ≤25s/tick).

  Org-vs-Loro differential is now LIVE → host nothing more.

  **✅ DELETED `general_e2e_pbt(_sql_only)` (2026-06-29, user-approved).** Removed
  `tests/general_e2e_pbt.rs` (both `component_pbt!` variants over E2ESut: `full_headless` +
  `sql_only`) + its `.proptest-regressions`. Audit (Round 3c) had found only `SutSeamMutate`
  (now hosted + green on the keystone) and `SutFixtureFs` (git/jj, negligible) unique to it; the
  parity gate (`composed_catalog_covers_e1_relocated_caps`) stays green. Repointed the canonical-PBT
  policy to the keystone: CLAUDE.md rule, `wiki/entities/holon-integration-tests.md`, TODO.md, the
  justfile `general` recipes (`--features pbt --test general_e2e_composed_pbt`), and the
  `multi_peer.rs` doc comment. **§8.10 ledger: −2 tests − 1 regressions file, +0 scaffolding (DOWN).**
  Lingering `general_e2e_pbt` *string* refs remain only as (a) the native `PbtSuiteSpec` name and
  (b) explanatory comments — both belong to the native-runner core that the NEXT §8.10 step deletes.

## Round 4 (2026-07-01) — Partial-E5 HEADLESS dissolution landed; runner CORE is WINDOWED-BLOCKED

Executed the headless half of "Partial E5" (jj worktree `pbt-substep3-perdraw-floor`, on top of the
#1 fix+refactor: External/Bulk on the keystone + `general_e2e_pbt` deleted). Committed increments:

- **2a** — deleted `tests/cross_frontend_pbt.rs` + `run_phased_pbt_sync` + orphan `gpui_ui_pbt*.regressions`.
- **2c** — deleted `extended_gen_pbt.rs` + `layout_override_pbt.rs` (component_pbt! twins); env sweeps
  survive via the SHARED generators + new justfile `pbt-extended-gen`/`pbt-layout-override` recipes
  (keystone under `HOLON_PBT_EXTENDED_GEN`/`HOLON_PBT_LAYOUT_OVERRIDE`); layout_override KNOWN RED disclosed.
- **2d** — repointed `BisectionStepper` onto `ComposedSut<WideE2E>` (+ bisect_driver: seed `ref0` via
  `wide_e2e_ref_for`, divergence signature → the composed marker). Probe-verified.
- **step 3** — threaded `HOLON_PBT_INVARIANTS` (disclosed warn/skip softening) into the composed
  `ComposedSut::check_invariants` via a relocated `pbt/invariant_mode_override` module (survives the
  native-registry deletion). Keystone green under the env.
- **4a** — deleted the now-dead slice-macro machinery: `slice.rs` all `macro_rules!` (declare_pbt_slice!/
  component_pbt!/…, 807 lines) + `stepper.rs` HeadlessTest/SmtStepper/run_via_state_machine_test/
  GpuiReplayStepper.

**⚠ HARD BLOCKER on the runner CORE (steps 4-core + 5).** `run_invariant_registry`, `phased.rs` windowed
entry points, `impl StateMachineTest for E2ESut`, `Subsystem`/`min_sut`/`PbtSuiteSpec`, and `parity.rs`
are STILL LIVE — driven by the WINDOWED gpui/tui replay harnesses in `frontends/gpui/tests/pbt_harness/*`
+ `frontends/tui/tests/common/pbt_main.rs` (`run_pbt_with_driver_sync_callback` /
`replay_fixture_with_driver_sync_callback` / per-step `E2ESut` hooks). (An earlier pass wrongly called this
"not windowed-blocked" by grepping only `crates/`, missing `frontends/`.) Deleting the runner core therefore
requires FIRST repointing those windowed replay harnesses onto `compose_windowed_sut` — this IS the
"windowed cap-impl deletion (AFTER PARTIAL E5)" / E4-step-7 below, a separate workstream. Partial-E5 (headless)
is otherwise complete.

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

## Round 5 (2026-07-01) — VERIFIED terrain map for the windowed repoint (task #7)

Re-mapped the windowed harness ⇄ `E2ESut`/`phased.rs` coupling against the *worktree* code (an
exploration agent read all six harness files + `phased.rs` + `harness.rs`/`builder.rs`; cross-checked).
This makes the prior "separate large workstream" bullet concrete and ordered. **No code landed this
round — this is the de-risking map that governs the increments below.**

**The one-sentence shape.** Today `compose_windowed_sut` is only ever a **per-step, check-only add-on
layered on a running `E2ESut`** (`invariant_runner.rs` hook + `sim_windowed_replay.rs` `per_tick`,
opt-in via `HOLON_PBT_WINDOWED_CATALOG`). `E2ESut` still owns the three things that make a windowed run
go: it **boots** the backend/session/engine, **applies** each transition, and **runs** the native
registry. `compose_windowed_sut` produces a **`CapMap`, not a `ComposedSut`** — it *wraps* handles
`E2ESut` produced and *reads* invariants. Repointing = moving boot + apply + check off `E2ESut`.

**Two NEW pieces of shared machinery the repoint needs (neither exists yet):**
1. **A standalone windowed backend/session booter** — replaces `E2ESut::new` + the StartApp transition.
   Today `PbtReadyContext` (`engine: BackendEngine`, `session: FrontendSession`,
   `reactive_engine: ReactiveEngine`) is sourced **from `sut.ctx`** (`phased.rs`), i.e. from the booted
   `E2ESut`. A standalone booter must produce those for a given `Wiring` so the window
   (`launch_holon_window_rebindable`) can bind to them **without** `E2ESut`. WARNING: `ComposedSut::init_test`
   boots caps **on tokio** (`harness.rs`) — thread-affinity forbids reusing it for the window; the
   booter is a distinct construction path.
2. **A windowed `ComposedSlice`/`ComposedSut` constructed from INJECTED window handles** — not
   `init_test`'s tokio boot. Its `apply_transition` routes the full alphabet (`PressKey`/`ArrowNavigate`/
   `ClickBlock`/`DragDropBlock` + the backend-write transitions) through the windowed CapMap's caps
   (`SutBlockInteract`/`SutArrowNavigate` via the live `GpuiUserDriver`/`SimUserDriver`; the
   backend-write caps once the CapMap composes `SutBackend` — see Hard-B below). Template to copy:
   `composed/harness.rs` (`ComposedSut<S: ComposedSlice>`, `apply` -> `S::apply_transition`,
   `FixtureAssertable` impl). `replay_steps` / capture / gherkin already produce `Vec<FixtureStep>` and
   are medium-agnostic — they only need `S = <windowed ComposedSut>` instead of `E2ESut`.

**Hard parts (from the map):**
- **A — nothing applies transitions through the windowed CapMap.** `run_windowed_composed_check` only
  `run_selected`s (checks). Need the windowed `ComposedSlice::apply_transition`.
- **B — nothing boots the backend the window binds to; the windowed CapMap has no `SutBackend`.**
  `window_input_wide` composes only the 3 window components (geometry + frontend-engine + driver-input).
  To apply the full alphabet AND check storage invariants, `compose_windowed_sut` must ALSO compose the
  backend/storage/editor read+write caps **over the window's LIVE `FrontendSession`** (the analogue of
  `compose_sut`'s frontend arm, but wrapping the live session rather than booting `HeadlessFrontendComponent`).
- **C — `replay_steps`/capture/gherkin are hard-wired to `S = E2ESut`.** `ComposedSut<S>` already impls
  `StateMachineTest + FixtureAssertable`, so the bridge is real — but it must be *constructed from the
  injected window handles*, not `init_test`.
- **D — coverage delta (the deletion gate). ✅ AUDITED 2026-07-01 — GO, no relocation needed.** The
  native windowed path's UNIQUE surface is just THREE invariants (not the 4+budget an earlier draft
  claimed): `inv-displayed-text/widget` (proxy), `inv-focus-matches-ref`, `inv-window-focus-matches-engine-focus`
  (self). `inv-sql-budget` is ALREADY relocated (`NATIVE_ONLY_EXCLUDED` + `E1_RELOCATED_CAP_COVERAGE` +
  `WIDE_REQUIRED_INVARIANTS`). All three unique ids ALREADY have composed catalog wires
  (`catalog.rs:43,47,55`) that select under `full_gpui` and run the **identical body structs** via
  `run_windowed_composed_check`. So native (a) `run_proxy_registry` + (b) `native_self_invariants` are
  pure per-tick DUPLICATION of what the windowed composed check already runs on the same live
  geometry/engine/driver. ⇒ deleting native (a)+(b) loses ZERO coverage **iff the windowed composed
  driver `composed/windowed.rs` stays live** (it must — it's the repoint target). ⚠ These three are
  INHERENTLY windowed (need `SutLayout`/`SutDriver`, deselect headless) — NEVER collapse to the headless
  keystone alone or they vanish.
- **E — the tui `pbt_main.rs` is the heaviest.** It uses the **random** path
  (`run_pbt_with_driver_sync_callback`, 50 steps), never the composed path, and feeds a
  `frontend_visual_state` backstop the composed `GpuiWindowComponent` reports as honest `None`. Needs a
  windowed composed **random** runner (generate + apply through caps) or a prior convert-to-replay.

**Increment order (thinnest -> heaviest; each its own commit; section 8.10 NO-branch justifies the new windowed
machinery — thread affinity means `WideE2E` genuinely cannot drive it):**
0. **Coverage-delta audit (Hard-D)** — ✅ DONE 2026-07-01: GO, no relocation needed (see Hard-D above).
   The 3 native-windowed-only invariants are already composed-wired and run by `run_windowed_composed_check`.
1. **Windowed booter (Hard-B boot half)** — standalone `(BackendEngine, FrontendSession, ReactiveEngine)`
   for a `Wiring`, additive; smoke-test a window binds to it with no `E2ESut`.
2. **Windowed full CapMap (Hard-B caps half)** — extend `compose_windowed_sut` to compose the live
   session's `SutBackend`/storage/editor/read caps, so `run_selected` runs the FULL catalog on the
   window path. De-risk additively by promoting `run_windowed_composed_check` to run the full catalog.
3. **Windowed `ComposedSlice`/`ComposedSut` from injected handles (Hard-A/C)** — `apply_transition` over
   the full CapMap; construct from step-1/2 handles; drive via `replay_steps`.
4. **Repoint harnesses (thinnest first):** `gpui_capture_replay` + `gpui_gherkin_replay` -> `windowed_replay`
   -> `sim_windowed_replay` (promote its per_tick composed check to primary; drop the env gate) ->
   `pbt_main` (tui, needs the random runner). Each drops its `E2ESut` use.
5. **Then unblocks:** delete the native runner core (`run_invariant_registry`, `phased.rs` windowed
   cluster, `impl StateMachineTest for E2ESut`, `Subsystem`/`min_sut`/`PbtSuiteSpec`) -> retire `parity.rs`
   LAST (deletion gate).

**Key files (worktree):** `phased.rs` (`PbtReadyContext`/`PbtReadyResult`, `replay_fixture_with_driver_sync_callback`,
`run_pbt_with_driver_sync_callback`), `fixtures/mod.rs` (`replay_steps`), `invariant_runner.rs`
(`has_window` selection + `run_windowed_composed_check` hook), `composed/windowed.rs` +
`window_slice/builders.rs` + `window_slice/components.rs`, `composed/harness.rs` (the `ComposedSut`/
`ComposedSlice` template), and the six harness files under `frontends/gpui/tests/` + `frontends/tui/tests/common/pbt_main.rs`.

WARNING **Tooling:** `ast-outline` reads a stale index of the MAIN checkout — its line numbers are wrong for
the worktree. Read worktree paths directly. Use worktree-absolute paths for ALL file ops. Build via
`bash -c '... > log 2>&1'` (nu `out+err>` redirects false-green).


### Round 5 UPDATE (boot-seam verified) — ★ REFRAME: reuse `compose_sut`'s boot, window = renderer

A boot-seam map (agent-verified, worktree) shows the repoint is **simpler than "two new machinery
pieces"** implied. Findings:
- The boot is already factored into the production DI booter `holon_app::new_from_config_with_di`
  (`crates/holon-app/src/session.rs`). BOTH `TestContext::start_app` (E2ESut) and
  `HeadlessFrontendComponent::new_with_loro` call it. `HeadlessFrontendComponent` is **already a
  standalone windowless booter** producing `(session, engine, reactive)`.
- `launch_holon_window_rebindable` (`frontends/gpui/src/lib.rs`) binds to **`session` + the frontend
  `ReactiveEngine` only** (`BackendEngine` is MCP-only). The window needs NO session-construction path —
  it is a **pure renderer** over a pre-booted reactive engine, on the gpui main thread. Boot is
  thread-agnostic (`Send` Arcs); topology already exists in `pbt_harness/mod.rs::run_in_gpui_window`
  (boot on runtime thread → ship Arcs over a channel → bind on main thread).

**⇒ REFRAMED architecture (supersedes "standalone booter + windowed ComposedSlice with new reconcile"):**
Reuse `compose_sut(full_headless, resolver)`'s full headless CapMap (backend/storage/editor caps **+
the `IdResolver` reconcile**), hand its booted `session`+`reactive` to a gpui window as a renderer over
the SAME reactive engine, and **override only the driver caps** (`DriverInputComponent`) with the
window's `GpuiUserDriver`. Everything else — `SutBackend`, storage invariants, id-resolution — comes
FREE from `compose_sut`. This **dissolves the Hard-D/increment-2 id-resolution blocker** (no new
reconcile; the window is a view onto the headless session, exactly as E2ESut renders its own session).
The §8.11 faithfulness rule is satisfied: gesture caps bind the window's `GpuiUserDriver` (highest rung),
not the headless `ReactiveEngineDriver`.

**Revised increment plan:**
1. ✅ **DONE (this commit):** `compose_sut`'s `ComposedSut` now surfaces the booted `session` + `reactive`
   (new `pub session`/`pub reactive` fields; new `HeadlessFrontendComponent::session()` accessor) so a
   windowed harness can bind a window to a `compose_sut`-booted session. Additive, build-green, no new warnings.
2. **Window-over-compose_sut bind + driver-cap insert.** ✅ CAP-OVERLAY CORE DONE (pure-insert design):
   the base is built by `compose_sut_windowed_base(set, resolver)` = a full `compose_sut` with the driver
   rung DEFERRED (`DriverPlacement::Deferred` — a new explicit choice threaded through
   `compose_sut_seeded_impl`; all existing `compose_sut`/`compose_sut_seeded` callers unchanged, default
   `HeadlessReactive`). Then `window_slice::builders::overlay_windowed_caps(caps, geometry, engine, driver)`
   INSERTS `GpuiWindowComponent` (`SutLayout`) + `DriverInputComponent::with_input` (the gesture caps) —
   both NEW, since the base deferred its driver, so NO cap is ever registered-then-overridden. Fail-loud:
   `overlay_windowed_caps` panics if the base already has a `SutDriver` (i.e. wasn't built deferred).
   Compile-verified. ✅ FOUNDATIONAL CLAIM VERIFIED GREEN (macOS) by the new test
   `frontends/gpui/tests/gpui_compose_sut_windowed.rs`: a TestPlatform window RENDERS a
   `compose_sut_windowed_base` session (68 elements, 63 non-degenerate), the deferred base hosts
   `SutBackend` (13 booted blocks), and the driver rung is correctly absent — i.e. the window-as-pure-
   renderer-over-compose_sut architecture works. REMAINING for full step 2: exercise `overlay_windowed_caps`
   itself with a live window driver (folds into the increment-3 StateMachineTest runner).
   REMAINING: on the gpui thread, `compose_sut(full_headless)` → take `.session`/`.reactive` →
   `launch_holon_window_rebindable(session, reactive, …)` (topology in `pbt_harness/mod.rs::run_in_gpui_window`)
   → `overlay_windowed_caps(composed.caps, geometry, composed.reactive, gpui_driver)` → `run_selected` and
   assert the full catalog (block/storage + windowed families) runs with the window attached. ⚠ target the
   TestPlatform sim path (headless-verifiable), not xcap (needs a display).
3. **Windowed StateMachineTest wrapper:** wrap the above as an `S: StateMachineTest + FixtureAssertable`
   whose `apply` reuses **WideE2E's `apply_transition`** (same headless dispatch through the shared
   session) BUT with gesture transitions routed through the window's driver caps; construct from injected
   window handles (not `init_test`). Drive via `replay_steps`.
   - ✅ **Sub-step 3b-i DONE (rev `4ad2b594`):** `ComposedSut::from_parts` + a `SettleHook` seam
     (`composed/harness.rs`) wrap already-booted caps and pump the window before each
     `check_invariants`; a windowed non-vacuity floor (keyed off an ACTUAL `SutLayout` cap) fails loud
     on a silent windowed deselect. `compose_sut_windowed_base_seeded` (builder) + `boot_and_seed_wide_windowed_base`
     + `windowed_composed_sut` (wide_e2e) assemble the SUT. Test `windowed_composed_sut_runs_full_catalog_green_on_the_initial_frame`
     (`gpui_compose_sut_windowed.rs`) is GREEN: the UNIFIED catalog (block/storage + windowed geometry +
     focus) runs over ONE `wide_e2e_ref()` oracle in ONE SUT. Threading: a dedicated multi-thread rt drives
     apply/check leaf futures while the session backend runs on its own runtime; the settle hook self-pumps
     the window via `app_ptr` (no `block_on`).
   - **Faithful focus (landed 3b-i):** initial page-root focus is established via the ENGINE
     (`dispatch_intent_sync(navigation.focus)` = same SQL write + `maybe_mirror_navigation_focus` into
     `engine.focused_block()`), NOT the raw `SutFocusWrite` (which bypasses the mirror — invisible
     headlessly since the headless `SutDriver` is withheld, but a divergence once a window `SutDriver`
     reads focus).
   - ✅ **Sub-step 3b-ii DONE (rev `09d29192`):** drive a hand-built `ClickBlock` gesture SEQUENCE through
     the real `StateMachineTest::apply` path over the window (each click focuses a text child via the
     window `SimUserDriver` → `set_focus` → engine mirror, opening its editor); the unified catalog stays
     GREEN each tick, INCLUDING the editor/displayed-text families that engage once an editor opens. The
     window boot/overlay/settle/teardown is now a shared `with_windowed_wide_sut(run)` test helper.
   - ✅ **Sub-step 3b-iii DONE (rev `444465f2`):** the capture/gherkin BRIDGE — drive a fixture through
     `replay_steps` over the windowed `ComposedSut<WideE2E>` (already `FixtureAssertable`); GREEN. Fixture is
     post-boot only (no `StartApp`; the harness pre-boots), matching composed-keystone captures.
   - ✅ **`SutFocusWrite` made FAITHFUL — supersedes the earlier "withhold it" plan (that was WRONG):**
     the windowed SUT SHOULD carry `SutFocusWrite` (`NavigateFocus`/`FocusEditableText` are real
     windowed capabilities). The bug was the IMPL — `HeadlessFrontendComponent::apply_navigate_focus` called
     `session.execute_operation` DIRECTLY (a headless shortcut) which writes the SQL nav tables but bypasses
     `dispatch_intent_sync` → `maybe_mirror_navigation_focus`, so `engine.focused_block()` stayed stale.
     Fixed in two steps: rev `c0095344` dispatched through `self.reactive.dispatch_intent_sync` (same SQL +
     mirror), then rev `77387a04` superseded that with the fully faithful form — the cap CLICKS the
     LeftSidebar entry through the production `ReactiveEngineDriver` (`click_entity(id, "left_sidebar")` →
     `find_click_intent` → `apply_intent` → `dispatch_intent(navigation.focus)`), exactly how E2ESut's
     `apply_navigate_focus` and the sibling `apply_focus_editable_text` drive it — the user-gesture path,
     not a synthesized dispatch (§8.11). Correct by construction — NO cap-withholding, NO `cap_set`
     subtraction. Headless keystone regression-free (the mirror is inert headlessly — `SutDriver` withheld).
     The 3b-i boot workaround (manual engine dispatch) reverted to driving `NavigateFocus` through the
     now-faithful cap.
   - ⚠ **REMAINING (3b-iv, folds into increment 4) — the proptest loop:** needs PER-CASE window setup (can't
     use `init_test` — thread affinity) + a windowed oracle carrying the ACTUAL windowed `cap_set`
     (`ComposedSut::cap_set()` — SutLayout/SutDriver present, honest, no subtraction) so
     `aggregate_transitions` narrows + the `required_invariants` floor matches the window. That IS the
     increment-4 work of repointing `sim_windowed_replay`/`random_pbt_sim` onto the windowed `ComposedSut`, so
     3b-iv and increment 4 merge.
4. Repoint harnesses (thinnest first; carries the 3b-iv live windowed oracle cap_set —
   `ComposedSut::cap_set()`, `SutFocusWrite` faithfully present, no withholding — + per-case window
   proptest) → 5. delete native core → retire `parity.rs` LAST. (Unchanged.)

⚠ Runtime must stay multi-thread and alive on the background thread while the window runs on main
(CDC/matview/org-sync tasks). `wait_for_ready = true` avoids the `without_wait()` sync-handle race.
