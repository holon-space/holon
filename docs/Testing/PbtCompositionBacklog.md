# PBT Composition — Work Backlog

Sliced tasks for extending the γ composition (`docs/Testing/PbtCompositionDesign.md`).
The point of this file is **distribution**: turn the remaining work into units that
are independent, self-verifying, and low-judgment enough to hand to fast/cheap
agents — while keeping the judgment-heavy work (component scoping, honesty calls,
framework changes) with a smart agent.

## ★ North Star — ONE configurable PBT (read this first)

Everything in this backlog serves a single end state, stated by the user (2026-06-21):

> **One PBT, configured by env vars for which subsystems are active (`Loro`, `Org`,
> `Turso`, `frontend`, `GPUI`). It automatically picks the feasible transitions + their
> generators and the feasible invariants for that configuration. Start it with just
> `Loro` → a very fast test that still generates every transition possible in that
> config and checks every feasible invariant (individual invariants still toggleable).
> Start it with all/most subsystems → on failure it minimizes BOTH the step sequence
> AND which subsystems must be active to reproduce. That single PBT covers everything.**

**There is no per-slice test in the end state.** `structural_pbt.rs`, `navigation_pbt.rs`,
`memory_slice/*`, `sql_slice/*` and every other slice are **scaffolding** — proving grounds
that de-risk one cap-host or one reconcile mechanic at a time. As the single configurable
PBT subsumes each, the slice is **deleted** (the "deletion-first / convergence" track).
Judge any new increment by: *does it move a capability into the one configurable PBT and let
a slice die?* — not *does it add a working slice?*

### What already exists vs. the gap

Most of the plumbing is built (this is the documented target of **ADR 0009** + the F2
convergence track) — it just currently runs over the monolith `E2ESut`:

| North-star requirement | Existing mechanism | Status |
|---|---|---|
| Subsystem activation via env | `HOLON_PBT_WIRING_AXES` over `ComponentSet`/`Subsystem` (`invariants/registry.rs`) | ✅ exists (axes: Loro/Org/Turso; **frontend/GPUI axes missing**) |
| Auto-pick feasible **transitions** + generators | `aggregate_transitions` value-level **wiring gate** (`transition_dispatch.rs`) — a transition self-skips when its subsystem is inactive | ✅ exists (over `E2ETransition`/`SutHandle`) |
| Auto-pick feasible **invariants** | `Needs::selected_against(sut_caps, ref_caps)` (`holon-pbt-core/composition.rs`) — capability presence selects | ✅ exists |
| Toggle an individual invariant | `HOLON_PBT_INVARIANTS=name:off\|warn` (`invariant_runner.rs`); weights via `HOLON_PBT_WEIGHTS` | ✅ exists |
| Minimize the **step sequence** | proptest shrinking | ✅ exists |
| Minimize the **active subsystem set** | `bisect_driver.rs` + `holon_pbt_core::bisect` — replays a captured failing `Vec<E2ETransition>` across the `ComponentSet` lattice, localizes the smallest reproducing set (ADR 0009 §3/§4) | ✅ exists |

**The gap = the entire F2 / E-track.** All of the above drives the **monolithic `E2ESut`**
(the bisector literally "builds an `E2ESut` wired for the node"). The composed-`CapMap` work
is replacing that one concrete SUT with a **`CapMap` assembled from components per active
subsystem set**, so the *same* alphabet + invariants + bisection run over composition instead
of a monolith. Concretely the remaining work is:
1. **Dissolve `E2ESut`** — relocate its cap impls onto components (E1/Phase-A), delete the
   impls (E3), delete `E2ESut` + the parallel `Subsystem`/`min_sut` selection (E5).
2. **Assemble the `CapMap` by subsystem** — a builder that, given the active
   `ComponentSet`/wiring (from `HOLON_PBT_WIRING_AXES`), composes exactly the components
   whose caps that config needs (`HeadlessFrontendComponent`, `LoroBackendComponent`,
   `SqlProjectionComponent`, the E4 `GpuiWindowComponent`, `OpDispatchWriter`, …).
3. **Add the frontend + GPUI subsystem axes** (E4 supplies the windowed/geometry component).
4. **Point the convergence PBT at the composed `CapMap`** and **delete the slices**.

The single configurable PBT is then just the convergence test driving a per-config composed
`CapMap` — the slices gone, `E2ESut` gone, one test parameterized by env.

> **⚠ Convergence rule (read before any E3 increment) — discharge a cap consumer by DELETION,
> not by a new slice.** When E3 needs to delete an `E2ESut` cap impl, the blocker is whatever
> test consumes that cap over `E2ESut`. If the swap config (`full_headless`) already provides
> the cap — which it does for everything `WideE2E` drives — the ONE PBT already covers the
> cap's invariants, so **delete the standalone test** (and, if the invariant must be guaranteed
> exercised, add its id to `WIDE_REQUIRED_INVARIANTS`; if it had a unique real-SUT teeth, move
> that into `composed/invariants/<name>.rs`). **Do NOT rewrite the test as a `ComposedSut`
> slice** — that just mints more scaffolding the North Star will have to delete later (the
> "temporary-PBT churn loop"). Build a new composed slice ONLY for caps `WideE2E` cannot yet
> drive (E4/GPUI/windowed input). **Litmus: every E-track increment must make total scaffolding
> go DOWN.** Full rule + worked example: `PbtCompositionDesign.md` §8.10. (This corrects the
> 2026-06-24 `xk` increment, which minted `TaskStateSlice`; the 2026-06-25 cleanup deleted it.)

## How to read this

Each task is tagged:
- 🧠 **smart** — requires a design/honesty judgment (what to wrap, is this testing
  something real, does this duplicate E2E). Do *not* delegate to a cheap agent.
- 🤖 **cheap** — mechanical, follows the recipe, verified by a single test command.
  Safe to run in parallel; near-zero shared-file contention.

A 🤖 task carries everything an agent needs:
- **Exemplar** — the existing file to copy.
- **Body + caps** — the `pbt/invariants/bodies/*` body and the cap traits its
  `where` bounds require.
- **Honesty** — *why this tests something real* (pre-decided, so the agent doesn't
  have to judge it).
- **Files** — what to create (almost always new files; ≤1 shared line).
- **Gate** — the exact command that must go green.

The full step-by-step recipes (add an invariant / component / cap, with code
snippets, the test-triad patterns, and anti-patterns) are in
[`PbtCompositionContributing.md`](PbtCompositionContributing.md). The terse version
lives in `crates/holon-integration-tests/src/pbt/composed/invariants.rs` (module doc).

## Status (2026-06-23) — current top-of-file summary

The decomposition track is past its keystone. What landed since the 2026-06-17 block below:

- **`SutHandle` is an empty marker bundle** (cluster-peel done 2026-06-20): all of its own
  `&mut self` methods relocated into `#[capmap_adapter]` caps, so **`CapMap: SutHandle`
  holds** and a composed `CapMap` is drivable through the production transition alphabet.
- **★ THE SWAP is done** (2026-06-21): `general_e2e_composed_pbt` drives the production
  `aggregate_transitions` **auto-narrowed** alphabet (26 of 28 cap-feasible transitions) over
  `compose_sut(full_headless())` — the composed `CapMap` IS the SUT, green.
- **★ MACRO REPOINT is done** (2026-06-22): `general_e2e_composed_pbt` is now a production
  **integration test** (`tests/general_e2e_composed_pbt.rs`) driving `ComposedSut<WideE2E>`,
  **additive beside** the `E2ESut` `general_e2e_pbt`. The `WideE2E` machinery now lives in the
  pbt-gated `composed::wide_e2e` module (single source); `composed::harness`+`subsystem_seed`
  are `cfg(any(test, feature = "pbt"))`.
- **B5 watches done** (2026-06-22): `inv-{active-watches,watch-rows}-match-ref` run over the
  production reactive watch surface with a faithfully-modeled oracle; zero narrowings.
- **Convergence item #4 (Loro-doc-unification) is RESOLVED for the swap config** (verified
  2026-06-23): the composed builder gate is now `!has_turso || frontend_sync_handle.is_some()`
  (`composed/builder.rs:302`). In `full_headless` (Turso+Loro+ViewModel+**EditorState**) the
  frontend boots Loro-on, so the Loro read caps + peer mesh + `SutBackend` all observe the one
  shared authority doc and `wait_for_quiescence` blocks until an imported peer delta projects
  into Turso `block_raw`. So **`AddPeer`/`PeerEdit`/`MergeFromPeer`/`SyncWithPeer` are now
  admitted, not narrowed** (`full_headless_cap_set_admits_peer_transitions` passes). `SutLoro`
  is withheld **only** in full-mode-WITHOUT-editor — an honest cap-presence narrow-out (a peer
  merge there would land in a Loro doc invisible to Turso), not a coverage gap.
- **The old "toggle tooth" red is GREEN** (verified 2026-06-23): the headless
  `SutMutate::toggle_state` drives the real `cycle_task_state` op on the unified authority doc,
  so the SQL↔Loro coherence is visible on both sides (`wide_frontend_toggle_state_lockstep_stays_green`
  + `..._sut_only_toggle_state_is_caught` both pass). The "set_field on an unfocused block →
  no Loro cell" failure mode was an `E2ESut` windowed-`state_toggle` artifact; the composed path
  bypasses it.

**The true remaining gate for E3/E5** (deleting `E2ESut`): the composed path still auto-narrows
out **E4 windowed input** (`PressKey`/`ArrowNavigate`/drag — the real GPUI `send_raw_keystroke`
driver path) and the seam-mutate/fixture transitions, so `E2ESut` is still the wider-coverage
reference. **E2 verdict-parity is now DONE (2026-06-23)** for the composed-covered set — selection
parity (static) + headless (194 ticks, both green) + windowed (88 ticks on a real display, both
green, composed check non-vacuous). The next gating work is **not** E2 but **porting the 22
native-only-unported invariants** (the `inv-viewmodel-*` family, `/loro`+`/matview` store-variants,
`inv-focus-matches-ref`) into the composed catalog so a *full* (not partial) E3 can proceed, plus
**E4** for the remaining windowed-input transitions.

**Base-health note (2026-06-23).** The canonical run (`cargo nextest run`, `.config/nextest.toml`
default-filter) executes only specific **integration binaries** (`general_e2e_pbt`, `extended_gen_pbt`,
…) — it does **not** run the `holon-integration-tests` **lib** unit tests, so the composed lib slices
have drifted unnoticed. Running `cargo test -p holon-integration-tests --lib --features pbt` today:
180/192 pass. The 12 fails split into:
- **3 deterministic test-infra reds — FIXED 2026-06-23** (these are coverage-oracle safety nets, not
  scaffolding, so worth fixing): `every_body_file_has_a_registry_entry` (orphan-check didn't know the
  `*_backend.rs` = `/block_raw` variant-file convention, nor consult the composed catalog),
  `every_file_is_registered_as_a_module` (`transitions/` orphan parser didn't recognize
  `pub(crate) mod`), and `now_query_compiles_to_canonical_sql` (`norm_ws` didn't absorb
  paren-adjacent whitespace). All three green.
- **8 scaffolding-slice reds — a B5 regression, NOT fixed here (scaffolding on death row).**
  `build_started_ref` (`composed/subsystem_seed.rs:333`) unconditionally calls
  `seed_booted_layout_into_ref`, so the oracle now contains `block:journals`/`__default__`. The
  frontend/wide SUTs boot that layout (green), but the **non-frontend scaffolding slices**
  (`memory_slice::structural_pbt`, `composed::subsystem_seed::*`, `sut_handle_decomp_spike`) build an
  in-mem SUT that does **not**, so `inv-blocks-match-ref/block_raw` reports `block:journals` missing.
  Per the ★ NORTH STAR these slices are **scaffolding to be deleted** once the ONE PBT subsumes them,
  and they're already excluded from the default nextest filter — so this decay is expected, not a
  product red. A real fix would thread a `boots_layout` flag through `build_started_ref`'s ~9 callers
  (the `has_actor(UI)` gate can't distinguish it — UI tracks `EditorState`, not frontend-boot).
- **1 separate red:** `keyword_set_survives_sut_serialize_parse` (`transitions/write_org_file.rs:430`)
  — org-keyword `task_state` round-trip diverges (`Some(NEXT)` vs `None`); independent investigation.

The old "toggle tooth" red is GREEN and is NOT one of the above. Everything below the next header is
the older, still-valid detail; dates inside each item are accurate as written.

## Status (2026-06-17)

- **Framework**: `CapMap`/`expect_ref`/`#[capmap_adapter]`/`BridgedInvariant`/
  `run_selected` — done, stable. 18 caps host on `CapMap` today (`SutLoroTaskState`
  added).
- **Convergence harness LANDED (2026-06-17)** — `tests/subsystem_convergence_pbt.rs`:
  the faithful, subsystem-*generated* PBT. It generates the active `Wiring` as the
  state machine's `init_state`, drives the **real booted `E2ESut`** through the full
  production transition alphabet (`aggregate_transitions`), runs the wiring-gated
  invariant registry, and on failure **shrinks the active subsystem set toward the
  minimal reproducing combination** — an architectural delta-debugger over a faithful
  app. `#[ignore]`d (each case/shrink reboots a real app); smoke via
  `HOLON_PBT_WIRING_AXES`/`PROPTEST_CASES`. Subsumes the in-process `subsystem_shrink`
  spike, now **deleted** (see F6).
- **Hand-rolled ref models RETIRED (2026-06-17, F2 Stage 2)** — `FixtureRef`,
  `EditorModel`, `FixtureEditorRef`, `EditorPureRef` are **gone**. All slices + catch
  triads now seed the real `ReferenceState` oracle via the `impl CapProvider for
  ReferenceState` + `reference_state_ref_caps` keystone (Design §8.8); shared
  seed/plant helpers live in `composed/subsystem_seed.rs`. Closes F6.1 deferred
  item #4.
- **F2 Stage 1 keystone LANDED (2026-06-18)** — the SUT *write* caps
  (`SutBlockTreeWrite`/`SutEditorMirrorWrite`/`SutFocusWrite`/`SutQuiesce`) are now
  `&self` + interior mutability, so `#[capmap_adapter]` hosts them on `CapMap` like
  read caps: **a composed slice's `CapMap` IS a `SutTransitionTarget`.**
  `memory_slice/structural_pbt.rs` drives a `CapMap` SUT through the production
  structural alphabet (Split/Join/Indent/Outdent) against the real `ReferenceState`,
  green (+3 prod bug fixes; MoveUp/Down gated out of *generation* pending a
  sequence-faithful SUT order). This unblocks the **Bundle E** E2ESut-dissolution
  endgame below. Detail: `~/.claude/plans/transient-percolating-narwhal.md`.
- **Shared catalog** (`pbt/composed/`): **13** invariants wired
  (`no_parent_cycles`, `source_language`, `blocks_match`, `no_orphan`,
  `block_content`, `block_content_sql`, `block_parent`, `editor_text`,
  `editor_caret`, `loro_no_errors`, `loro_children_match_ref`,
  `viewmodel_no_error_widgets`, **`task_state_storage_coherence`**).
- **Slices**: **five** — `memory_slice` (`MemoryBackend` + in-mem editor),
  `loro_slice` (real `LoroBackend` CRDT), `sql_slice` (real Turso `BackendEngine`),
  `frontend_slice` (real **windowless** `FrontendSession` + `ReactiveEngine`
  — the production render pipeline, no GPUI), and **`sql_loro_slice`** (the
  combined SQL+Loro SUT — the only non-redundant consumer of `SutLoroTaskState`).
  All run the *same* catalog; selection picks each slice's applicable subset.
  `memory_slice` 19, `loro_slice` 6, `sql_slice` 4, `frontend_slice` 2,
  **`sql_loro_slice` 2** (coherence positive + catch over real Turso+Loro;
  the fixture-driven triad lives in the shared catalog), composed catalog triads
  incl. SQL, Loro, ViewModel, and **coherence** invariants.

## The shape of what's left (important)

**With the memory + editor + Loro components, the SutBackend/editor/Loro caps are
covered.** The remaining bodies in `pbt/invariants/bodies/` bind caps no current
component provides (`SutSqlProjection`, `SutViewModel`, `SutRenderer`, `SutLayout`,
`SutDriver`, `SutWatchRows`, `SutLoroTaskState`). So coverage still grows by
*adding components* (Bundle B/C/D), not ref caps.

The backlog is therefore **bundles**: each new component is a 🧠 scope+build task
that unlocks a batch of 🤖 invariant-adds. A component also makes *every already-
wired* SUT-cap invariant run over the new realization for free (a selection test,
not an invariant-add — the payoff of the design).

---

## Ready now

### E0 ✅ DONE (2026-06-15) — Editor commit round-trip
`commit_editor()` (`take_commit()` → `MemoryBackend::update_block`) in
`memory_slice/integration_tests.rs`, plus `memory_slice_editor_commit_flows_to_backend`
(focused positive) and `memory_slice_editor_commit_roundtrip_matches_ref` (proptest:
random type/delete/move → commit each op → `blocks-match-ref/block_raw` confirms the
stored content equals the independent `EditorModel` text). Genuine differential test
of the editor's `String` byte-math vs the `Vec<char>` oracle, *through* real storage.
18 tests green.

### E0b ✅ DONE (2026-06-15) — Selection-regression guard
`memory_slice_selects_exactly_the_full_catalog` asserts the memory+editor slice
selects exactly its 8 applicable ids and discloses the 2 Loro invariants as
*deselected* (no `SutLoroLog`), not silently dropped. Caught real drift the moment
the Loro invariants joined the catalog — the guard works.

---

## Bundle A — Loro storage component ✅ DONE (2026-06-15)

The whole bundle landed: a `loro_slice` over a real `holon-loro` `LoroBackend`,
the §6 "a new component re-runs the catalog for free" claim made concrete.

### A1 ✅ DONE — `LoroBackendComponent`
`pbt/loro_slice/components.rs` wraps a real `LoroBackend` CRDT and provides
`SutBackend` (over `get_all_blocks`) **and** `SutLoroLog`. The 6 block-tree
invariants now run over Loro by selection, plus a Loro mutation-sequence proptest
cross-checks the CRDT against an independent model each tick. `loro_had_errors` is
honest-`false` (standalone CRDT has no `LoroSyncController`); `loro_children_of`
returns the tree's authoritative fractional-index order via `list_children`.

### A2 ✅ DONE — `loro_no_errors`
`composed/invariants/loro_no_errors.rs` (`Needs SutLoroLog`) + catalog line + triad
over a `FixtureLoroLog` double (catch injects `had_errors = true`). Inert-but-honest
in the pure-Loro slice; teeth in the fixture catch and the E2E counter.

### A3 ✅ DONE — `loro_children_match_ref`
`composed/invariants/loro_children_match_ref.rs` (`Needs SutLoroLog + RefBlockTree`)
+ catalog line + triad. Real teeth: the fixture reorders siblings the CRDT's
monotone fractional index can't, and the per-parent order check fires. Runs live
over the real tree in the Loro mutation proptest.

---

## Bundle B — SQL projection component (B1 + B2 ✅ DONE 2026-06-15)

**Honesty verdict (resolved):** the goal is to *retire the monolithic `E2ESut`*
(the §F2 convergence). So overlap with `E2ESut`'s SQL surface is the **point** —
this component is the storage-layer slice of E2ESut's replacement. It reuses
`E2ESut`'s own SQL realization (the same `block_raw` queries, the shared
`parse_block_row`) so the swap is clean. It is genuinely *lean*: a real Turso
`BackendEngine` driven through the production block CRUD operation, with **none**
of the reactive engine / frontend / navigation / CDC machinery.

### B1 ✅ DONE — `SqlProjectionComponent`
`pbt/sql_slice/components.rs` wraps a real `BackendEngine` and provides
`SutBackend` (over `block_raw`) **and** `SutSqlProjection`. The 6 block-tree
invariants now run over Turso by selection (the §6 payoff over a *third* storage
realization), plus a Turso mutation-sequence proptest cross-checks against an
independent model each tick. Engine built via `create_test_engine_with_setup` +
a block-CRUD `SqlOperationProvider` over `BLOCK_WRITE_TABLE` (no `EventInfraModule`
— structural block ops are out of scope). The navigation/focus/watch members of
`SutSqlProjection` are honest-empty (no reactive engine drives them, §5.1).

### B2 ✅ DONE — `block_content_matches_ref` (SQL variant)
`composed/invariants/block_content_sql.rs` (`Needs SutSqlProjection + RefBlockTree`,
id `inv-block-content-matches-ref`) + catalog line + triad over a new
`FixtureSqlProjection` double (catch = divergent `block_raw.content`). Real teeth:
runs live over the Turso store in the mutation proptest, *and* the SQL-column read
path the `block_raw` (typed-snapshot) variant doesn't exercise.

### B3 🤖 Wire `block_tags_references_exist` (deferred)
- **Body/caps**: `bodies/block_tags_references_exist.rs` — `SutSqlProjection`.
- **Honesty**: orphan tag-junction rows. ⚠️ The current slice creates only plain
  blocks (no tag ops), so it would be *vacuous in-slice* (teeth only in the
  fixture catch) until the slice drives tag creation. Wire alongside a tag op.

### B4 🤖 `focus_roots` ✅ DONE / `matview_consistent_with_ref` (still deferred)
- **`focus_roots` ✅ DONE** (via the SutHandle-track `NavigateFocus`→`SutFocusWrite`
  increment, 2026-06-19): `inv-focus-roots` is wired into the composed catalog
  (`composed/catalog.rs:57`, body `composed/invariants/focus_roots.rs`, `Needs RefFocus +
  SutSqlProjection`), `RefFocus` got its `#[capmap_adapter]`, and
  `HeadlessFrontendComponent` realizes `SutFocusWrite` + the `focus_roots` matview read, so
  it bites (no longer vacuous) over the navigation/frontend slices.
- **`matview_consistent_with_ref` still deferred**: the registered id is
  `inv-blocks-match-ref/matview` (a `blocks_match` store-variant), NOT yet wired into the
  composed catalog. It needs the `block` matview read (`block_row`) — watch for CDC/IVM
  hydration timing vs the synchronous `block_raw` write.

### B5 ✅ DONE (2026-06-22) — `active_watches_match_ref` / `watch_rows_match_ref` (task #5)
- **Body/caps**: `active_watches` = `RefWatches + SutWatchRows`; `watch_rows` = `RefWatches +
  RefBackend + SutWatchRows` (RefBackend added for seed-exclusion). Both wired into the composed
  catalog over the `frontend_slice`'s real headless `ReactiveEngine` watch surface.
- **The parity fix (task #5) — faithful oracle MODELING:** `generate_test_query` always emits
  `QuerySource::AllBlocks`, so a watch returns the whole block set. The booted composed SUT carries
  11 scaffold blocks (9 from `index.org` + the `journals` page shell + `__default__`) the hand-built
  oracle didn't model, and the oracle carried a phantom `started-ref-layout-query` seed the SUT lacks.
  Rather than seed-EXCLUDE them from the watch (which weakens the check), the **oracle now MODELS the
  real layout** the SUT boots, so `inv-watch-rows-match-ref` stays a full-fidelity full-block-set
  comparison. `StartApp::apply_to_ref`'s layout-seeding was extracted into
  `seed_booted_layout_into_ref` (`transitions/start_app.rs`, behavior-preserving — it parses the same
  `assets/default/index.org` + builds the `journals`/`__default__` page blocks + classifies
  `layout_blocks`); `build_started_ref` (`composed/subsystem_seed.rs`) now calls it instead of the
  phantom. Journals.org's body is NOT modeled — the SUT skips it (`seed_default_org_assets`
  early-returns on a non-empty vault). `SetupWatch`/`RemoveWatch` un-narrowed from `wide_e2e_ref`
  (now ZERO narrowings); `general_e2e_composed_pbt` drives them green @40 with random predicates over
  the modeled oracle. Teeth: `wide_frontend_setup_watch_lockstep_stays_green` +
  `wide_frontend_sut_only_watch_rows_is_caught`.

### B6 ✅ DONE (2026-06-15) — `task_state_storage_coherence` + combined slice
The **genuine** consumer of `SutLoroTaskState` (`SutLoroTaskState` vs
`SutSqlProjection`, a task_state cross-check between two SUT realizations).
- Hosted `SutLoroTaskState` on `CapMap` (`#[capmap_adapter]`); `LoroBackendComponent`
  now provides it (`properties["task_state"]` off the live tree — the same scalar
  the SQL side reads via `json_extract`).
- New **`pbt/sql_loro_slice/`** (`builders.rs` + `integration_tests.rs`, no new
  component — reuses `SqlProjectionComponent` + `LoroBackendComponent`). Composition
  choice: both provide `SutBackend` and `CapMap` is one-provider-per-cap, so the
  builder registers **SQL fully** (canonical block store) and **Loro for only
  `SutLoroTaskState`** — no silent `SutBackend` shadowing; Loro is purely the
  second task_state oracle.
- `composed/invariants/task_state_storage_coherence.rs` wires the body
  (`Needs SutSqlProjection + SutLoroTaskState`, no ref) + triad via new
  `FixtureLoroTaskState` double (and `FixtureSqlProjection.task_state` map).
  Catalog 12→13; E0b deselected list updated. Teeth verified.
- **Scope honesty:** the fixture triad tests the invariant body + selection (infra
  self-test, the teeth gate); the two `sql_loro_slice` real-store tests exercise
  *production* code (Turso `json_set`/`json_extract` round-trip vs Loro property
  meta), but only over **static** hand-picked states — no property-based
  exploration, and the escaping-stress inputs are not covered. That exploration is
  deferred to **F5** (a shared composed generator), not hand-rolled here.

> Skip `block_ids_match_ref` — redundant with `blocks_match`'s id-set equality
> (`compare_block_subset` already reports missing/spurious). Note, don't wire.

---

## Bundle C — Headless ViewModel/Renderer component (C1 ✅ DONE 2026-06-15)

### C1 ✅ DONE — `HeadlessFrontendComponent`
`pbt/frontend_slice/components.rs` wraps a **real windowless** `FrontendSession` +
`ReactiveEngine` over a Turso `BackendEngine`, built through the production DI path
(`holon_app::new_from_config_with_di`) over a seeded org file — no GPUI, no
geometry, no display link. Provides:
- `SutRenderer` over the real `ensure_watching` → `snapshot` → `interpret_pure` →
  `view_model_to_snapshot` path (faithful port of `E2ESut`'s render methods, the
  shared helpers made `pub(crate)`). All six methods implemented.
- `SutViewModel` — real `headless_error_node_count` (counts `Error` nodes in the
  rendered tree); the gpui-frontend-engine-specific methods (`frontend_root_vm`,
  `provider_stability_report`, …) are honest `None`/defaults (this slice has a
  headless engine, no separate gpui *frontend engine*).
- `SutBackend` over `block_raw` (the block-tree catalog runs over this realization
  too, §6 — a fourth storage backing the same catalog).
`Config::with_arc` was added (the session is built once, async, then shared).
The headless render path is **not** GPUI-window-flaky (that was window/geometry/
focus-specific); it polls `ensure_watching` until loaded with a 3s ceiling.

### C2 ✅ DONE — `viewmodel_no_error_widgets`
`composed/invariants/viewmodel_no_error_widgets.rs` (`Needs SutViewModel`, no ref)
+ catalog line (now 12) + triad via a new `FixtureViewModel` double. Runs over the
**real** rendered tree in `frontend_slice` (a valid layout has 0 error widgets);
teeth verified (fixture `Some(2)` → caught; `Some(0)` → catch fails).

### C3 🤖 Remaining renderer invariants — NO new oracle needed (note corrected 2026-06-23)

**🟢 Batch 1 LANDED (2026-06-23) — ViewModel cluster (4 of the ~20).** Ported
`inv-frontend-engine`, `inv-frontend-root-not-error`, `inv-live-tree-matches-fresh`
(`Needs SutViewModel`), and `inv-view-selection` (`Needs SutViewModel + RefRender`) into the
composed catalog. Changes: 4 bridge modules (`composed/invariants/{frontend_engine,
frontend_root_not_error,live_tree_matches_fresh,view_selection}.rs`) + catalog wires;
**`#[capmap_adapter]` added to the `RefRender` trait** (`holon-pbt-core/capabilities.rs:1440`
— the one-line hosting step; `ReferenceState` already implemented `RefRender`) + `RefRender`
inserted in `CapProvider::register`; memory-slice `selects_exactly` deselection list updated
(+4). Verified: lib selection tests 15/15; `frontend_wide_pbt` green (109s) with
`inv-frontend-engine` + `inv-frontend-root-not-error` added to `WIDE_REQUIRED_INVARIANTS`
(every-tick non-vacuity PROOF — they run over the real headless render pipeline);
`general_e2e_composed_pbt` green (78s). `live-tree`/`view-selection` are wired+passing but
left off the every-tick required list (readiness-dependent).

**🟢 Batch 2 LANDED (2026-06-23) — `value_fn_provider_*` + the `SutRenderer` cluster (10 more).**
- Batch 2a: `inv-value-fn-provider-identity` (`SutViewModel + RefTaskState + RefBlockTree`),
  `inv-value-fn-provider-arg-variance-13` (`SutViewModel + RefLayout + RefGlobalFocus`). Added
  `#[capmap_adapter]` to `RefTaskState` + `RefGlobalFocus` (one line each; `ReferenceState`
  already implemented them) + registered both. This **completes `SutViewModel`'s coverage**.
- Batch 2b: the 8 `SutRenderer` invariants — `viewmodel_snapshot`, `viewmodel_tree_virtual_slots`,
  `matview_consistent_with_ref`, `editable_text_has_draggable`, `viewmodel_root_matches_render_expr`,
  `viewmodel_decompiled_rows_match_query`, `viewmodel_entity_ids_subset_of_data`,
  `viewmodel_state_toggle_correct`. All ref caps (`RefLayout`/`RefRender`/`RefBlockTree`/
  `RefTaskState`) already hosted — pure wires.
- Verified: lib selection 15/15; `frontend_wide_pbt` green (219s); `general_e2e_composed_pbt`
  green (147s) — all 10 pass over the **real headless render pipeline**, no divergence.

**🟢 Batch 3 LANDED (2026-06-23) — `editable-text-triggers` + storage cluster (4 more).**
`inv-viewmodel-editable-text-triggers` (`SutRenderer`), `inv-live-children-match-ref`
(`SutSqlProjection + SutLoroLog + RefBlockTree`), and the `blocks_match` `/loro`
(`SutLoroLog`) + `/matview` (`SutBackend + SutSqlProjection`) store-variants (new `wire_loro`/
`wire_matview` in `composed/invariants/blocks_match.rs`). All caps already hosted. Verified:
lib selection 47/47 (the 3 `memory_slice::structural_pbt` reds are the **pre-existing B5
scaffolding decay**, not batch 3 — my new invariants all deselect there); `frontend_wide_pbt`
green (281s); `general_e2e_composed_pbt` green (185s).

**Unported now 4:** the windowed pair (`inv-focus-matches-ref`=`SutDriver`,
`inv-frontend-no-error-widgets`=`SutLayout+SutViewModel`) + the cap-host pair
(`inv-no-errors`=`SutErrorLog`, `inv-sql-budget`=`SutSpanMetrics`, need a component host).

**★ E3 DELETION NOW UNLOCKED for `SutRenderer` + `SutLoroLog`.** All their native invariant
consumers are composed-covered: `SutRenderer` (viewmodel-* family + matview-consistent +
editable-text-{has-draggable,triggers}); `SutLoroLog` (loro-no-errors, loro-children-match-ref,
blocks-match/loro, live-children-match-ref). Both are read caps (not transition-apply). Their
`E2ESut` impls can be deleted via the E3 mechanic — **pending a standalone-consumer check**
(the `SutLoroTaskState` lesson: a `tests/*_pbt.rs` slice may still dispatch their invariants
over `E2ESut`). `SutViewModel` stays (windowed `frontend-no-error-widgets` not yet covered);
`SutSqlProjection`/`SutBackend` stay (transition-apply + many consumers).

**The original heading ("need `RefRender` + a faithful ref") is STALE** — it predates the
F2 Stage-2 keystone (2026-06-17) that retired the hand-rolled fixtures and made the
production `ReferenceState` the single oracle. `ReferenceState` **already implements**
`RefRender` (`reference_capabilities.rs:770`) and `RefLayout` (`:650`) and `RefTaskState`
(`:866`). They are simply **not yet inserted** into the ref `CapMap` in
`CapProvider::register` (`:895`) — a deliberate "don't register a ref cap until its
invariant is wired, to avoid catalog scope creep" discipline (see the comment at `:888`),
**not** a missing oracle.

So wiring each renderer invariant is the **uniform porting pattern**, no fixture/ref design:
1. add `caps.insert(self.clone() as Arc<dyn RefRender>)` (and `RefTaskState` where needed) to
   `CapProvider::register` — one line, harmless (selection ANDs SUT∧ref cap sets);
2. add the composed-catalog `wire()` (`composed/invariants/<name>.rs` bridging the same body
   struct, `Needs` = its `Ref*`+`Sut*` bounds);
3. teeth (clean→Ok, planted→Fail over a `CapMap`).

The component's `SutRenderer`/`SutViewModel` impls are already in place
(`HeadlessFrontendComponent`, headless via `widget_tree_snapshot` + the warm-loop fixed-point),
so these run **headless**:
  `viewmodel_root_matches_render_expr` (`RefRender+SutRenderer`),
  `viewmodel_decompiled_rows_match_query` (`RefRender+SutRenderer`),
  `viewmodel_entity_ids_subset_of_data` (`RefLayout+RefRender+SutRenderer`),
  `viewmodel_state_toggle_correct` (`RefBlockTree+RefTaskState+SutRenderer`),
  `view_selection` (`RefRender+SutViewModel`),
  `editable_text_has_draggable` (`RefLayout+SutRenderer`),
  `matview_consistent_with_ref` (`RefLayout+SutRenderer`),
  `viewmodel_snapshot` / `live_tree_matches_fresh` / `frontend_engine` (SUT-only, no ref).
Genuinely windowed/geometry invariants (`frontend_bounds_*`, `inv-window-focus`,
`displayed_text/*`) are already composed-covered via the E4 `GpuiWindowComponent`. Only
`inv-no-errors` (`SutErrorLog`) + `inv-sql-budget` (`SutSpanMetrics`) need their **SUT** cap
hosted on a component first.

---

## Bundle D — Degraded "shows source" twin (signature negative-selection demo)

### D1 🧠 Query-engine component + the twin invariant pair
Add a `SutQueryResults` component; author the complementary pair: "decompiled rows
match query" (needs the cap **present**) and "shows source" (needs it **absent**,
`sut_absent: [dyn SutQueryResults]`). Depends on Bundle C to observe the rendered
output. This is the §5.2 / §6 degraded config — high design value, judgment-heavy.

---

## Bundle E — `E2ESut` dissolution endgame (F2 final)

**Decision (2026-06-18, user-confirmed framing):** the windowed frontend is **not
architecturally special** — it is just one more component backing a cap, exactly
like the Turso component backs `SutSqlProjection`. The end state is §6 in full:
`E2ESut` *dissolves* into components, and "E2E" becomes the full component list
(including a windowed one). There is **no permanent residue**; `E2ESut` survives only
as a *migration waypoint* — the temporary backing for `gpui_ui_pbt` — until the
windowed component exists, then it is deleted.

**Why the window felt special (and why it isn't) — refined 2026-06-18 after a
read-only audit.** What's already true (so it is *not* a blocker):
- The geometry/window caps (`frontend_root_vm`, `widget_tree_snapshot`,
  `visual_content_fraction`, `rendered_elements`, and even the fresh-frame
  `rendered_elements_fresh`) are **already** object-safe `async fn (&self)` cap-trait
  methods in `capabilities.rs`, **already** have `caching_proxy.rs` forwards, and
  `HeadlessFrontendComponent` **already** implements them as honest-`None` (§5.1). So
  **`CapMap` can host the windowed caps today** — no `&mut self` flip, no trait
  surgery. (The `native_self_invariants` `&mut`-comment in `invariant_runner.rs` is
  stale w.r.t. these — the windowed bodies are all `&self`; caret/text already run
  headless anyway.)

What genuinely remains for the windowed realization — **exactly one** real unknown,
the rest mechanical:
1. **A realization that returns non-`None` geometry.** The view-model tree is
   *logical*; there are no bounds until a layout/paint pass runs, which needs a
   surface (real window **or** `TestPlatform`). Same shape as "`SutSqlProjection`'s
   only realization needs Turso." **The open question is whether `TestPlatform` can
   produce real, *deterministic* geometry without a real gpui window — this is the
   one make-or-break (see E0c below).** It is why `frontend_slice` stops at
   `SutViewModel`/`SutRenderer` and leaves `SutLayout` on `E2ESut`.
2. **One shared frame-pump settle.** A windowed component settles by pumping paint
   (what makes `rendered_elements_fresh` fresh) instead of polling a watch
   (`frontend_slice` already polls, 3 s ceiling). This is the deferred
   `RegistryHost`/per-realization-settle seam (Design §8 Step 1) — *one* shared seam,
   not per-invariant work.

Operational flakiness (occlusion / blur bimodality / real-window key focus — the
TestPlatform-migration history) is a property of *that one component's realization*,
handled by §8.7's cost-asymmetry rule (down-weight in generation, opt-in, prefer a
`TestPlatform` window over a real one), **not** a reason to keep a god-type. Whether
that flakiness is *eliminable* on `TestPlatform` is precisely what E0c settles.

**Cap relocation map** (`E2ESut` impls → home). Headless caps already placed: ✅
`SutBackend`, `SutBlockTreeWrite`, `SutEditorMirrorRead`, `SutLoroLog`,
`SutLoroTaskState`, `SutSqlProjection`, `SutRenderer`, `SutViewModel`. Orphans needing
a home (E1):

| Cap | Proposed home | Note |
|---|---|---|
| `SutEditorMirrorWrite` | `InMemEditorComponent` | Stage-1b: collapse `InProcEditorSut` into the component |
| `SutLoro` (live-tree) | `LoroBackendComponent` | sits beside `SutLoroLog`/`SutLoroTaskState` |
| `SutCdc` | reactive-engine component (`HeadlessFrontendComponent` or `SqlProjectionComponent`) | apply-only `drain_cdc(&mut self)` stays as-is (§7) |
| `SutWatchRows` | `HeadlessFrontendComponent` (reactive watch surface) | unblocks B5 invariants |
| `SutQueryCompile` | tiny `QueryCompileComponent` (pure) or `HeadlessFrontendComponent` | stateless |
| `SutOrgRead` / `SutOrgFileWrite` / `SutOrgRender` | new `OrgFileComponent` over the FileSystem port | the ADR-0011 in-memory FS path |
| `SutLifecycle` | the booted-session component(s) (`HeadlessFrontendComponent` owns the session) | — |
| `SutLayout` + window-focus + displayed-text | **`GpuiWindowComponent` (E4)** | the windowed component — last + most expensive |

### SutHandle decomposition track (started 2026-06-19) — drive a composed CapMap through the FULL transition alphabet

**The keystone this names precisely (the exploration-loss constraint).** The bulk
SUT caps (`SutBackend`, `SutSqlProjection`, `SutViewModel`, `SutRenderer`,
`SutLoroLog`, `SutErrorLog`) cannot be deleted from `E2ESut` (E3) without gutting
coverage, because removing an invariant body from `native_proxy_invariants` drops it
from the full proptest exploration (`general_e2e_pbt` / `subsystem_convergence_pbt` /
`gpui_ui_pbt` `StateMachineTest`s), leaving it only in the *static* composed slices.
That trade was fine for the org/watch caps; it is **not** fine for the core caps whose
block-tree invariants over full exploration *are* the suite's primary value. The real
unblock: make a composed `CapMap` drivable through the **full transition alphabet** in a
`StateMachineTest`, so the composed path runs the same exploration the native path does
— then native dispatch becomes redundant and `E2ESut` deletes with no coverage loss.
Today ~41 of ~50 transitions are welded to the `E2ESut`-only `SutHandle` monolith
(`transition_dispatch.rs`); only 4 structural (`memory_slice/structural_pbt.rs`) + 3
editor + now 1 navigation transition dispatch on fine-grained caps.

**The REAL gating risk for the endgame (NOT proven by single-transition slices).** Can a
single concrete SUT type satisfy the **union** of all ~50 transitions' trait bounds at
once, and can a `StateMachineTest` drive a **heterogeneous mixed alphabet** through one
`CapMap`? A single-`{NavigateFocus}` (or single-`{4 structural}`) alphabet proves *none*
of that — it only proves the per-transition rebind mechanic. Do not read a green
single-cluster slice as "decomposition proven."

**Step-0.5 3-cap-union probe result (2026-06-19): PASS.** A compile-only assertion
(`navigation_pbt.rs`, `assert_three_cap_union::<HeadlessFrontendComponent>`) confirms one
concrete type simultaneously satisfies `SutFocusWrite + SutSqlProjection + SutBackend`
with no wrapper. The miniature of the union-of-~50-bounds question is tractable; scale it
up as more clusters are decomposed.

**Increment 1 — `NavigateFocus` onto `SutFocusWrite` ✅ DONE (2026-06-19).** First
non-structural/non-editor transition decomposed. Mechanics: (1) `NavigateFocus`'s
`apply_to_sut` rebound `S: SutHandle` → `S: SutFocusWrite`; `apply_navigate_focus`
removed from the `SutHandle` trait (the macro dispatch carries `+ SutFocusWrite` directly
rather than folding it in as a supertrait — that would clash with `SutHandle`'s remaining
`apply_focus_editable_text`). (2) `SutHandle::apply_navigate_focus` flipped `&mut self` →
`&self` (the navigation state is all behind interior-mut / `Arc` seams, V1), so the new
`impl SutFocusWrite for E2ESut` delegates to it; the inline retry-loop drain swapped
`drain_region_cdc_events` (`&mut`) → the `&self` `drain_delivery_barrier` (the region_data
mirror drain is redundant — the shared `check_invariants` prep re-drains region CDC). (3)
The two focus invariant bodies (`InvNavigationFocus`, `InvFocusRoots`) ported into the
composed catalog (`composed/invariants/navigation_focus.rs` + `focus_roots.rs`); `RefFocus`
got its `#[capmap_adapter]` (the plan's claim it already had one was wrong) + a
`caps.insert(... as Arc<dyn RefFocus>)` in `ReferenceState::register`. (4)
`HeadlessFrontendComponent` realizes `SutFocusWrite` (production `navigation.focus` op +
focus-matview settle) + `SutSqlProjection` (focus rows); `live_focus_root_rows` reads the
`focus_roots` matview (same source as `focus_roots_rows`, so mirror==matview → focus_roots
teeth produce a real `Fail`, never a CDC-lag `Skipped`, V4). (5) New
`frontend_slice/navigation_pbt.rs` `StateMachineTest`: `{NavigateFocus}` alphabet over a
real headless Turso session, checked against a **`RefFocus`-only** ref CapMap (so only the
two focus invariants select — no block-tree alignment needed; the focus invariants read
`navigation_history`/`open_pins`, not `block_state`). Ref seeded to `block:journals`
initial focus to match the SUT boot. Teeth: lockstep green + non-vacuity (`ran_ids`),
SUT-only navigate trips both focus invariants with `Fail`. `SutSqlProjection` is added in
the *navigation builder* (`frontend_navigation_wide`), NOT in `register`, so other
frontend-slice tests don't newly select `block_content_sql`.

**Selection-safety:** memory_slice's `selects_exactly_the_full_catalog` updated (the two
focus invariants disclosed-deselect there — no `SutSqlProjection`); sql_slice selects them
but they pass vacuously (honest-empty focus rows, unnavigated ref). Parity oracles
(`native_runner_dispatches_exactly_the_registry`, `composed_catalog_covers_e1_relocated_caps`)
unaffected — the focus bodies stay wired in the native runner; the catalog only gained
copies. lib green except the 2 pre-existing reds (`every_body_file_has_a_registry_entry`,
`now_query_compiles_to_canonical_sql`); native compiles green. NOTE: the native
`general_e2e_pbt_sql_only` hits a **pre-existing nondeterministic** settle-race flake in
`apply_type_chars` (Loro `content_raw` not landed before TypeChars; "increase
pre_inv16_settle" — fails on different random block ids each run, unrelated to navigation).

**Increment 2 — `NavigateHome` onto `SutNavHistoryWrite` ✅ DONE (2026-06-19).** New
`SutNavHistoryWrite { apply_navigate_home(CapRegion) }` cap; `NavigateHome` rebound off
`SutHandle`; `E2ESut` flips `apply_navigate_home` `&mut`→`&self` and delegates. The
`navigation_pbt` slice now drives a **2-transition** alphabet (`{NavigateFocus,
NavigateHome}`); `go_home` exercises a focus-*clear* settle path (make-or-break H4 passed).
`back`/`forward` deferred to E4 (headless prod doesn't mirror `go_back`/`go_forward`).

**Increment 3 — `SetupWatch` onto `SutWatchRegister` ✅ DONE (2026-06-19).** First cluster
that required **flipping the watch state to interior-mut** (the keystone risk row A named
this UNPROVEN). `TestEnvironment`'s `active_watches`/`watch_queries`/`ui_model` `HashMap`s
→ `RefCell` (no borrow crosses an `.await`; `E2ESut` is never `Send`-bound — `?Send` impls
driven via `block_on` — so `RefCell` is sound, not `Mutex`); `setup_watch` flipped
`&mut`→`&self`; the drain loops (`drain_cdc_events`, `assert_cdc_quiescent`) keep `&mut
self` and just `.borrow_mut()` (the watch guards drop before the `&mut self`
`all_blocks_stream` access). New `SutWatchRegister { register_watch(query_id, source, lang)
}` cap takes the **compiled** query (pbt-core can't name the int-test `TestQuery`; the
`SetupWatch` transition compiles via `compile_for` at the boundary). `E2ESut` realizes it
by forwarding to `setup_watch` (via `Deref`); `HeadlessFrontendComponent` shares the
`register_watch_compiled` core with `register_query_watch`. The dead `SutHandle::
apply_setup_watch` was **removed** (no delegator, unlike `apply_navigate_home`). Teeth:
`frontend_slice_setup_watch_via_cap_makes_invariants_bite` drives `SetupWatch` through the
composed `CapMap` and `inv-watch-rows-match-ref` bites (Ok clean, Fail on dropped child).
**Validation:** `general_e2e_pbt_full` PASSES (exercises the E2ESut `setup_watch`/drain
RefCell path at runtime — no borrow panic); lib 134 pass / same 4 pre-existing reds; native
`--features pbt --tests` compiles. (`cdc_delivery_pbt` is pre-existing red — its config
lacks `preset org_writes`, so `StartApp` never bootstraps: "no transition applicable" at
init, before booting; unrelated to this work. `general_e2e_pbt_sql_only` "SetupWatch: 32
rejections" is the documented pre-existing sql_only generation flake.)

**Remaining clusters:** `navigate_forward`/`back` (leader-chord surface, E4), then the
heavier `SutHandle` methods (`start_app`/lifecycle, `switch_view`, `toggle_state`,
`click_at_element`) — each its own vertical slice. The endgame keystone — one
collision-free CapMap builder hosting the union of all transition caps, driving a
mixed-alphabet `StateMachineTest` matching the convergence harness's exploration, then
native retirement (E3/E5) — comes only after enough of the alphabet is decomposed *and*
the union-of-bounds probe (scaled up) stays tractable. **Doc follow-up ✅ DONE
(2026-06-19):** the stale `#[ignore]` notes in `subsystem_convergence_pbt.rs:16-41` were
rewritten to match the macro (`slice.rs:347/364-386`; it runs by default).

#### Cluster-peel ✅ DONE + E3 Phase A cap-porting (2026-06-20)

**Cluster-peel ✅ DONE (2026-06-20).** All 30 of `SutHandle`'s own methods relocated into
`#[capmap_adapter]` caps; `SutHandle` is now an **empty marker bundle** (15 supertraits +
blanket `impl<T> SutHandle for T`, no `impl SutHandle for E2ESut`) in
`transition_dispatch.rs:168`. A composed `CapMap` holding the 15 caps now satisfies
`SutHandle` exactly as `E2ESut` does. lib + gpui + tui + pbt-core compile green;
`general_e2e_pbt` PASSES (`general_e2e_pbt_sql_only` = documented pre-existing Loro-settle red).

**The structural fact that orders the endgame.** `general_e2e_pbt` is
`impl StateMachineTest for E2ESut { type SystemUnderTest = Self }` (sut.rs:1206). Transitions
are generic `impl<S: Cap>`, but the harness picks ONE concrete SUT type — so E2ESut cap impls
**cannot** be deleted piecemeal while E2ESut IS the SUT. The real unit of progress is
**swapping the SUT to a composed `CapMap`**; cap-impl deletion (E3) is the dead-code sweep that
follows. Order: (A) close the provider gap → (C) build a composed-SUT `StateMachineTest`
driving the full alphabet (= the endgame keystone) → swap → (E3) delete impls leaves-first →
(E5) delete `E2ESut`.

**E3 Phase A — provider gap: 5 → 9 of 15 bundle caps (2026-06-20).** Ported onto
`HeadlessFrontendComponent` via production session/engine paths (NOT re-impls): **A1**
`SutViewControl` (honest `current_view` field replacing the `"all"` stub, read by
`SutViewModel::current_view`), `SutMcpEmit` (faithful no-op — windowless stack has no
`PbtMcpIntegration`, same as `E2ESut` with an empty `pbt_mcp` slot), `SutHistoryWrite`
(`engine.undo/redo`); **A2a** `SutNavHistoryDrive` (pin=`focus_pin`, unpin=`close`,
back/forward — all via `session.execute_operation("navigation", …)`). **A2a refutes the old
"back/forward not mirrored headless" claim** (lines 19–20, 392): the GPUI `synthetic_dispatch`
wrapper was window-bound, but the underlying `navigation` *provider* ops are session-executable.
New probe `headless_nav_history_ops_dispatch`: `focus_pin` populates `focus_roots(main)`,
`go_back`/`go_forward` dispatch headlessly without error (history *semantics* parity → Phase B).
`frontend_slice` 17/17 green. Union-of-bounds probe (`navigation_pbt.rs`) scaled **4 → 14**
caps on one concrete type — green.

**The entanglement boundary (where mechanical porting stops).** The clean "add a session-op
impl to `HeadlessFrontendComponent`" pattern is **exhausted at A1/A2a**. The remaining 6 bundle
caps are each entangled and are NOT standalone component impls:
- `SutBlockInteract`, `SutArrowNavigate` — need a `UserDriver` + geometry the windowless
  component lacks (the A2b driver/geometry fork; deferred pending a decision).
- `SutMutate` — `toggle_state` clicks the state_toggle *widget* (geometry) + reads `pre_ref_state`;
  `apply_mutation`/`bulk_external_add` are `&self` no-ops whose work lives in the **seam**.
- `SutFixtureFs` — manipulates `E2ESut`'s `doc_uri_map`/`documents` bookkeeping + real git/jj
  subprocesses; pre-boot fixture ordering vs. the component's eager `new()` boot.
- `SutAppLifecycle` — the **bootstrap** (start_app/restart); needs the `exp4` lazy-`&self`-boot model.
- `SutLoro` — CRDT peer writes through the `&mut apply_peer_*` seam.

These converge on **Phase C**: porting them IS building the composed-SUT harness — the
`block_tree_post_action` **seam** (sut_check_invariants.rs:33, the ref_state-dependent
settle/synthetic-id-reconcile work) and the **lifecycle** (lazy-`&self` boot, exp4 LifecycleProbe)
*for the composed `CapMap` SUT*, not more standalone impls.

**Phase C plan (= the endgame keystone, in progress).** Build a `ComposedSut { caps: CapMap,
resolver: IdResolver, rt }` generalizing the spike's `SqlStructuralSut`
(sut_handle_decomp_spike.rs:481) + `exp4` lifecycle, driving a **heterogeneous mixed alphabet**
through ONE CapMap against the `composed_invariant_catalog` oracle — directly attacking the
named gating risk (lines 336–341). Increment order: (C1) widen a composed `StateMachineTest`
to a mixed alphabet over the **already-ported** caps (structural + focus + nav-home + the A1/A2a
transitions whose effects the focus/nav oracle observes — `NavigateBack`/`Forward`/`Pin`/`Unpin`;
this also lands A2a's deferred history-semantics parity); (C2) generalize the per-tick
`reconcile_split_ids` mini-seam into the full `block_tree_post_action` seam port; (C3) add the
lazy-boot lifecycle so `SutAppLifecycle` drives as a composed transition; (C4) layer in the
seam-dependent caps (`SutMutate`/`SutFixtureFs`/`SutLoro`) cluster by cluster; the A2b
driver/geometry caps fold in once that fork is decided. Each increment is its own vertical slice
with teeth, like the NavigateFocus/Home/SetupWatch increments above.

**Phase-C plan REFINED by 4 parallel research agents (2026-06-20).** Findings materially
simplify and reorder it:

- **The seam is NOT a C1 blocker.** Navigation transitions have **no `block_tree_post_action`
  arm** — they fall through `_ => {}` (sut_check_invariants.rs:339). The seam exists only for
  block-mutating + lifecycle transitions. So a composed nav (+structural) SUT needs **only the
  spike's existing `IdResolver` split-reconcile** — nothing new. The hard caret/focus-handoff
  seam pieces (`sync_caret_to_new_split_block`, PressKey focus verify) already self-disable
  without a driver/geometry, so a headless SUT legitimately skips them. ⇒ the full seam port
  (old C2) is deferred to the PressKey/editor + lifecycle arms, NOT needed for C1.
- **Lifecycle: pre-boot, don't drive `StartApp` (minimal C3 = Option A).** Both existing
  composed StateMachineTests (`NavSut` navigation_pbt.rs:197-243, `MemStructuralSut`
  structural_pbt.rs:178-194) already pre-boot in `init_test` and seed the ref already-started
  (`build_started_ref`), excluding `StartApp` from the alphabet. C1 extends `NavSut` the same
  way — **zero new SUT scaffold, zero `HeadlessFrontendComponent` boot refactor**. Lazy-boot
  (exp4, drives `StartApp`) is proven but over-scoped; defer to a dedicated lifecycle increment.
- **C1 transitions, per-transition verdict (all faithfully mutate the ref AND are observed —
  non-vacuous):** `NavigateBack`/`Forward` → inv-navigation-focus (cursor→current_focus);
  `PinBlock`/`UnpinBlock` → inv-focus-roots (open_pins). BUT:
    - **PinBlock — safe to add now**: SUT mints its own `history_id` (nothing passed in). Verify
      the slice seed has a pinnable Text descendant of Main + the component surfaces RightSidebar
      in `focus_roots`.
    - **NavigateBack/Forward — probe FIRST**: the code itself flags headless back/forward matview
      parity as unproven (components.rs:1077). Make-or-break: drive `go_back` headlessly after two
      focus navs, read `current_focus_rows()` — add only if it moves to match the ref cursor.
    - **UnpinBlock — defer**: needs the ref `next_history_id` ↔ SUT AUTOINCREMENT lockstep proven
      for this slice's boot seed (risk C; no reconcile map exists for nav history_ids, unlike the
      block `doc_uri_map`). Add after a probe confirms a freshly-pinned block's ref id == the SUT
      real `navigation_history` row id.
    - **Revised C1 order:** PinBlock (with teeth) → probe back/forward matview parity + probe
      pin/unpin id alignment → add Back/Forward + Unpin gated on those probes.
- **C4 parallelism (confirmed):** the only shared central file is the composed seam, and **only
  `SutMutate` touches it**. So:
    - **`SutFixtureFs`** — fully independent: extend `HeadlessFrontendComponent` (already owns
      `org_fs`/`org_root`/`_temp`) for `write_org_file`+`create_directory`; no seam, no ref_state.
      Blocker: `git_init`/`jj_git_init` shell out to a real path but writes go to the in-mem FS
      (mismatch) — those + `create_stale_loro` need a real on-disk fixture.
    - **`SutLoro`** — independent (needs only a shared primary `doc_store` handle): mechanical F2
      write-cap recipe (`&mut self`→`&self`, add `#[capmap_adapter]`, `peers` behind `RefCell`).
      Currently `&mut self` + non-adapter ⇒ cannot host on a `CapMap` until flipped.
    - **`SutMutate`** — NOT parallelizable: `toggle_state` clicks the state_toggle widget
      (needs `SutDriver`+`SutLayout` = the A2b fork), `apply_mutation` needs driver dispatch +
      the seam + transitively `SutLoro`. Do last, serial, after A2b is decided.
  ⇒ `SutFixtureFs` and `SutLoro` are the genuine C4 fan-out (two agents, disjoint files);
  `SutMutate` is the entangled tail.

#### C1 status + open item: boot nav-history alignment (2026-06-20)

C1 (extend `navigation_pbt`'s composed `StateMachineTest`) has landed, all green
(`frontend_slice` 23/23):

- **`PinBlock`** — in the **generated** mixed alphabet now (`{NavigateFocus, NavigateHome,
  PinBlock}` drive one composed `CapMap` against the focus oracle) + teeth
  (`pin_block_lockstep_stays_green`, `sut_only_pin_block_is_caught_by_focus_roots`) + asserting
  make-or-break probe (`headless_pin_block_right_sidebar_probe`). The slice seed (`doc_org_files`)
  gained a stable-id Text child block via a `:PROPERTIES: :ID: ref-block-0 :END:` drawer so the
  pinnable block can be named by constant. `PinBlock` is emitted directly (not via its
  `weighted_generator`), bypassing its `block_state` precondition — sound because the
  `RefFocus`-only slice doesn't model `block_state`, `apply_to_ref` only pushes `open_pins`, and
  `inv-focus-roots` is a pure id-set compare.
- **`NavigateBack`/`NavigateForward`** — teeth only (`navigate_back_forward_roundtrip_lockstep_stays_green`,
  `sut_only_navigate_back_is_caught_by_focus`) + parity probe
  (`headless_back_forward_focus_parity_probe`, which **refuted** the standing doubt: headless
  `go_back`/`go_forward` *do* move `current_focus(main)` to track the ref cursor).

**✅ FIXED — boot nav-history-depth alignment (2026-06-20).** The first divergence: adding
`NavigateBack` made a boundary `go_back` give ref `focus_roots(main)={}` vs SUT `{block:journals}`.
Root cause: `navigation_ref()` = `build_started_ref` + `navigate(journals)` carried a deeper
history `[None, c1, journals]` (cursor 2) — `build_started_ref` seeds a `c1` focus (for editor
preconditions unused by this slice) and `NavigationHistory::new()` prepends a `None` "home" entry —
so `go_back` walked the ref into phantom `c1`/`None` entries the headless SUT (booted journals-only)
never had. **Fix landed:** `navigation_ref()` now trims `navigation_history[Main]` to
`entries=[Some(journals)], cursor=0` after seeding, matching the SUT boot. Robust at
`PROPTEST_CASES=40` ×3 for the `{NavigateFocus, NavigateHome, PinBlock}` generated alphabet.

**✅ FIXED — `go_home` idempotency (the deeper gate) + `UnpinBlock` history-id alignment
(2026-06-20).** With the seed trimmed and `NavigateBack`/`Forward` re-added, a higher-case run
(`PROPTEST_CASES=40`) surfaced a SECOND divergence, minimal sequence `NavigateHome`×N →
`NavigateBack`: the ref ended at `current_focus(main)=home` (None) while the headless SUT's `go_back`
*restored* `journals`. **Root cause: `navigate_home.rs::apply_to_ref` had NO idempotency guard** — it
pushed a `None` `navigation_history` entry on EVERY call, so `NavigateHome`×N → N phantom home entries
that `NavigateBack` walked back through. Production `navigation.focus(None)` is **idempotent** when
already home (proven by `headless_go_home_idempotency_probe`: `go_home`×3 → a single NULL row), like
`navigate_focus`'s same-target focus. (An investigation agent initially mis-verdicted this as "already
fixed / no ref change needed" — it traced a *single* `go_home`; the empirical probe caught the
repeated-`go_home` case.) **Fix landed:** added an `already_home` guard to `navigate_home.rs::apply_to_ref`
(skip the history/open-pins push + `next_history_id` bump when current focus is `None`), mirroring
`navigate_focus`. This is a shared production-PBT transition — verified **no `general_e2e_pbt`
regression** (still PASS; only the documented pre-existing `sql_only` Loro-settle red remains). With
it, `NavigateBack`/`NavigateForward` are now in the generated alphabet, **robust at `PROPTEST_CASES=40`
×3**.

`UnpinBlock` history-id alignment (risk C) also resolved: `headless_unpin_block_probe` showed the SUT
assigns the pin `navigation_history.id` via AUTOINCREMENT (journals=1, pin=2), but the ref's
`next_history_id` was off-by-one (the same `c1` seed bumped it). `navigation_ref()` now also resets
`next_history_id`=2 + the journals open-pin id=1; the `unpin_block_lockstep_stays_green` teeth asserts
the ref predicts id 2, matching the SUT, so `close(history_id)` targets the right row.

**✅ C1 COMPLETE — generated alphabet is `{NavigateFocus, NavigateHome, PinBlock, NavigateBack,
NavigateForward, UnpinBlock}`, robust at `PROPTEST_CASES=40` ×3.** `UnpinBlock` is generated
state-dependently (`NavMachine::transitions` draws its `history_id` from `open_pins[RightSidebar]`
via `unpin_candidates`, never the Main focus pin; shrink-gated by `preconditions` so a shrink that
drops the creating `PinBlock` invalidates it). The full nav alphabet now drives one composed
`CapMap` against the focus oracle, with teeth + make-or-break probes for every transition
(`headless_pin_block_right_sidebar_probe`, `headless_nav_history_ops_dispatch`,
`headless_back_forward_focus_parity_probe`, `headless_unpin_block_probe`,
`headless_go_home_idempotency_probe`). `frontend_slice` 27/27 green.

This C1 work also produced one shared-transition **fix** (`navigate_home` idempotency) verified
against `general_e2e_pbt`, and surfaced one **prod smell to file**: `go_back` parks the cursor on a
soft-closed `navigation_history` row without re-opening it (`provider.rs:392-406`), so cursor-based
`current_focus` reads the last block while the open-rows `focus_roots` reads home — two SUT
projections disagree; the ref reproduces the same split so the PBT cannot catch it.

#### C2/C3/C4 research re-run + decisions (2026-06-20, post-C1)

A second 3-agent read-only sweep (C2 seam / C3 lifecycle / C4 fan-out) refined the above
and produced two **decisions** (user):

- **Next implementation step = C2.0 — the generic reconcile+settle loop.** Confirmed: the
  "seam" is NOT inside any cap impl — it lives entirely in the E2ESut harness `apply()`
  (`sut_check_invariants.rs:33-341`, `block_tree_post_action`). C2.0 = lift the spike's generic
  `IdResolver` kernel (`sut_handle_decomp_spike.rs:505-544`: before/after SUT-id diff → shared
  `IdResolver`, then `with_resolved_doc_uris` at check time) into the live composed
  StateMachineTest + add the id-minting **structural alphabet** (Split/Join/Indent/Outdent) so the
  reconcile actually fires. `SutBlockTreeWrite` is **already registered** on
  `HeadlessFrontendComponent` (`components.rs:1012`). No prerequisite refactor; highest leverage;
  unblocks `SutMutate` and the lifecycle seam arms downstream.
- **C3 lifecycle model = Option B — deferred-boot refactor** (for when `start_app` is tackled,
  AFTER C2.0). `new()` will only stage org files; a `boot()` cap runs `new_from_config_with_di`;
  `engine`/`session`/`reactive`/`injector` become `OnceCell`. Mirrors E2ESut faithfully and enables
  a **true** `simulate_restart` (the current E2ESut `simulate_restart` is only a file re-touch, NOT
  a process restart — `test_environment.rs:2419`). Larger blast radius (touches all ~13 cap impls
  that read those fields) but chosen over the minimal re-warm. C3's other three caps
  (`concurrent_schema_init` clean; `create_document` + file-poke `simulate_restart` with a
  `documents`→`Mutex` interior-mut) port without the refactor.
- **C4 fan-out unchanged**: `SutFixtureFs` onto `HeadlessFrontendComponent` (clean: `create_directory`
  /`git_init`/`jj_git_init`; `write_org_file` re-key adaptation; **defer `create_stale_loro`**) ∥ Loro
  read caps. NOTE the Loro read half (`SutLoroLog`/`SutLoroTaskState`) is **already hosted** by the
  existing `LoroBackendComponent` (cap-host table) — confirm coverage before any new Loro port; the
  `&mut self` `SutLoro` peer-write surface stays on E2ESut (needs the flip recipe to host on a CapMap).
  `SutMutate` = serial tail, after C2.0 relocates the seam.

#### ✅ C2.0 COMPLETE — generic reconcile+settle loop on the SUT-swap target (2026-06-21)

`frontend_slice/structural_pbt.rs` — `FrontendStructuralSut`, the FIRST reconcile-based
structural StateMachineTest driving a composed **`CapMap`** (the SUT is the CapMap, NOT a
component) into which **`HeadlessFrontendComponent`** contributes the `SutBackend` cap and
`OpDispatchWriter` the `SutBlockTreeWrite` cap. `HeadlessFrontendComponent` is **one
constituent component** of the composition that replaces `E2ESut` — not a monolithic
replacement (see ★ North Star: the replacement is a per-subsystem-assembled CapMap, and this
slice is scaffolding that will be folded into the single configurable PBT and then deleted).
Alphabet `{SplitBlock, JoinBlock}` (+ `Nothing` no-op fallback) over leaf siblings, checked
by `composed_invariant_catalog()` against the live `ReferenceState`. Green at
`cases=24 × len 1..10` (~42s); `frontend_slice` **31/31**.

**The reconcile kernel** (lifted from the spike `sut_handle_decomp_spike.rs:505-544`): per
tick, diff the SUT `block_raw` id-set before/after, pair the one minted real `uuid` against
the oracle's one synthetic `block::split-N`, accumulate into a shared `IdResolver`
(`OpDispatchWriter::with_resolver`); at check time `with_resolved_doc_uris` remaps the
oracle into SUT id space. Unlike `memory_slice` (which hints ids via `set_next_split_id`),
Turso mints real uuids, so reconcile is mandatory — this is the kernel C2 was about.

**Scaffold seed-injection** (the headless-specific alignment, the make-or-break, proven by
`components::tests::headless_structural_seed_and_reconcile_probe`): the full production boot
leaves ~13 scaffold blocks (`__default__`, the layout/sidebar tree + PRQL query children,
`journals`, the booted org doc) that the spike's bare engine never has. `compare_block_subset`
is id-set-EXACT (the "subset" is over *facets*, not blocks), so each booted id is injected
into the oracle as `block_documents[id]=no_parent` → it joins `seed_block_ids()` and filters
out of the SUT snapshot, reducing the comparison to the working `{parent,c1,c2}(+split)` on
both sides. (Headless analog of E1 `SutOrgRead` seeding the oracle from booted blocks.) The
working tree is seeded via the production create op as **leaf siblings under a seed page**
(`structural-page`) so candidates are never direct children of `no_parent`. Teeth: a SUT-only
split is CAUGHT by `inv-blocks-match-ref/block_raw`; a lockstep split stays green.

**Two real product smells FILED** (deferred follow-ons, like the `go_back` smell; both block
`Outdent`/`Indent` from this slice's alphabet, documented in `StructTransition`):
1. **`no_parent` write-back inconsistency.** Turso `split_block`/`outdent` writes a literal
   `NULL` `parent_id` when a block lands at the top level, whereas the bootstrap writes the
   `sentinel:no_parent` string — `Block::try_from` rejects the `NULL`. `MemoryBackend`
   tolerates it (so `memory_slice` never hit it). Surfaced by splitting/outdenting a
   `no_parent` block. → `Outdent` excluded; page-root keeps Split off the top level.
2. **Split-of-block-with-children divergence.** Splitting a block that HAS children: Turso
   makes the new block a **child** of the split block; the oracle (and `MemoryBackend`) make
   it a **sibling**. → `Indent` excluded (it's the only transition that turns a leaf into a
   parent), keeping every candidate a leaf so the divergence is never exercised.

Both are genuine Turso-vs-oracle/MemoryBackend behavior gaps worth a `general_e2e_pbt` repro
check + fix; neither blocks the reconcile-loop keystone. Only non-test change: a `pub(crate)
HeadlessFrontendComponent::engine()` accessor (additive — no production code touched, so no
`general_e2e_pbt` regression risk).

#### ★ SCOPED: the CapMap-by-subsystem builder (2026-06-21)

The keystone that lets the convergence PBT drive a composed `CapMap` instead of `E2ESut`,
assembled per active subsystem — so the slices retire wholesale. Grounded in a 3-agent map of
the real API. **This is the deliverable that turns the slice-by-slice work into the one PBT.**

##### What already exists (don't rebuild)
- **Config space → subsystems**: `ComponentSet` (`holon-pbt-core/component_set.rs`, interrogated
  via `has_storage/has_actor/has_projection`, no member iterator) → `subsystems(&set) -> BTreeSet<Subsystem>`
  (`invariants/registry.rs:73`; 9 variants, `BlockTree`+`Driver` intrinsic-always).
- **Assembly**: `Config::new().with(component).build() -> CapMap`; a component is any
  `impl CapProvider { fn register(self: Arc<Self>, &mut CapMap) }` (`composition.rs:192-219`).
- **Selection**: `run_selected(catalog, &sut, &ref)` → `Needs::selected_against(sut.cap_set(), ref.cap_set())`
  (`composition.rs:237,350`). Disclosed deselection, never faked. The wide PBT's `run_proxy_registry`
  is already the same shape — comment at `invariant_runner.rs:337` says "a composed CapMap plugs
  into the very same seam".
- **Ref side is UNIFORM** (key simplifier): ONE `impl CapProvider for ReferenceState`
  (`reference_capabilities.rs:895`) registers ALL `Ref*` caps; selection ANDs SUT∩Ref so the **SUT**
  cap set alone decides. The builder needs no per-subsystem ref components — only the seed varies.
- **Alphabet + wiring gate**: `aggregate_transitions` + per-variant `required_wiring().satisfied_by(&state.wiring)`
  (`transition_dispatch.rs:337-385`) — a transition is structurally absent when its subsystem is off.
  C2.0 already proved this reuses verbatim with a cap-scoped bound (`structural_pbt.rs:169`).
- **Prototype to generalize**: `build_sut(rt, has_loro, has_editor)` (`composed/subsystem_seed.rs:138`)
  is the existing flag-driven by-config builder.

##### THE central hazard → the one framework primitive needed
`CapMap` keeps **one provider per cap `TypeId`**; a 2nd component providing the same cap
**silently shadows** the 1st (the documented reason `sql_loro_wide` is hand-rolled). `SutBackend`
is provided by Memory, Loro, Sql, AND HeadlessFrontend — so naïve `.with(a).with(b)` is wrong.
→ Add **`CapMap::merge_missing(&mut self, other: CapMap)`** to holon-pbt-core: insert only TypeIds
not already present (first-registered wins). Then the builder registers components in **precedence
order**, each into its own CapMap, `merge_missing` into the accumulator — precedence is just order,
no per-cap hand-listing. (This replaces the brittle `sql_loro_wide` "register one fully, hand-insert
the other's non-overlap" pattern everywhere.)

##### Builder signature + the subsystem→component table
```rust
// async (boots real components); returns the SUT CapMap + the aux the harness needs.
async fn compose_sut(set: &ComponentSet, resolver: &IdResolver) -> ComposedSut;
struct ComposedSut {
    caps: CapMap,
    scaffold_ids: BTreeSet<EntityUri>, // booted-frontend filter set (empty if no real boot)
    multi_thread: bool,                // booted FrontendSession needs it; memory/sql don't
    settle: Duration,                  // 150ms booted, ~0 for synchronous memory/loro
}
```
Registration order = precedence (canonical `SutBackend` first), via `merge_missing`:

| active (from `subsystems(&set)`) | component | caps it contributes | precedence note |
|---|---|---|---|
| storage `Turso` (+ ViewModel) | `HeadlessFrontendComponent` | `SutBackend`+VM/Renderer/Org/Focus/Nav/Watch + `SutBlockTreeWrite`(resolver) | **canonical backend** when frontend active |
| storage `Turso` (no frontend) | `SqlProjectionComponent` | `SutBackend`,`SutSqlProjection`,`SutBlockTreeWrite`(resolver) | canonical backend |
| storage `Loro` | `LoroBackendComponent` | `SutBackend`*,`SutLoroLog`,`SutLoroTaskState` | *backend only if no Turso; else merge_missing keeps just the Loro-read caps |
| (neither) | `MemoryBackendComponent` | `SutBackend`,`SutBlockTreeWrite` | fallback backend |
| `EditorState` | `InMemEditorComponent` | `SutEditorMirrorRead/Write` | **shares the backend store Arc** (commit lands where invariants read) |
| `TursoProjection` (no frontend) | `SqlProjectionComponent` | `SutSqlProjection` | nav/focus projection |
| `FrontendBounds` | `GpuiWindowComponent` (E4) | `SutLayout` | windowed; needs real window |
| `Driver` (windowed) | `GpuiDriverComponent` (E4) | `SutDriver` | windowed focus |

The `SutBlockTreeWrite` writer is always `OpDispatchWriter::with_resolver(engine, resolver)` so the
generic reconcile works for id-minting (Turso) backends; memory's hint-honoring path collapses to an
empty resolver (identity). `seed` and `oracle` come from `build_started_ref(subsystems)` + the
store-seed appropriate to the canonical backend (`seed_store`/`seed_sql`/real boot + scaffold capture).

##### The harness it feeds (folds the 3 near-duplicate StateMachineTests into one)
`MemStructuralSut`/`NavSut`/`FrontendStructuralSut` have a **byte-identical skeleton** (caps+rt;
`apply` = `match t { _ => t.apply_to_sut(ref_state, &mut caps).await }`; the before/after-diff →
shared `IdResolver` reconcile kernel; `check` = `run_with_seeded_ref(catalog, &caps, oracle)` +
`failures().is_empty()` + `REQUIRED ⊆ ran_ids`). Only 6 axes differ: component list (← the builder),
alphabet, seed, id-strategy (collapses to "resolver, empty for hint backends"), runtime flavor,
settle. → one generic `ComposedSut<A: Alphabet>` owns the skeleton; the 3 slice tests become thin
config entry points, then disappear when the convergence swap lands.

##### The reconcile-generalization boundary (what bounds the first swap)
The C2.0 loop reconciles **block-minting** only (Split/Join, 1 mint/tick). The full `E2ESut` seam
(`block_tree_post_action`, `sut_check_invariants.rs:33-341`) ALSO does **doc-uri** minting
(StartApp/CreateDocument), **focus/caret** handoff (PressKey), **mutation/peer** (ApplyMutation/
BulkExternalAdd), and **Turso-CDC settle barriers** (vs the flat `sleep(SETTLE)`, fine for synchronous
Loro/Memory). So the composed builder can drive only the **subset of the alphabet whose caps are
ported AND whose reconcile is covered** — exactly the Phase-A/C entanglement boundary. The swap is
therefore **incremental by config**: configs whose active alphabet is fully covered (structural + nav
over Loro/Turso headless) swap first; doc-uri/focus/peer reconcile generalize as those caps land.

##### Increments (each retires something)
1. **`CapMap::merge_missing`** primitive (holon-pbt-core) + unit test. ✅ DONE (2026-06-21):
   `composition.rs` — merges a second component's caps keeping first-registered (precedence)
   and RETURNS the shadowed cap names for disclosure (fail-loud, not silent). Tests
   `merge_missing_merges_disjoint_caps` + `merge_missing_keeps_first_registered_and_reports_shadow`;
   holon-pbt-core 34/34.
2. **`compose_sut(set, resolver) -> ComposedSut`** (`composed/builder.rs`) + the subsystem→component
   table above. **2a ✅ DONE (2026-06-21): the storage arms** — Turso (`SqlProjectionComponent`,
   canonical backend, resolver-sharing writer) + Loro (`LoroBackendComponent`), with the **Turso+Loro
   precedence merge** via `merge_missing` that replaces `sql_loro_wide`'s hand-rolled non-overlap insert.
   Deferred arms guarded **fail-loud** (no silent under-provisioning). 4 tests green: turso-only /
   loro-only (no `SutBlockTreeWrite` → structural self-gates) / turso+loro (shadow report contains
   `SutBackend`) / rejects-no-storage. **Honest framing recorded in the module doc**: `compose_sut`
   provides the FULL subsystem cap-set, not the slices' hand-tuned minimal subsets (e.g. `sql_loro_wide`
   drops `SutLoroLog`), so it's verified by **cap-set membership + shadow disclosure, NOT byte-parity**
   — the seed-unification that makes every selected invariant pass is increment 3. **2b ✅ DONE (2026-06-21): the frontend arm** —
   `HeadlessFrontendComponent` (async org boot over a fixed minimal page) is the canonical Turso
   backend AND supplies the ViewModel/Renderer/Org/watch/nav caps; `SutSqlProjection` added explicitly
   (not in the component's `register`), the writer overridden to the resolver-sharing one, and the
   booted **scaffold ids captured** into `ComposedSut.scaffold_ids` (+ `multi_thread=true`,
   `settle=150ms`, `engine` exposed for seeding) — the aux a harness needs to fold C2.0's
   `boot_and_seed`. Test `compose_sut_frontend_arm_caps_and_aux` (multi-thread) green; 5 builder tests
   total. NOTE the arm provides the FULL frontend cap-set (14+), not `boot_and_seed`'s minimal
   `{SutBackend, SutBlockTreeWrite}` — so the actual slice rewrite waits on the seed-unification in
   increment 3. **2c ✅ DONE (2026-06-21): the editor arm** — when the config has `Projection::EditorState`,
   `compose_sut` registers an `InMemEditorComponent` (`SutEditorMirrorRead/Write`) committing into the
   CANONICAL backend so a committed keystroke lands where the block invariants read. KEY: `BackendEngine`
   has **no** `CoreOperations`, so the editor's commit was narrowed to a new `EditorCommitTarget` trait
   (just `commit_block_content` — minimal interface, no faked methods): blanket via `CoreOpsCommit(Arc<dyn
   CoreOperations>)` for Loro/memory, and `impl EditorCommitTarget for BackendEngine` (the production
   `block`/`set_field` op, same as `SqlProjectionComponent::update_content`) for Turso. `InMemEditorComponent::new`
   is unchanged (wraps in `CoreOpsCommit` internally — zero existing-call-site churn); new `new_commit` takes an
   explicit target. The `!EditorState` fail-loud assert is removed. **`compose_sut(full_headless())` now
   COMPOSES** (was a panic) — proven by `compose_sut_full_headless_composes` (canonical Turso frontend backend
   + Loro read caps + editor mirror caps, `SutBackend` shadow disclosed). Tests `compose_sut_editor_arm_turso_caps`
   (set_field path) + `compose_sut_editor_arm_loro_caps` (CoreOps path); memory_slice/subsystem_seed editor
   consumers green (refactor behavior-preserving). **NOT YET: driving editor transitions green** needs
   committed-content parity (Design §8.8 deferred half — the ref must commit too) + the lifecycle/mutate/fixture
   arms + reconcile generalization. **2d TODO**: lifecycle/mutate/fixture arms, then E4 GPUI. → as arms land,
   the static `*_wide` builders collapse to `compose_sut` calls.
3. **`ComposedSut<A>` generic harness** — reframed by the fresh-drive finding (below). The harness
   must adopt the wide PBT's **fresh-start drive** model, NOT fold the slices' hand-seed model.
   - **✅ Fresh-drive PROBE DONE (2026-06-21)** — `structural_pbt.rs::teeth::frontend_fresh_drive_full_capset_probe`:
     built the SUT via `compose_sut(frontend)` (FULL 14+ cap-set), seeded the working tree, and **drove
     `NavigateFocus(page)` on the SUT** (instead of hand-seeding the oracle against an un-navigated SUT).
     The full catalog selected **15 invariants and 14 PASS** — incl. `inv-navigation-focus` +
     `inv-focus-roots` (the ones slices omit `SutSqlProjection` to dodge), viewmodel, displayed-text,
     org-render, watches, block-tree. **The fresh-drive model is VALIDATED: focus invariants pass when
     the SUT is DRIVEN.** The ONLY failure is `inv-blocks-match-ref/org` — exactly the predicted gap: the
     tree was seeded via STORE create ops, so it's absent from the org files `SutOrgRead` parses. **Fix =
     seed via the ORG path** (write the tree to org files; the `FileSyncController` ingests it into the
     store — the production block-origin path), so store AND org agree. That's the clean seed-unification.
   - **✅ ORG-SEED gap CLOSED → FULL CATALOG GREEN (2026-06-21)** —
     `structural_pbt.rs::teeth::frontend_fresh_drive_org_seed_full_catalog_green`: boot the frontend with
     the working tree **AS the org** (`structural-page.org`: `#+ID: structural-page` + `* parent/* c1/* c2`
     with pinned `:ID:` drawers), so the session ingests it into the store AND `SutOrgRead` parses it —
     store and org share ONE source. Scaffold = booted − {parent,c1,c2} (the page stays seed). Drive
     `NavigateFocus(page)` on the SUT. Result: **all 15 selected invariants PASS, zero failures** over
     `compose_sut(frontend)` — block-tree, content, org, org-render, viewmodel, displayed-text, watches,
     navigation, focus. **The seed-unification is SOLVED for the frontend config.** (Two format gotchas
     fixed along the way: the org filename IS the page title the viewmodel renders — name it
     `structural-page.org` to match the oracle's page content; the store-only seed left the tree out of
     org, which is what `inv-blocks-match-ref/org` caught.) frontend_slice + compose_sut: 37/37.
   - **✅ HARNESS EXTRACTED (2026-06-21)** — `composed/harness.rs`: `ComposedSut<S>` (the generic
     `StateMachineTest`) + the `ComposedSlice` trait. The harness owns the byte-identical skeleton — the
     runtime, the per-tick `IdResolver` reconcile, the scaffold-injection, and the `run_with_seeded_ref`
     + non-vacuity check. A slice provides only the 6 axes (Transition, Machine, REQUIRED_INVARIANTS,
     SETTLE, MULTI_THREAD, `build`/`apply_transition`). `FrontendStructuralSut` + its `impl StateMachineTest`
     + the local `sut_ids`/`inject_scaffold_seed` (~115 lines) **DELETED**; `frontend_structural_pbt` now
     runs over `ComposedSut<FrontendStructural>` (a ~25-line `ComposedSlice` impl). frontend_slice +
     composed: 77/77. The reconcile/check kernel is shared by the eventual wide-over-`compose_sut` path,
     so it's not slice-only scaffolding.
   - **✅ NAV + MEMORY FOLDED (2026-06-21)** — both structural slices now run over `ComposedSut<S>`. The
     harness gained two overridable axes (each defaulted so the frontend slice is unchanged):
     `ComposedSlice::align_ids` (id alignment, default no-op = the generic reconcile; **memory** overrides it
     to `set_next_split_id(next_id)` counter-sync — the reconcile then sees an identity pair) and
     `ComposedSlice::run_report` (check scope, default = full catalog over the scaffold-seeded
     reconcile-resolved oracle; **nav** overrides it to a `RefFocus`-only `run_selected` over the raw oracle).
     A new `type Handle` carries a slice-owned component (memory/nav keep their `Arc<…Component>`; frontend
     uses `()`), and `build` now also receives the initial `ref_state` (so a counter-sync slice seeds its
     `next_id`). `MemStructuralSut`/`NavSut` + their `impl StateMachineTest` **DELETED** (~170 lines), replaced
     by ~50-line `ComposedSlice` impls; the harness absorbed ~45 lines of generalization that serves all three.
     `memory_slice` + `frontend_slice` + `composed`: **89/89** (incl. all teeth; `frontend_navigation_pbt`,
     `memory_slice_structural_pbt`, `frontend_structural_pbt` all green over the shared harness).
   - **Remaining**: (a) ✅ org-seed recipe proven; (b) ✅ harness extracted; (c) ✅ nav + memory folded. Then
     the **wide-over-`compose_sut`** swap (needs per-cap-bound alphabet dispatch for partial configs) retires
     the slices entirely — scoped below.

   ##### ★ SCOPED: per-cap-bound alphabet dispatch (2026-06-21)
   **Goal**: the wide PBT drives a composed `CapMap` and AUTO-derives its alphabet from the CapMap's
   actual `cap_set()` — a partial config (Loro-only, sql-only) generates only the transitions whose caps
   are present. Two sub-problems, mapped to the real code:

   **✅ FORK RESOLVED + DONE (2026-06-21): Option A** — flipped `SutLoro` to `&self` + interior mutability +
   `#[capmap_adapter]`; `CapMap: SutHandle` now holds (see PCG-4). All 14/14 caps host on `CapMap`.

   **A. Type-level dispatch — 13/14 DONE (probed 2026-06-21).** The wide enum dispatch is
   `impl<S: SutHandle> TransitionImpl<ReferenceState, S> for E2ETransition`
   (`transition_dispatch.rs:280`); each variant's `apply_to_sut` is bound on ONE fine-grained cap
   (`SplitBlock: SutBlockTreeWrite`, `NavigateFocus: SutFocusWrite`, …), and the bundle `SutHandle`
   (`transition_dispatch.rs:168`, 14 caps, blanket-impl'd) is their union. A compile-probe shows
   **`CapMap` already satisfies 13 of the 14** caps via `#[capmap_adapter]` — the SOLE holdout is
   **`SutLoro`** (`capabilities.rs:634`), whose methods are `&mut self` async, incompatible with the
   `Arc<dyn Cap>`/`expect()` (`&self`) adapter. So `E2ETransition::apply_to_sut` accepts `S = CapMap`
   the instant `CapMap: SutLoro` holds. **THE FORK** (needs a user call):
     - **Option A (recommended)** — flip `SutLoro` to `&self` + interior mutability on the Loro provider
       (peer-mesh behind `RefCell`/`Mutex`, same recipe as cluster-peel INC3's watch HashMaps;
       soundness = no borrow across `.await`), then `#[capmap_adapter]`. Makes `CapMap: SutHandle`
       fully → the ENTIRE 51-variant alphabet (incl. peer/Loro) drives a CapMap → unlocks the
       **Loro-only fast config** the North Star wants. Larger, but on the critical path.
     - **Option B** — defer Loro: a concrete `impl TransitionImpl<ReferenceState, CapMap>` with the
       Loro-requiring variants hard-stubbed (panic; gated out of generation so never reached), rest
       dispatch normally. Swaps the non-Loro wide PBT now; Loro-only stays impossible; throwaway stub.

   **B. Runtime cap-gate — the actual "per-cap-bound" mechanism (fork-independent).** Mirror the
   EXISTING wiring gate (`required_wiring().satisfied_by(&state.wiring)`, generation in
   `transition_dispatch.rs:352` + slice generator `slice.rs:159`, replay in `stepper.rs:151`) with a
   cap gate:
     1. Add `required_caps() -> &'static [CapId]` to `TransitionFactory` (default `&[]` = always
        feasible), the cap-analog of `required_wiring()`. `CapId::of::<dyn Cap>()` already exists
        (`composition.rs:74`); `CapSet` is a `HashSet<TypeId>` with `contains(&CapId)`
        (`composition.rs:214`).
     2. Override per transition with the cap in its `TransitionImpl` bound (`SplitBlock →
        [SutBlockTreeWrite]`, peer ops `→ [SutLoro]`, …) — ~51 one-liners, most default-empty or single.
        Guard with a test: each transition's `required_caps ⊆ {the cap its impl is bound on}`.
     3. Thread the SUT's `cap_set()` into the generator — carry the active `CapSet` on `ReferenceState`
        beside `wiring` (both gates already read `state.wiring`, so this is the least-invasive seam).
        Gate becomes `required_wiring.satisfied_by(wiring) && required_caps ⊆ cap_set`. With a FULL
        cap_set nothing gates → wide PBT byte-unchanged (regression-safe).

   **Increments** (each compile-green + regression-safe on its own):
   `PCG-1` ✅ DONE (2026-06-21) — `required_caps() -> Vec<CapId>` default-empty on `TransitionFactory`
     (`lib.rs:81`, cap-analog of `required_wiring()`; owned `Vec` since `CapId::of` isn't `const`). Pure
     no-op: nothing calls it yet, default needs no impl changes; holon-pbt-core + holon-integration-tests
     both compile clean.
   `PCG-2` ✅ DONE (2026-06-21) — `ReferenceState.cap_set: Option<CapSet>` (`None` = unrestricted →
     regression-safe) + `caps_available(&[CapId])` + `with_cap_set()`; value-level
     `E2ETransition::required_caps()` macro mirror; cap gate `&& state.caps_available(&…required_caps())`
     wired into ALL wiring-gate sites (`aggregate_transitions` gen, both `__declare_pbt_slice_arm` generator
     arms, both slice replay-skip gates, `stepper::transition_applicable`). Regression: `general_e2e_pbt`
     (full) **PASS**, all slices/nav/composed/subsystem_convergence green (91/92); the lone red
     `general_e2e_pbt_sql_only` is the PRE-EXISTING baseline failure (`sut_capabilities.rs:1637` Loro
     `content_raw` not landed), unaffected (its `cap_set` is `None`).
   `PCG-3` ✅ DONE (2026-06-21) — 43 of 50 transitions now override `required_caps()` with the single cap
     their `TransitionImpl` is bound on (type-safe-correct: a fine-grained-bound body can't call outside
     `S`). The 7 exceptions: 5 peer ops (`AddPeer`/`PeerEdit`/`PeerCharEdit`/`SyncWithPeer`/`MergeFromPeer`)
     stay default-empty — `SutLoro` **isn't dyn-compatible** (bare `async fn` → no `dyn SutLoro`, so no
     `CapId::of`), and they're already wiring-gated on `HasStorage(Loro)`; `Nothing`/`DeliverBlockContent`
     need no cap. Guard test `transitions::required_caps_guard` (50-row table, type-level) locks the mapping.
     **DISCOVERY for PCG-4**: hosting `SutLoro` on `CapMap` needs `#[async_trait(?Send)]` (dyn-compat) on
     top of the `&self`/interior-mut flip + `capability!` + adapter. All slices/guard green (49/49).
   `PCG-4` ✅ DONE (2026-06-21) — `SutLoro` flipped `&mut self`→`&self` + `#[capmap_adapter]` (emits
     `#[async_trait(?Send)]` for dyn-compat + `CapName` + `impl SutLoro for CapMap`). `LoroSut.peers` →
     `RefCell<Vec<…>>` (the only structural mutation; the lone `push` and every indexed read are short
     scoped borrows, never across an `.await` → sound). `E2ESut`'s impl → `&self` via a new `loro()`
     accessor (`loro_mut` deleted). The 5 peer transitions now declare `required_caps=[SutLoro]` (guard
     updated). **`CapMap: SutHandle` now HOLDS** (compile-probed) → the wide `E2ETransition` enum dispatches
     over `&mut CapMap`. Regression: `general_e2e_pbt` (full, exercises real peer sync) **PASS**, zero
     `BorrowMutError`, 50/51 (lone red = pre-existing `sql_only` baseline); gpui/tui clean.
   `PCG-5a` ✅ DONE (2026-06-21, EARLY/pre-PCG-4) — `builder::tests::wide_alphabet_narrows_to_partial_compose_sut_capset`
     proves the wide alphabet (`aggregate_transitions`) **auto-narrows to a REAL partial `compose_sut`
     cap set** (the generation-side North-Star property). Over `compose_sut([Turso])`'s cap set
     `{SutBackend, SutSqlProjection, SutBlockTreeWrite}`: (A) deterministic discrimination via
     `caps_available` — block-tree transitions feasible, focus/editor/watch/view/lifecycle/mutate/arrow
     gated out; plus the **cap gate is strictly finer than wiring** (`NavigateFocus` passes Turso wiring
     yet is cap-excluded). (B) integration over `aggregate_transitions`: `narrow ⊊ full`, every dropped
     variant is cap-infeasible (the gate's exact reason — fixture ops need `SutFixtureFs`), and every
     narrow sample is cap-feasible (gate applied, never bypassed → no absent-cap `expect` panic).
   `PCG-5b` ✅ DONE (2026-06-21) — `structural_pbt::teeth::wide_e2e_transition_drives_composed_capmap`
     drives the production WIDE `E2ETransition` enum over a composed `CapMap` (not `E2ESut`): wraps
     `SplitBlock` in `E2ETransition`, sets the gate RHS via `with_cap_set(caps.cap_set())` (asserts the gate
     admits it), dispatches `<E2ETransition as TransitionImpl<_, CapMap>>::apply_to_sut` (the path
     `CapMap: SutHandle` unlocked), reconciles, composed catalog green + non-vacuous. This is the exact
     dispatch the wide-macro SUT swap will use. **Remaining sub-piece ✅ DONE (2026-06-21): the peer
     transitions now drive over a Loro `CapMap`.** `impl CapProvider for LoroSut` (`sut_loro.rs`) hosts the
     `SutLoro` cap (the `&self` PCG-4 flip made it dyn-compatible), and `compose_sut`'s Loro arm now builds the
     peer mesh: it constructs a `LoroDocumentStore`, hands the read component (`LoroBackendComponent`) a
     backend over its **global doc**, and registers a `LoroSut::new(doc_store, None, empty_doc_uri_map)` over
     the SAME doc — so a merged peer block lands in the store the read caps observe. (`sync_handle=None`: a
     pure-Loro config has no `LoroSyncController`/SQL mirror, so `wait_for_quiescence` no-ops; empty
     `doc_uri_map` = identity resolution since Loro honors the reference's provided ids; the loro-only fast
     config never persists, so the tempdir drops immediately after `get_global_doc` caches the doc in memory.)
     Test `builder::tests::compose_sut_loro_arm_drives_peer_transitions` drives `AddPeer`→`PeerEdit(Create)`→
     `MergeFromPeer` over the WIDE `E2ETransition` enum on the composed `&mut CapMap` and asserts the merged
     block appears in the shared primary store (the end-to-end shared-doc proof — a non-shared store would
     leave it invisible); `compose_sut_loro_only_caps` updated to assert `SutLoro` is now present. Regression:
     full `general_e2e_pbt` (real peer sync) PASS, builder/loro/composed suites green (the lone red is the
     pre-existing `general_e2e_pbt_sql_only` baseline, `cap_set=None`, unaffected). Then the wide-macro SUT
     swap (§5) + parity gate.
4. **Generalize the reconcile** (doc-uri, then focus/caret, then peer) as Phase-C ports land — each
   widens the swappable config set.
5. **Swap `__declare_pbt_full_slice!`** SUT from `{ inner: E2ESut }` to `ComposedSut`, behind a
   parity gate (run both, assert identical verdicts) → then delete `E2ESut` impls (E3) and `E2ESut`
   itself + `Subsystem`/`min_sut` machinery (E5). → **all slices gone; one configurable PBT remains.**

   **★ SWAP increment 1 ✅ DONE (2026-06-21): the production wide enum drives a multi-step PBT over the
   composed frontend `CapMap`.** `frontend_slice/structural_pbt.rs::frontend_wide_pbt` — a real
   `StateMachineTest` (`ComposedSut<WideFrontend>`, 16 cases × 1..8 steps) that drives the **production
   `E2ETransition` enum** (the exact `<E2ETransition as TransitionImpl<_, CapMap>>::apply_to_sut` dispatch
   the general_e2e swap will use — PCG-5b proved 1 step; this is a random sequence) over the **FULL**
   `HeadlessFrontendComponent` cap set (same caps `compose_sut`'s frontend arm assembles: `register` +
   `SutSqlProjection` + resolver-sharing writer), checked by the **FULL** composed catalog. All 8 REQUIRED
   invariants run+pass each tick — block (`no-orphan`/`no-parent-cycles`/`blocks-match/block_raw`/
   `block-parent`), **org** (`blocks-match/org`), **focus/nav** (`navigation-focus`/`focus-roots`), and
   **viewmodel** (`viewmodel-no-error-widgets`) — the coverage the minimal-cap `FrontendStructural` slice
   can't reach. Boot recipe = the working tree AS the boot org (page-rooted leaf siblings, pinned `:ID:`)
   so store+org share one source (org invariant green even after structural Split, a non-obvious result:
   the headless session writes structural ops back to org); initial focus DRIVEN onto the page root on the
   SUT (fresh-drive model). 75s; frontend+composed suite 56/56, no regression.

   **★ SWAP increment 2 ✅ DONE (2026-06-21): `frontend_wide_pbt` now drives over the REAL `compose_sut`
   builder — not a hand-rolled lookalike.** `boot_and_seed_wide` previously assembled the CapMap inline
   ("the same caps `compose_sut`'s frontend arm provides"); it now DELEGATES to the production builder via the
   new `compose_sut_seeded(set, resolver, frontend_seed_org)` (`compose_sut` = `compose_sut_seeded(..,
   DEFAULT_FRONTEND_SEED_ORG)`). The wide slice passes `set = {Turso, ViewModel, EditorState}` and its working
   tree AS the boot org (`structural-page.org`). So the wide PBT's SUT is now assembled by the EXACT
   `compose_sut` the `general_e2e_pbt` swap (§5 step 5) points at — the two can no longer drift. KEY: a faithful
   swap over `compose_sut` REQUIRED the seed-org parameter — the builder hardcoded a minimal `doc0.org`, and an
   engine-only post-boot seed leaves the working tree absent from the org `SutOrgRead` parses →
   `inv-blocks-match-ref/org` diverges; booting the tree AS org keeps store+org one source. The builder's own
   `scaffold_ids` assumes a post-boot engine seed (everything booted = scaffold), so the wide slice recomputes
   scaffold = booted ∖ {parent,c1,c2}. The #3 focus-sink writer + editor READ cap + `SutSqlProjection` now ALL
   come from `compose_sut`'s arms (inline duplicates deleted). `frontend_wide_pbt` green 78s over the real builder; all wide teeth +
   `compose_sut`/`subsystem_convergence`/nav/memory green; only the pre-existing `general_e2e_pbt_sql_only`
   baseline red.

   **★ SWAP increment 3 ✅ DONE (2026-06-21): widened the swap config from `turso_frontend_editor` to the
   FULL `full_headless`.** `boot_and_seed_wide` now passes `ComponentSet::full_headless()` (Turso + Loro +
   ViewModel + EditorState) — the EXACT config `compose_sut(full_headless())` / the §5 `general_e2e_pbt` swap
   targets. This adds the Loro arm's read caps so `inv-loro-no-errors` + `inv-loro-children-match-ref` now RUN
   each tick (added to `WIDE_REQUIRED_INVARIANTS` for non-vacuity — "green" provably includes the loro store
   agreeing with the oracle, not just the Turso projection). MAKE-OR-BREAK probed first
   (`full_headless_static_catalog_probe`, static, 3s): the Loro arm reads the SAME shared global doc the
   frontend's Turso session writes (Full mode: Loro authority → projected to Turso with the same block ids),
   so the harness reconcile's Turso-derived synthetic→real id map resolves Loro too — 20 invariants green
   statically. Then under the wide alphabet (8 transitions, multi-step): `frontend_wide_pbt` green 100s with
   all 10 required invariants (block/org/focus/nav/viewmodel + loro). The curated wide alphabet still excludes
   the Loro PEER transitions (the peer CAPS being present is selection-neutral; driving AddPeer/MergeFromPeer
   needs the peer-mesh reconcile). **The headless swap SUT is now the production `full_headless` CapMap.**
   Remaining toward the full §5 swap: (a) point `general_e2e_pbt` itself at `ComposedSut<WideFrontend>` behind
   the verdict-parity gate; (b) widen the alphabet (the remaining seam/Turso-smell/nav-history transitions).

   **★ SWAP increment 4 ✅ DONE (2026-06-21): split the no-op seam-mutate caps off `SutMutate` (honest
   cap-presence → auto-narrowing).** The blocker for pointing `general_e2e_pbt` at `ComposedSut` is that the
   FULL aggregate generates transitions whose caps the composed `CapMap` carries but only NO-OPS — a fake
   (silent-pass). Fixed for the mutate family: `apply_mutation`/`bulk_external_add` moved OFF `SutMutate` into a
   new `SutSeamMutate` cap. `SutMutate` now = `toggle_state` only (a genuinely composable headless op). `E2ESut`
   provides `SutSeamMutate` (its `block_tree_post_action` seam runs the real `ref_state`-dependent dispatch);
   `HeadlessFrontendComponent` deliberately does NOT (no composed seam yet — a no-op would be a fake). So
   `ApplyMutation`/`BulkExternalAdd` now `required_caps` = `SutSeamMutate` → AUTO-NARROW out of any composed
   alphabet (honest absence) while `general_e2e_pbt` (E2ESut, `cap_set=None`) still drives them via the seam.
   `SutSeamMutate` added to both `SutHandle` supertrait lists (CapMap still satisfies `SutHandle` at compile
   time via the blanket adapter impl; a missing provider only panics if actually called, which auto-narrowing
   prevents). Test `full_headless_capset_admits_toggle_but_not_seam_mutate` proves the discrimination. Regression:
   `general_e2e_pbt` (full, drives the seam-mutate transitions over E2ESut) + `frontend_wide_pbt` + composed
   suite green; only the pre-existing `sql_only` baseline red. **This removes the mutate-family blocker; the
   FULL-aggregate swap still needs: Indent/Outdent (share `SutBlockTreeWrite` with Split/Join so they can't be
   cap-gated out — blocked on the 2 Turso smells), nav-history (Back/Forward/Pin/Unpin — `SutNavHistoryDrive`
   present+real, need the per-boot history-id alignment), and StartApp/SimulateRestart (`SutAppLifecycle` present
   but `unimplemented!()` — deferred-boot lifecycle). Until those, the swap test uses the curated wide alphabet
   (`frontend_wide_pbt`), not the full auto-narrowed aggregate.**

   **★ SWAP increment 5 ✅ DONE (2026-06-21): cleared the 3 named blocker families (dispatched as 3 parallel
   research subagents → execution-ready plans → executed serially). The wide alphabet is now ~16 transitions.**
   - **Nav-history (Back/Forward/Pin/Unpin/Home) — FOLDED.** Probe (`wide_boot_navigation_history_id_probe`)
     proved the wide boot assigns `journals#1, page#2` (next=3) and — critically — that `FocusEditableText` /
     `CreateDocument` (already in the alphabet) write NO `navigation_history` rows, so the AUTOINCREMENT counter
     stays in lockstep. `structural_ref_wired` now mirrors the boot stack (`[journals,page]` cursor 1, page-pin
     id 2, `next_history_id=3`). `PinBlock` targets a FIXED stable seed (`c1`), NOT the weighted generator —
     `SutNavHistoryDrive::pin_block` doesn't resolve oracle→real ids (only `OpDispatchWriter` does), so pinning a
     split-minted synthetic id pinned a GHOST (the one failure found + fixed). `UnpinBlock` layered in
     state-dependently. Teeth: pin lockstep + SUT-only-caught.
   - **SimulateRestart — IMPLEMENTED.** `SutAppLifecycle::simulate_restart` on `HeadlessFrontendComponent`:
     file-touch each tracked org file (faithful to E2ESut, itself a touch not a reboot) + settle block_raw to a
     stable id-set in the cap (no composed seam). Make-or-break (`wide_simulate_restart_lockstep`) proved the
     re-parse is `:ID:`-stable. StartApp stays DEFERRED — the composed SUT is pre-booted (`app_started=true`) so
     its precondition gates it out; `SutFixtureFs` fixture transitions auto-narrow out (cap absent).
   - **Indent/Outdent — UN-BLOCKED, NO PROD FIX NEEDED.** Both filed "Turso smells" turned out STALE on the
     composed `full_headless` path: deterministic teeth (`wide_indent_outdent_roundtrip_lockstep`,
     `wide_indent_then_split_parent_lockstep`) + 2 random sweeps green. #1 (top-level NULL parent_id) never fires
     (page-rooted tree only outdents to the real page block; composed reader tolerates NULL); #2
     (split-of-block-with-children → Loro child-vs-sibling) does NOT reproduce — the Loro-authority→Turso path
     places the new block as a sibling correctly. (Smells were E2ESut-era or already fixed.)
   - **Status:** `frontend_wide_pbt` green ~85-96s with the ~16-transition alphabet; teeth + composed/nav/memory/
     general_e2e green; only the pre-existing `sql_only` baseline red. **Remaining to point `general_e2e_pbt`
     ITSELF at `ComposedSut` (full auto-narrowed aggregate):** verify the other present-cap transitions also
     auto-narrow or drive cleanly — peer (`SutLoro` present in full_headless → AddPeer/PeerEdit would generate;
     needs a frontend+loro peer mesh), `PressKey`/`ArrowNavigate` (E4 geometry), editor `MoveCursor`. The wide
     curated alphabet now covers the headless-feasible structural+nav+editor+lifecycle set.
   - **Alphabet widened to `{Split, Join, NavigateFocus, ToggleState}` ✅ (2026-06-21).** `NavigateFocus`
     joined the structural pair: total, mints no blocks (reconcile no-op), target drawn by the production
     generator from the oracle's focusable descendants → SUT+oracle navigate in lockstep, focus matviews
     stay aligned. This exercises the focus/nav invariants **dynamically** integrated with the
     block/org/viewmodel checks each tick — beyond the nav slice's focus-ONLY check. Then **`ToggleState`
     (the mutate arm)**: `impl SutMutate for HeadlessFrontendComponent` realizes `toggle_state` HEADLESSLY
     via the production `block`/`set_field task_state` op (NOT E2ESut's windowed `state_toggle` clicks,
     which need `SutLayout`/`SutDriver`) — matching `ToggleState::apply_to_ref`'s `Update{task_state}`
     exactly; `apply_mutation`/`bulk_external_add` are faithful `&self` no-ops (as on E2ESut — seam-relocated,
     so they stay OUT of the composed alphabet). `SutMutate` registered on the component (selection-neutral
     write cap) → wide_frontend + nav + `compose_sut` frontend all host it; `ToggleState` self-gates via its
     render/focus generator (fires after a `NavigateFocus` lands focus on a text child). Teeth:
     `wide_frontend_toggle_state_lockstep_stays_green` + `wide_frontend_sut_only_toggle_state_is_caught`
     (blocks-match catches the property divergence) + `wide_frontend_sut_only_navigate_is_caught`.
     `frontend_wide_pbt` green at 61s with all four transitions; `compose_sut(full_headless)` cap-set now
     includes `SutMutate`. 21/21 regression green (nav selection unchanged).
   - **Why not the rest of the alphabet yet:** the 2 filed Turso smells (`Indent`/`Outdent` top-level
     NULL-parent + split-of-parent child-vs-sibling); `NavigateBack/Forward/Pin/Unpin` need the
     nav-history-depth + history-id-counter alignment the dedicated nav slice carries (folding into the
     full-catalog drive is a later increment); editor transitions need committed-content parity; and the
     doc-uri/peer reconcile generalizations. Auto-narrowing the alphabet to the cap set is already proven on
     the GENERATION side (PCG-5a); what's new is DRIVING the production enum multi-step over the full cap set
     + full catalog.
   - **The general_e2e swap path:** the editor arm (2c) is now DONE so `compose_sut(full_headless())`
     **composes** (no longer panics — see 2c above). Remaining increments toward driving it green, in order:
     (a) ✅ editor arm (2c, done); (b) **mutate arm** — `SutMutate::toggle_state` ✅ DONE (headless
     `set_field`, `ToggleState` drives in `frontend_wide_pbt`); `apply_mutation`/`bulk_external_add` still
     need the `block_tree_post_action` seam (reconcile), and `SutFixtureFs` (WriteOrgFile/CreateDirectory/
     GitInit) is still unported; lifecycle is moot — the composed SUT is pre-booted, so `StartApp` cap-gates
     out; (c) **reconcile generalization** (doc-uri minting for CreateDocument/BulkExternalAdd, focus/caret
     for PressKey, peer) + **committed-content parity** (the ref must commit editor text — Design §8.8
     deferred half) so editor + external-add transitions drive green; (d) then point `general_e2e_pbt`
     itself at `ComposedSut` behind the parity gate. Each new arm + reconcile widens the swappable config set
     toward full_headless.
   - **★ EDITOR ARM (committed-content parity) ✅ DONE (2026-06-21) — via the REAL headless editor.** The
     handoff's keystone #1 landed (user chose the production-faithful path: `HeadlessFrontendComponent` hosts
     the production headless editor, NOT the `InMemEditorComponent` stand-in). Correction: editor transitions
     were NEVER E4/geometry-blocked — `FocusEditableText::required_caps=[SutFocusWrite]` dispatches over
     `CapMap`. (1) The component now holds a `ReactiveEngineDriver` (→ prod `HeadlessEditorMirror`):
     `apply_focus_editable_text`=`driver.click_entity(id,"main")` (was `unimplemented!`); new
     `impl SutEditorMirrorWrite` (per-char `send_raw_keystroke` + `settle_block_content`) +
     `impl SutEditorMirrorRead` (caret from mirror, live text from `MutableText`). WRITE cap in the general
     `register` (selection-neutral); READ cap added per-CapMap (it selects `inv-editor-*`). (2) Char typing
     needs a `MutableText`⇒Loro ON: new `new_with_loro(.., loro_enabled)` flips `HolonConfig.loro.enabled`
     AND wires the Loro-backed `BlockCellRegistry` into the reactive engine (the windowless build bypasses the
     frontend `on_start` that normally does this ⇒ else `editable_text` Err "no MutableText"; mirrors E2ESut
     `ensure_reactive_engine`). (3) `frontend_editor_pbt` (`ComposedSut<WideEditor>`, structural_pbt.rs)
     drives the production `TypeChars` multi-step over the real headless editor (Loro on), checked by the FULL
     catalog incl. `inv-block-content-matches-ref/block_raw` (THE committed-content parity check) +
     `inv-editor-{text,caret}-matches-ref`. Oracle `editor_ref` mirrors the SUT boot EXACTLY
     (`NavigateFocus(page)` blur → `FocusEditableText(c1)` open; nav focus = page on both sides is
     load-bearing). Kept SEPARATE from `frontend_wide_pbt` (Loro-off structural) so the structural arm is
     untouched. Teeth: `editor_type_chars_lockstep_stays_green` + `editor_sut_only_type_chars_is_caught`.
     111/111 touched suites + `general_e2e_pbt` PASS. DEFERRED: `DeleteBackward` (caret-0 backspace =
     structural `join_block` = block REMOVAL, which the mint-only per-tick reconcile doesn't model); retire
     `InMemEditorComponent` from `compose_sut`'s editor arm (now the headless component hosts the real
     editor — North Star); fold `frontend_editor_pbt` into the ONE wide PBT once reconcile handles removal.
     This supersedes (c)'s "committed-content parity" sub-item for the frontend editor path.
   - **★ #1 (compose_sut→real editor) + #2 (ONE combined wide PBT) ✅ DONE (2026-06-21).** #1: the
     `compose_sut` frontend arm now boots `new_with_loro(.., has_editor)` and registers the headless editor
     READ cap, so frontend/full_headless configs host the PRODUCTION headless editor; the
     `InMemEditorComponent` stand-in is used ONLY for non-frontend editor configs (`has_editor && !has_frontend`).
     #2: the standalone editor PBT is FOLDED into the single `frontend_wide_pbt`, which now drives the full
     7-transition alphabet `{Split, Join, NavigateFocus, ToggleState, FocusEditableText, TypeChars,
     DeleteBackward}` over the Loro-on headless editor + full catalog (with a non-vacuity guard that the editor
     transitions generate + chain). FINDINGS that corrected the plan: (a) `DeleteBackward` needs NO
     removal-reconcile — without `MoveCursor` the caret stays at end-of-text so backspace never reaches caret 0
     (the only join/removal state); (b) the real editor↔structural desync is that `OpDispatchWriter` dispatches
     structural ops through raw `BackendEngine::execute_operation`, bypassing the frontend split/join
     focus-handoff (`apply_structural_focus`) prod does via the op response — so the composed SUT leaves no open
     editor after a split while the ref opens one. FIX: `WideMachine::apply` blurs the editor after `Split`/`Join`
     (composed SUT leaves no editor after structural ops; editors open only via `FocusEditableText`). 106/106 +
     `general_e2e_pbt` green; combined PBT stable across 4 runs.
   - **#3 (focus-handoff fold) ✅ DONE (2026-06-21):** `OpDispatchWriter` gained an optional frontend focus
     sink (`with_resolver_and_focus(engine, resolver, reactive)`). When present (booted-frontend configs),
     `split_block`/`join_block` dispatch through the PRODUCTION `ReactiveEngine::dispatch_intent_sync`
     (`execute_operation` + `apply_structural_focus`) instead of raw `BackendEngine::execute_operation`, so the
     op-response focus result moves the SUT's `focused_block` (+ armed caret seed) onto the new/merged block —
     exactly as `SplitBlock`/`JoinBlock::apply_to_ref` does via `set_focus` + `open_active_editor`. Absent
     (memory/turso storage-only configs) → raw execute, unchanged. The `WideMachine::apply` BLUR WORKAROUND is
     DELETED: the SUT now leaves an editor open on BOTH sides after a structural op, so a `TypeChars` can follow
     a `SplitBlock` directly (true split-then-type). KEY (why the editor invariants don't need extra
     synthetic-id plumbing): `with_resolved_doc_uris` already remaps `active_editor.block_id` synthetic→real, so
     the editor read caps read at the resolved real id; `inv-editor-caret` skips on SUT `None` (no keystroke
     yet); `inv-editor-text` matches because the new block's `MutableText` reflects the post-split content
     regardless of focus. So the blur's ONLY real job was preventing a later `TypeChars` from panicking on "no
     focused block" — the handoff removes the need. New deterministic teeth
     `wide_split_then_type_lockstep_stays_green` (split `c1`, then `TypeChars` with NO intervening
     `FocusEditableText` — only the handoff makes the keystroke land on the new block; editor-text + content
     parity run non-vacuously). 105/106 (`general_e2e_pbt_sql_only` = documented pre-existing baseline red,
     `E2ESut` path untouched by this change); combined `frontend_wide_pbt` green ~85s.
   - **Seam rebuild SR-1 (doc-uri-minting reconcile generalization) ✅ DONE (2026-06-21):** `CreateDocument`
     now drives green over the composed frontend CapMap — the FIRST seam-rebuild increment. (a) `impl
     SutAppLifecycle for HeadlessFrontendComponent` with a real `create_document`: writes an empty org file
     into the session's watched `org_root`; the PRODUCTION `FileSyncController` watcher ingests it and mints the
     doc block in `block_raw`; the action polls `block_raw` until the doc block (matched by title = file stem)
     lands, then returns (no `ref_state`, no resolver). `start_app`/`simulate_restart`/`concurrent_schema_init`
     are fail-loud `unimplemented!()` (not in any composed alphabet — lifecycle is a later increment). Cap
     registered selection-neutrally. (b) The harness per-tick reconcile is generalized from split-ids-only to a
     composed-LOCAL predicate `is_composed_minted_synthetic_id` = `block::split-N` ∪ `block:ref-doc-N` (the
     `next_synthetic_doc_uri` scheme); the global `is_synthetic_ref_id` is deliberately UNCHANGED so E2ESut's
     split-only mapping is unaffected. The minted doc page is one new `block_raw` id paired 1:1 with the oracle's
     one new synthetic doc-uri — the doc-uri case the old `block_tree_post_action` CreateDocument arm handled,
     now generic. (c) `CreateDocument` added to the wide alphabet (`wide_aggregate`); combined `frontend_wide_pbt`
     green ~90s with the 8th transition. (d) The #3 focus-handoff was also applied to `compose_sut`'s frontend
     arm (was only on `boot_and_seed_wide`) for faithfulness on the real swap target. FINDING: `Block.is_page()`
     is FALSE on projected `block_raw` rows (page-ness is a `block_tags` Page tag, not a `Block` field
     post-projection) — match doc blocks by title, not `is_page()`. Teeth: `wide_create_document_lockstep_stays_green`
     (lockstep green, blocks-match runs non-vacuously) + `wide_sut_only_create_document_is_caught` (SUT-only mint
     CAUGHT). 107/108 (`general_e2e_pbt_sql_only` = documented pre-existing baseline red, `E2ESut`+global predicate
     untouched).
   - **Seam rebuild remaining pieces:** `BulkExternalAdd` (multi-block mint + full-doc org serialize + the
     resolver/`documents` plumbing the component lacks), `ApplyMutation` (resolver plumbing onto the `&self`
     `SutMutate` cap + generator can't restrict to reconcile-clean Update/Delete/Move), `StartApp`/`SimulateRestart`
     (deferred-boot lifecycle), peer dispatch. Each needed only when its transition enters the alphabet.

   - **★ THE SWAP — `general_e2e_composed_pbt` (auto-narrowed full alphabet) ✅ DONE (2026-06-22).** Promoted
     the curated wide PBT to the PRODUCTION `aggregate_transitions` generator: `general_e2e_composed_pbt`
     (`ComposedSut<WideE2E>`) drives the production `E2ETransition` enum via `aggregate_transitions` (NOT a
     curated list) over `compose_sut(full_headless())`, checked by the full composed catalog. The ref carries
     `full_headless` wiring + `cap_set` so the alphabet **auto-narrows** to the composed SUT's drivable caps —
     **green at `PROPTEST_CASES=40 × 1..8` (193s)**, driving 26 of the 28 cap-feasible transitions (the proven
     curated 16 PLUS the newly-converged `MoveUp/Down`, `MoveCursor`, `Redo`, `UndoLastMutation`, `SwitchView`,
     `EmitMcpData`, `ConcurrentSchemaInit`; `StartApp` precondition-gates out). Probe
     `swap_probe_full_headless_narrowed_alphabet` documents the 28 feasible / 16 auto-excluded (peer×5,
     seam-mutate×2, BlockInteract×5, ArrowNavigate, fixture×3) — the E4/seam/fixture set narrows by
     cap-absence for free. **Three enablers:**
     1. **Honest peer narrowing** — `compose_sut` now gates the peer `SutLoro` registration on `!has_turso`
        (Loro-canonical only). In `full_headless` the canonical `SutBackend` is the Turso projection while the
        Loro arm builds a SEPARATE doc store; a peer-merged block would be invisible to the invariants' backend,
        so claiming a drivable `SutLoro` there was dishonest. Now peer auto-narrows out of `full_headless`; the
        pure-Loro config keeps it. (`compose_sut_full_headless_composes` updated: `SutLoro` now `is_none()`.)
     2. **Synthetic-id ghost fix** — the production `PinBlock`/`NavigateFocus` generators target post-split
        synthetic `block::split-N` ids; the nav/focus caps dispatched them literally → ghost pins
        (`inv-focus-roots` diverge). Threaded the shared `IdResolver` into `HeadlessFrontendComponent`
        (`OnceLock<IdResolver>` + `set_resolver`, wired in `compose_sut`'s frontend arm); `pin_block`/
        `apply_navigate_focus`/`apply_focus_editable_text` now `resolve_id` (oracle→real, identity for stable
        seeds), mirroring `OpDispatchWriter::resolve`.
     3. **Deliberate narrowing of deferred-convergence families** via a new framework primitive
        `CapSet::without(CapId)` (holon-pbt-core/composition.rs). **As of 2026-06-22 NO narrowing remains** —
        `wide_e2e_ref` uses the full `full_headless_cap_set()`. Both families that were narrowed converged:
        `SutMutate`/`ToggleState` (task #4) and `SutWatchRegister`/`SetupWatch`/`RemoveWatch` (task #5, B5 —
        see below). `CapSet::without` stays in the framework for future use.

   - **★ MACRO REPOINT ✅ DONE (2026-06-22).** `general_e2e_composed_pbt` now runs as a PRODUCTION INTEGRATION
     TEST (`tests/general_e2e_composed_pbt.rs`), driving `ComposedSut<WideE2E>` — ADDITIVE alongside the
     `E2ESut`-backed `general_e2e_pbt`. The architectural blocker was that the macro/integration tests link the
     lib built WITHOUT `cfg(test)`, but the composed harness was `cfg(test)`-only. Resolved by: (1) un-gating
     `composed::harness` + `composed::subsystem_seed` from `#[cfg(test)]` → `#[cfg(any(test, feature = "pbt"))]`
     (`pbt` is a default feature; the only coupling was a `fixtures::*` glob in `subsystem_seed`, replaced with
     direct imports); (2) relocating the `WideE2E` slice machinery (`page_root`/`SETTLE`/`WIDE_TREE_ORG`/
     `structural_ref{,_wired}`/`wide_ref`/`wide_e2e_ref`/`full_headless_cap_set`/`boot_and_seed_wide`/
     `WIDE_REQUIRED_INVARIANTS`/`WideE2E{,Machine}`) into a new pbt-gated `crate::pbt::composed::wide_e2e` module
     as the **single source of truth** — `structural_pbt.rs` now `use`s it (no duplication; `frontend_wide_pbt`
     + teeth + `swap_design_probe` stay and consume it); (3) the thin `tests/` entry. GREEN: integration test
     8 + 40 cases (36s/193s), `frontend_wide_pbt` 80s via the relocated imports, lib regression 40/40. The
     composed test runs ALONGSIDE `E2ESut` `general_e2e_pbt` (the composed alphabet auto-narrows out
     peer/E4/watches/mutate/fixtures), so `E2ESut` stays the wide-coverage reference until those converge — at
     which point its headless cap impls (E3) and the struct itself (E5) can be deleted.

   - **★ `ToggleState` / `inv-task-state-storage-coherence` (task #4) ✅ DONE (2026-06-22).** `ToggleState` now
     drives in the composed swap alphabet (the `SutMutate` narrowing is removed). Three changes + two real prod
     bugs the faithful composed SUT exposed:
     1. **Read-doc unification:** `compose_sut`'s Loro arm builds the read cap (`LoroBackendComponent`) over the
        FRONTEND's authority `LoroDocumentStore`'s global doc (when the frontend booted with Loro on), not a
        separate `tempdir` store — so a write through the frontend op pipeline is visible to `SutLoroTaskState`/
        `SutLoroLog`. New accessor `HeadlessFrontendComponent::loro_doc_store()` (`try_resolve` +
        clone-shares-global-doc); captured in the frontend arm, used in the Loro arm. This also made
        `inv-loro-children-match-ref` run NON-vacuously over `full_headless` (it was previously skipped — the
        separate doc was empty).
     2. **Faithful toggle:** `HeadlessFrontendComponent::SutMutate::toggle_state` now dispatches the real
        `cycle_task_state` op `cycle_click_count(current, target)` times (the op Cmd+Enter / the `state_toggle`
        widget fires) instead of a `set_field task_state` shortcut — so it exercises the production toggle path
        (`LoroBlockOperations::cycle_task_state` → Loro authority → `block_raw` projection).
     3. **`wide_e2e_ref` drops the `.without(SutMutate)`; the lockstep tooth is un-`#[ignore]`d.**
     - **PROD BUG #1 FIXED:** `LoroBlockOperations::set_state` wrote `set_field(id, "TODO", …)` but the canonical
       key is `task_state` (org parser, SQL provider, `Block::task_state()` all agree) — so `cycle_task_state`
       (read `task_state`, write `TODO`) was a NO-OP in Loro mode: **Cmd+Enter never advanced the task keyword in
       production** (Loro is the default block `OperationProvider` via the OrgMode DI factory). Reproduced first
       (the tooth showed SUT `properties{"TODO":"TODO"}` vs ref `{"task_state":"TODO"}`), then fixed.
     - **PROD BUG #2 FIXED:** `LoroBackend::list_children` resolved a block `parent_id` via the id_cache only,
       which is EMPTY on a `from_document`/peer-attached backend → "Cannot resolve parent_id to TreeID" for a
       present block. Now uses the shared `resolve_parent_tree_id` (TreeID → cache → tree-walk-by-`STABLE_ID` →
       populate). Exposed by change (1).
     - KEY insight: `inv-task-state-storage-coherence` is a projection-faithfulness check (SQL is a pure Loro
       projection), so it ALWAYS held on `E2ESut` (which masks both bugs via its hand-rolled `loro_sut` feed). The
       composed SUT over the REAL production DI is the first to expose them. Verified: `general_e2e_composed_pbt` +
       `frontend_wide_pbt` green @40 cases; `holon-loro` 103/103; `general_e2e_pbt` (E2ESut) green; sole red =
       pre-existing `general_e2e_pbt_sql_only` TypeChars baseline. My earlier guess that #4 would also fix #5 was
       wrong — they are independent.
   - **★ `SetupWatch`/`RemoveWatch` watch-query parity (= B5, task #5) ✅ DONE (2026-06-22).** Was the last
     narrowing. `generate_test_query` always emits `QuerySource::AllBlocks`, so the watch returns the whole block
     set: the booted SUT's 11 scaffold blocks (9 `index.org` + `journals` page shell + `__default__`) vs the
     oracle's phantom `started-ref-layout-query` diverged the exact full-set compare. FIX = **faithful oracle
     modeling** (a first seed-exclusion attempt was rejected — it never verifies the layout blocks): the oracle now
     MODELS the real layout the SUT boots. `StartApp::apply_to_ref`'s layout-seeding was extracted into
     `seed_booted_layout_into_ref` (`transitions/start_app.rs`, behavior-preserving), and `build_started_ref`
     (`composed/subsystem_seed.rs`) calls it instead of the phantom — so `inv-watch-rows-match-ref` compares the
     full block set on both sides. Journals.org's body is excluded (the SUT skips it). `.without(SutWatchRegister)`
     dropped from `wide_e2e_ref`. Verified: `general_e2e_composed_pbt` @40 + `frontend_wide_pbt` @40 +
     `general_e2e_pbt` (main) green; sole red = pre-existing `general_e2e_pbt_sql_only` TypeChars baseline.
     Confirmed INDEPENDENT of task #4 (an oracle-block-set modeling gap, not Loro-doc topology). **With #4 + #5
     done, NO narrowing remains — the composed swap drives the full auto-narrowed production aggregate.** E3/E5 now
     gate only on E4 (windowed `GpuiWindowComponent`), the full-mode peer mesh, and the E2 attended parity run.

   - **✅ FULL-MODE PEER MESH DONE (2026-06-22, Part A).** The composed builder withheld `SutLoro` in
     `full_headless` (`builder.rs` `!has_turso` gate), so the peer transitions
     (`AddPeer`/`PeerEdit`/`MergeFromPeer`/`SyncWithPeer`) auto-narrowed out of the swap. Closed by registering
     `LoroSut` over the **frontend's shared authority `LoroDocumentStore` + real `LoroSyncControllerHandle`** in
     full mode (the exact analogue of E2ESut `sut_handle.rs:194-199`): a merged peer delta imported into the shared
     global doc wakes the controller (`loro_sync_controller.rs` `subscribe_root → run_loop → project`) and lands in
     the canonical Turso `block_raw` the block invariants read. Key facts (verified, not assumed): (1) the headless
     frontend DOES start the controller (shared `FrontendSession` factory, not GPUI-only), but via
     `without_wait()`→`tokio::spawn` so the handle resolves RACILY — the builder POLLS for it (≤2s) and fails loud if
     absent; (2) the projection is ASYNC — `apply_merge_from_peer`/`apply_sync_with_peer` already
     `wait_for_quiescence(sync_handle)`, which becomes active once the real handle is wired, so the per-tick `after`
     snapshot sees the projected row; (3) peer blocks carry a STABLE deterministic `block:peer-…` id identical on
     oracle and SUT (no UUID minted → identity `doc_uri_map` resolves them) — so the original "id bridge" plan was a
     non-problem. The REAL gate was the per-tick reconcile's `assert_eq!(synthetic.len(), real_new.len())`: a
     `MergeFromPeer` surfaces a fresh `block:peer-…` row with no matching synthetic id → it would PANIC. Fixed by
     excluding `block:peer-…` ids from `real_new` (`harness.rs::is_peer_scheme_id`). Gate `!has_turso` → `!has_turso
     || frontend_sync_handle.is_some()` (non-frontend configs unchanged). Tests: `headless_loro_sync_controller_
     resolves_after_boot` (A0 readiness probe), `compose_sut_full_headless_peer_mesh_projects_to_turso` (teeth:
     drive AddPeer→PeerEdit→MergeFromPeer, assert merged `block:peer-…` row in Turso `SutBackend`),
     `full_headless_cap_set_admits_peer_transitions` (deterministic auto-select proof). `general_e2e_composed_pbt`
     green @24 with peer transitions in the alphabet; `peer_conflict_pbt` + `general_e2e_pbt` (main) no regression.
     Stale `compose_sut_full_headless_composes` assertion flipped `SutLoro.is_none()`→`is_some()`. E3/E5 now gate only
     on E4 and the E2 attended parity run.

   - **✅ E4 PER-TICK WINDOWED LOOP DONE (2026-06-22, Part B).** `run_selected(composed_invariant_catalog)` over the
     windowed `CapMap` now runs on EVERY post-StartApp tick of the gpui sim loop, not just the one deterministic
     `gpui_window_slice` tick. A per-tick hook `FnMut(&mut S, &M::State)` was threaded through `replay_steps`
     (`fixtures/mod.rs`) → `replay_fixture_with_driver_sync_callback` (`phased.rs`) → `SimReplayer::replay`
     (`sim_windowed_replay.rs`); the hook captures the window `bounds` + the frontend engine (shared from `on_ready`),
     builds `window_focus_wide` + a ref `CapMap` from the LIVE `ReferenceState`, and runs the catalog. **B0 finding
     (the make-or-break my senior review flagged): a tokio runtime IS entered on the gpui thread at the hook point**
     (`random_pbt_sim.rs:102` keeps a multi-thread `window_rt.enter()` guard active for the whole replay), so
     `Handle::block_on`/`block_in_place` would PANIC. Solved with `futures::executor::block_on` (runtime-agnostic —
     polls on the gpui thread while the invariants' tokio awaits still resolve against the entered multi-thread
     runtime; the window is settled to a fixed point BEFORE blocking so no further pumping is needed). Per-tick cost
     (measured): ~80-130ms settle + ~10-100ms catalog (`ran=5` windowed invariants incl. `inv-frontend-bounds-
     rendered`, O(blocks) growing). Gated OPT-IN behind `HOLON_PBT_WINDOWED_CATALOG=1` (default-off keeps existing
     windowed runs fast; the `E2ESut` per-step `check_invariants` still runs in parallel — purely additive), with a
     non-vacuity guard (`inv-frontend-bounds-rendered` must run each tick). 149+ ticks green (`failures=0`). The
     overall `gpui_ui_pbt_sim` has a PRE-EXISTING `DeleteBackward` editor-keystroke flake (reproduced with the catalog
     OFF — `invariant_runner.rs:914` — part of the human-gated E2 attended suite), orthogonal to this loop.

#### The decomposition mastermind plan — risks figured out up front (2026-06-19)

This subsection is the **distilled endgame**: the exact structural keystone, the risk
register sorted by gating severity, the up-front spikes that kill each make-or-break
*before* per-cluster work begins, and the parallelizable peeling plan once they're green.
It is the synthesis the increments were circling. Read it before scheduling E1–E5.

##### The structural keystone (state it exactly, once)

The aggregate driver is **one** blanket impl in `transition_dispatch.rs:453-473`:

```
impl<S: SutHandle + SutFocusWrite + SutNavHistoryWrite> TransitionImpl<ReferenceState, S> for E2ETransition
```

So **the SUT that drives the full alphabet must satisfy `SutHandle` in its entirety.**
`SutHandle` is *not* `#[capmap_adapter]`-hostable and never can be: it uses native
`async fn` (not `#[async_trait]`), ~38 of its own methods are `&mut self`, and several
carry default `unimplemented!` bodies — all three make it object-unsafe / un-forwardable
through the `Arc<dyn Cap>` map. Therefore **a `CapMap` cannot drive the full alphabet
until `SutHandle` has *no own methods left*.** This — not the union-of-bounds probe — is
the load-bearing fact. Every green single-cluster slice proves the rebind *mechanic*; the
endgame is gated on **emptying the trait**.

The elegant end state (the recommendation): peel **every** own method off `SutHandle`
into a `&self` + interior-mut + `#[capmap_adapter]` cap, added back as a **supertrait**.
`SutHandle` collapses to a pure marker bundle:

```
pub trait SutHandle: SutBlockTreeWrite + SutEditorMirrorWrite + SutFocusWrite
    + SutNavHistoryWrite + SutLifecycle + SutWatchWrite + SutOrgFsWrite + … {}     // NO own methods
impl<T: /* all the supertraits */> SutHandle for T {}                              // blanket
```

Once `SutHandle` has no own methods, the transitional method-name clashes vanish
(`SutFocusWrite::apply_focus_editable_text` only clashed with `SutHandle`'s *own*
copy — gone), so `SutFocusWrite`/`SutNavHistoryWrite` fold back in as supertraits and the
macro bound collapses to plain `S: SutHandle`. `CapMap` — already satisfying every cap via
`#[capmap_adapter]` — then satisfies the blanket impl **for free**, with no hand-written
`impl SutHandle for CapMap` (which would be the rejected panicking-Move-B). This is the
clean dual of how `WideProxyCaps` already works on the read side.

##### Risk register (sorted by gating severity)

| # | Risk | Why it gates | Verdict |
|---|---|---|---|
| **A** | **`SutHandle` cannot empty** — some method is intrinsically `&mut self` and resists the `&self`+interior-mut flip | Until empty, `CapMap` can't satisfy the enum bound → no full-alphabet composed run → E3/E5 blocked | **THE keystone.** Proven flippable: blocktree, editor, focus, nav-home (stores are `Arc<Mutex>`/`Arc<RwLock>`), and the **watch cluster** (`setup_watch` — INC 3 wrapped `active_watches`/`watch_queries`/`ui_model` in `RefCell`; sound because no borrow crosses an `.await` and `E2ESut` is `?Send`/`block_on`-driven; `general_e2e_pbt_full` passes). Unproven: lifecycle (`start_app` *builds* fields), `press_key` (mutates `doc_uri_map`). Kill with **EXP-4** + **EXP-2**. |
| **B** | **Out-of-`apply_to_sut` harness seams** — `block_tree_post_action` (split-id reconciliation + caret sync), `pre_ref_state` carry-forward, `build_doc_uri_map`, `settle_before_invariants` all live in `E2ESut`'s `StateMachineTest` glue, *not* in `apply_to_sut` (`sut_check_invariants.rs:34-309`, `invariant_runner.rs:285-369`) | A composed full `StateMachineTest` must re-host every one of these. The two single-cluster slices *sidestep* them (synchronous backends, fixed ids, `set_next_split_id`) — so they prove **nothing** about this risk | **The real integration risk, under-acknowledged so far.** Kill with **EXP-2 + EXP-3**. |
| **C** | **Async settle / id reconciliation over a real backend** — split mints a real UUID that ≠ the oracle's synthetic `block::split-N`; `set_next_split_id` (the memory-slice trick) cannot work over Turso/async-Loro — you need the `map_unmapped_split_synthetic_ids` map-back, plus the convergent `BlockQuerySource` content-settle | Without it, `SplitBlock`/`CreateDocument`/`WriteOrgFile` produce false divergence or CDC-lag `Skipped` instead of `Fail` | Sub-risk of B; the part the slices can't fake. Kill inside **EXP-2** (mixed alphabet *must* include `SplitBlock` over the async headless component). |
| **D** | **Union-of-bounds tractability & method-name clashes** | A clash (like the focus one) or a type-checker blowup as ~20 caps stack would stall peeling | Low — 3-cap union already PASS; clashes are transitional (vanish when `SutHandle` empties). Cheap to monitor with **EXP-1**. |
| **E** | **`&mut self` apply-path caps that must stay `&mut`** — `SutCdc::drain_cdc`, `SutOrgFileWrite::write_org_file`, `SutLifecycle::apply_start_app` are declared `&mut self` and `#[capmap_adapter]` emits `unimplemented!` for them | Any transition whose cap method is `&mut self` is *undrivable* through `CapMap` | Forces the discipline: **every cap in the alphabet must be `&self`.** `drain_cdc` is apply-only (§7, never a transition) so it's fine; `write_org_file`/`start_app` must flip (covered by their clusters). |
| **F** | **Peer/sync cluster** (`AddPeer`/`SyncWithPeer`/`MergeFromPeer`/`PeerEdit`/`PeerCharEdit`) needs a multi-peer `LoroSut` built in `start_app` — no component hosts it | These transitions can't join the composed alphabet without a peer/sync component | Self-contained cluster; defer behind the keystone, build alongside the Loro component. |
| **G** | **The 4 `&mut self` self-invariants** (`native_self_invariants`, `invariant_runner.rs:605-618`) | They run over raw `&mut E2ESut`, not the proxy | Largely retired: editor caret/text → `SutEditorMirrorRead` (`&self`), window-focus → `SutDriver` (E4 inc4). Residue = `sql-budget` (otel). Low. |
| **H** | **Pre-existing nondeterministic `apply_type_chars` Loro settle-race** (`content_raw` not landed before `TypeChars`; flakes `general_e2e_pbt_sql_only` on random ids) | Will reappear in the composed full-alphabet run and break the "non-flaky" gate | Root-cause it as part of **EXP-3**'s settle seam — it is almost certainly the same missing barrier. |

##### De-risking experiments — kill the make-or-breaks before per-cluster work (Step-A discipline)

These are **cheap, up-front, and ordered**. Do not start parallel cluster peeling until
**EXP-2** is green — it is the single experiment that proves the endgame is reachable.

- **EXP-1 🤖 — the growing full-union compile probe (kills D, monitors A).** One
  `#[cfg(any(test, feature = "pbt"))] fn _assert_full_alphabet_union<S: SutBlockTreeWrite
  + SutEditorMirrorWrite + SutFocusWrite + SutNavHistoryWrite + SutLifecycle + … >() {}`
  enumerating the **target union** (every cap the emptied-`SutHandle` bound will require),
  instantiated with the planned composed component type. It won't fully compile until all
  caps are hosted — that's the point: it's the **living checklist** of remaining caps, and
  it flags any method-name clash the instant two caps collide. Extend it as each cluster
  lands. ~20 lines. (Mirror of `_assert_capmap_hosts_proxy_bodies`.)

- **EXP-2 🧠 — the make-or-break: wide composed `StateMachineTest`, mixed alphabet, one
  `CapMap`, real backend (kills A/B/C).** Stand up a `StateMachineTest` whose SUT is a
  `CapMap` over the **headless SQL+frontend component** (the async one, *not* a synchronous
  in-process store), driving a **deliberately heterogeneous** mini-alphabet —
  `{SplitBlock, TypeChars, Indent, ToggleState}` — through **one** `apply_to_sut`. It
  **must** include `SplitBlock` so the synthetic-id map-back (risk C) is exercised, not
  faked. Port `block_tree_post_action` + `pre_ref_state` carry-forward + the pre-invariant
  settle into a reusable composed runner (→ EXP-3). **Success = the doc's exact open
  question answered:** "can a `StateMachineTest` drive a heterogeneous mixed alphabet
  through one `CapMap` [over a real async backend]?" If green, the endgame is smooth sailing
  and every remaining cluster is a parallelizable ticket. If it stalls, the stall *is* the
  real plan, surfaced before sinking effort into 8 clusters.

- **EXP-3 🧠 — the one shared settle/reconciliation seam (kills B/C/H), built inside
  EXP-2.** Extract `settle_before_invariants` + `build_doc_uri_map`/`with_resolved_doc_uris`
  + `map_unmapped_split_synthetic_ids` into a **`ComposedRunner`** (the long-deferred
  `RegistryHost` — Design §8 Step 1) that wraps `run_selected` with: (1) a per-realization
  settle (`SutQuiesce::quiesce` for in-process; a CDC/content fixed-point poll for async —
  reuse `settle_on_snapshot`), (2) the doc-uri resolution, (3) the split-id map-back. This
  is the seam every composed slice has been avoiding; building it once unblocks all of E1–E5
  *and* is the natural home to root-cause risk **H** (the `apply_type_chars` race is almost
  certainly a missing content barrier this seam will own).

- **EXP-4 🧠 — the `StartApp` / lifecycle model decision (kills A's hardest case).** Decide
  and prove: does the composed SUT (a) boot its session in `init_test` and gate `StartApp`
  out of the composed alphabet, or (b) flip `SutLifecycle::apply_start_app` to `&self` +
  an `OnceLock<Session>` so `StartApp` *is* a real transition (the convergence harness's
  "startup is covered" goal)? Spike (b) as a probe on `HeadlessFrontendComponent`; if the
  `&self`+`OnceLock` boot is clean, lifecycle stops being special. **Recommendation:** (b),
  because the convergence harness already values covering boot — but confirm with a probe,
  cost is one afternoon.

##### EXP-2/3/4 — de-risking RESULTS ✅ (2026-06-19, worktree `pbt-decomp-derisk`)

The gating experiments are **green**. The central make-or-break — *can a composed
`CapMap` drive structural writes over a real async backend, reconcile the oracle's
synthetic split ids against the store's minted ids, and agree with the real
`ReferenceState` oracle across a multi-tick `StateMachineTest`?* — is **proven**.
Code: `crates/holon-integration-tests/src/pbt/sut_handle_decomp_spike.rs` +
`op_write_cap.rs` (+ a robustness fix in `sut_row_parsing.rs`). Gate: `cargo test -p
holon-integration-tests --features pbt --lib sut_handle_decomp_spike` → 4 passed;
regression (`sql_slice`+`frontend_slice`+`composed`+spike) → 62 passed, 0 failed.

- **C1** `exp2_async_split_through_capmap_mints_real_id` — `split_block` driven *through
  the `CapMap`'s `SutBlockTreeWrite` cap* over real Turso mints a fresh real id +
  truncates the block. Async structural write path works via composition.
- **C2 positive** `exp3_async_split_reconciled_against_oracle_passes_invariants` — split
  on both oracle (synthetic `block::split-N`) and Turso `CapMap` (real `uuid`), then
  synthetic→real **map-back** + `with_resolved_doc_uris`, and the catalog's block-tree
  invariants **agree with the oracle**. The reconciliation the `MemoryBackend` slice
  sidesteps works over an id-minting async backend.
- **C2 teeth** `exp3_unreconciled_split_is_caught` — skip the map-back → divergence
  caught (non-vacuous).
- **Full** `exp2_full_async_structural_state_machine` — a real `prop_state_machine!`
  (48 cases × ≤12 steps) drives `{SplitBlock, Indent}` over the Turso `CapMap` with
  **per-tick** synthetic→real reconciliation into a shared `IdResolver`, and the writer
  resolves each transition's (possibly synthetic) id to the store's real id (the
  `E2ESut::resolve_uri` analog). Green ~16s. Multi-tick reconciliation + id-resolution
  at scale.
- **EXP-4** `exp4_lifecycle_lazy_self_boot_with_honest_pre_start_reads` — a component
  boots its backend **lazily through `&self`** (interior-mut latch) with honest
  pre-start reads (`None`, not faked). Confirms **lifecycle model (b)**: `StartApp` can
  be a real composed transition, not forced into `init_test`.

**Findings that update the plan:**
- **Write caps ARE production operations — single-source them.** `SutBlockTreeWrite` is
  now realized **once** by a reusable `OpDispatchWriter` (`pbt/op_write_cap.rs`): a local
  newtype over `BackendEngine` dispatching `split_block`/`indent`/… through the
  production op dispatcher. Any storage component registers it — **no per-component,
  per-method forwarding**. (A blanket `impl<T: OperationProvider>` is orphan-illegal —
  both traits foreign to integration-tests, `holon-pbt-core` is thin — so the local
  newtype is the orphan-legal single-source.) **This collapses the whole
  op-dispatch-backed write family** (structural, mutation, create/delete,
  toggle-via-`cycle_task_state`) onto one writer, and single-sources the
  synthetic→real `IdResolver` (= `E2ESut::resolve_uri`/`doc_uri_map`).
- **EXP-3 settle seam is NOT needed for `block_raw` reads** — the base table is
  synchronously consistent over in-memory Turso; only *matview*-reading invariants (B4
  family) need a settle. Narrows the deferred `RegistryHost` work.
- **A real Turso production-fidelity constraint the `MemoryBackend` slice masks:** the
  production `QueryableCache<Block>` rejects a NULL `parent_id` — a top-level *text*
  block is not a valid production state (top-level = Pages). Splitting/outdenting a
  `no_parent`-rooted text block mints one; `MemoryBackend` tolerates it, Turso does not.
  Drove a genuine fix to shared `parse_block_row` (coalesce NULL/missing `parent_id` →
  `no_parent()`, which `no_orphan`/`block_parent` already treat as the valid root; every
  caller treated `None` as a hard error, so this only prevents spurious panics). The
  composed `StateMachineTest` therefore gates the top-level split and excludes `Outdent`
  (which escapes a shallow tree to `no_parent`); re-admitting `Outdent`/`Join`/`Move`
  needs a **production-faithful Page/doc-rooted seed** (test-oracle fidelity, orthogonal
  to the decomposition mechanism).

##### Cap home-rule — where each cap trait lives (decided 2026-06-20)

**A cap lives in the crate that owns the types it names** (and the functionality it
abstracts), *not* by default in `holon-pbt-core`. This dissolves the "move the param
type so `holon-pbt-core` can name it" problem (e.g. `NavDirection` is referenced in
**94 files** of `holon-frontend` — relocating it is a non-starter; instead the
`arrow_navigate` cap lives in `holon-frontend` and names `NavDirection` natively).

Three-way placement:

| Cap names… | Home | Examples |
|---|---|---|
| only `holon-api` primitives (`EntityUri`, `Region`, `CapRegion`) | `holon-pbt-core` (generic PBT machinery) | `SutBlockTreeWrite`, generic block-tree caps |
| a **domain crate's** types | **that domain crate**, behind a `pbt` feature | frontend caps (`SutFocusWrite`, `SutRenderer`, `arrow_navigate`/`NavDirection`, toggle/`CycleTarget`) → `holon-frontend`; Loro/peer caps → `holon-loro` |
| **test-only** types | `holon-integration-tests` (local cap trait) | `create_stale_loro`/`LoroCorruptionType`, `apply_mutation`/`MutationEvent` |

**Why it's acyclic / safe.** `#[capmap_adapter]` generates `impl Cap for CapMap`, so a
cap's home crate gains a (feature-gated) dep on `holon-pbt-core` + `holon-macros`.
`holon-pbt-core`'s `CapMap` is `TypeId`-erased — it never *names* a cap — so there is no
reverse edge: `holon-frontend → holon-pbt-core` (and `holon-loro → holon-pbt-core`) are
new but acyclic, and the `pbt` feature keeps the test vocabulary out of production builds.
Orphan rules hold: the cap impls (`E2ESut`, slice components) live in `holon-integration-tests`,
the trait is foreign, so `impl ForeignCap for LocalType` is legal; `capmap_adapter` on a
foreign-to-pbt-core trait is fine.

**End-state it implies:** `holon-pbt-core` slims to the generic engine
(`TransitionImpl`/`CapMap`/composition) + `EntityUri`-only caps; the existing
frontend/Loro caps currently in `capabilities.rs` **migrate home incrementally as each
cluster peels** (not a big-bang move). The `Home component` column below is the *impl*
home; this rule governs the *trait* home.

##### Parallelizable cluster-peeling plan (EXP-2 is green ✅)

Each non-navigation cluster becomes an **independent vertical slice** with the same
7-step recipe — the increment-1 (focus) and structural-PBT landings are the exemplars:

1. Define/flip the cluster's cap trait to `&self` + interior-mut, `#[capmap_adapter]`,
   **in its home crate per the cap home-rule above** (domain crate behind a `pbt` feature,
   or `holon-integration-tests`-local for test-only types — `capabilities.rs` only for
   `holon-api`-typed caps). 2. `impl` it on the owning component (reuse `HeadlessFrontendComponent`
   where possible; new `OrgFsComponent`/`PeerSyncComponent`/`WatchComponent` where not).
3. Rebind the transition(s)' `apply_to_sut` bound from `S: SutHandle` → `S: <Cap>`.
4. **Delete** the method(s) from `SutHandle` (`transition_dispatch.rs`). 5. Add `+ <Cap>`
   to the macro enum bound. 6. Extend EXP-2's composed alphabet with the cluster's
   transitions + a teeth test (clean `Ok` / planted `Fail`). 7. Selection-safety:
   update the `selects_exactly_the_full_catalog` deselection lists; tick EXP-1's union.

**Clusters (each a 🧠 ticket; ~roughly independent):**

| Cluster | Transitions | Home component | Notes |
|---|---|---|---|
| Editor / command | `toggle_state`, `trigger_slash_command`, `press_key` (split-id remap) | `InMemEditorComponent` / frontend | `press_key` is the hard one (doc_uri_map mutation + focus-handoff loop) — leans on EXP-3 |
| Block-tree mutation | `apply_mutation`, `click_block`, `drag_drop_block`, `undo`, `redo`, `bulk_external_add` | frontend/SQL component | engine write path + `wait_for_blocks_synced` (EXP-3 settle) |
| Click / geometry | `click_at_element`, `expand_toggle`, `toggle_collapse` | `GpuiWindowComponent` (E4) / frontend | expand-gate is a frontend-state flip |
| Watches | `setup_watch`, `remove_watch`, `switch_view`, `emit_mcp_data` | `HeadlessFrontendComponent` (prod watch surface — see SutWatchRows redesign) | `active_watches`/`pbt_mcp` friction is real; redesign onto prod path, don't port `ui_model` |
| Org-FS / VCS | `write_org_file`, `create_directory`, `git_init`, `jj_git_init`, `create_stale_loro`, `create_document` | new `OrgFsComponent` over the ADR-0011 FS port | `write_org_file`/`create_document` also re-key `ctx.documents` (the FileSyncController seam) |
| Peer / sync | `add_peer`, `sync_with_peer`, `merge_from_peer`, `peer_edit`, `peer_char_edit` | new `PeerSyncComponent` beside the Loro component (risk F) | needs multi-peer `LoroSut` |
| Lifecycle | `start_app`, `simulate_restart`, `concurrent_schema_init` | the booted-session component | gated on EXP-4's decision |

**Shared-file contention (the one parallelization hazard).** Every cluster edits the
**same two spots**: the `SutHandle` trait body and the macro `+ <Cap>` bound line, both in
`transition_dispatch.rs`. To keep clusters parallel: either (i) batch the trait-surgery in
short serialized PRs while the cap-impl + component + teeth work (the bulk) proceeds in
parallel branches, or (ii) front-load **all** the cap-trait *declarations* + supertrait
wiring in one prep commit (the empty-method-bodies skeleton), so each cluster afterward only
*fills in* its component impl + rebind + `SutHandle` method *deletion* (deletions don't
conflict the way additions do). Prefer (ii): it turns the keystone into a one-shot 🧠 prep,
after which the 7 clusters are genuinely contention-light 🤖/🧠 tickets.

##### #1 — the reconcile/settle seam (`ref_state` off the cap actions) — concrete design (2026-06-20)

Every cap action that still names `ref_state` uses it for one of **three** things, none of
which is intrinsic to the action — all route to the harness seam (`block_tree_post_action`
in `sut_check_invariants.rs`, run after *every* transition) which already owns `ref_state`
and already does this for `SplitBlock`/`JoinBlock`:

1. **Settle / count / verify against expected** — `expected_block_ids(ref_state)`,
   `expected_content_block_count(ref_state)`, `resolve_ref_blocks(ref_state, …)`,
   `wait_for_blocks_synced`. Used by `undo`/`redo` (✅ moved 2026-06-20), `simulate_restart`,
   `apply_mutation`, `bulk_external_add`, `start_app` (`prime_seed_count`). → a
   `block_tree_post_action` match arm per transition (the undo/redo arm is the template).
2. **Synthetic→real id reconciliation** — `map_unmapped_split_synthetic_ids`, the
   `create_document` doc-URI map-back, `start_app`'s `files.documents` boot reconciliation,
   `press_key`'s Enter→split remap. → the same seam (it already calls
   `map_unmapped_split_synthetic_ids` for `SplitBlock`); the `IdResolver` is the composed-path
   equivalent (proven in the EXP-2 spike).
3. **Config flags / boot manifest** — `is_properly_setup()`, `enable_loro()`,
   `files.documents` (which docs to seed). → carried on the transition struct (e.g.
   `StartApp { expects_valid_index, … }`) or read from the SUT's own config; not the oracle.

After this, **every cap action takes only its own concrete payload** (which the transitions
already carry — `bulk_external_add.blocks`, `apply_mutation.event`), and `ref_state` drops
from all cap signatures → the caps become `holon-pbt-core`/domain-crate-hostable. The per-tick
1:1 reconcile in the EXP-2 spike is the seed of the composed `block_tree_post_action`; the
two converge into the one `ComposedRunner`/`RegistryHost` (EXP-3) the deletion-ledger names.
**Order within #1:** do the pure settle-relocations first (undo/redo ✅, then restart,
mutation, bulk — mechanical, byte-equivalent to the undo/redo move), then the reconciliation
ones (create_document, press_key, start_app — they touch `doc_uri_map`/`IdResolver`).

##### Revised dependency order for the endgame — **NEXT STEPS**

1. ~~**EXP-1** (union probe) + **EXP-4** (lifecycle decision)~~ — **DONE.** EXP-4 green
   (model (b): `&self` lazy boot). EXP-1 **dropped as redundant**: composition (`CapMap`)
   satisfies the bound union per-cap via `#[capmap_adapter]`, not via one mega-component,
   so there is no single-type union to probe (`_assert_capmap_drives_runner` + the spikes
   cover it).
2. ~~**EXP-2 + EXP-3** (the gating spike)~~ — **DONE / green** (see RESULTS above). EXP-3's
   settle seam proved unnecessary for `block_raw` reads.
3. ~~**Promote the spike scaffolding into the framework**~~ — **DONE (2026-06-19).**
   `OpDispatchWriter` lives in `pbt/op_write_cap.rs`; `new_sql_engine_with_structural_ops`
   + a new `sql_structural_wide(engine, resolver)` builder moved into
   `sql_slice/builders.rs`; the spike re-exports them. `OpDispatchWriter` is registered on
   **both** `SqlProjectionComponent` and `HeadlessFrontendComponent` (full-DI session, so
   `SqlBlockOperations` is present). Slices green.
4. ~~**Production-faithful Page-rooted seed**~~ — **DONE (2026-06-19).** The composed
   `StateMachineTest` now seeds `page → parent → c1/c2` (focus on `parent`) on both sides
   (`page` is a seed doc via a `block_documents`/`no_parent` entry, excluded from the
   non-seed comparison), and drives the **full structural alphabet** `{SplitBlock, Indent,
   Outdent, JoinBlock}` over real Turso (48×≤12, green ~27s). **Two new fidelity findings
   (each a documented generation gate, not a SUT bug):** (i) **Outdent** of a direct
   page-child escapes to `no_parent` → gated when the target's grandparent is `no_parent`;
   (ii) **JoinBlock** of a *first child* — the oracle promotes children to the grandparent
   + deletes, but production `join_block` (no prev sibling) does not → gated to require a
   prev sibling. **`MoveUp`/`MoveDown` remain excluded** (order-drift: oracle `sequence()`
   vs SQL `sort_key` fractional index; no invariant compares child order, so a swap is
   silent until a later order-dependent op picks a different sibling on each side).
   Re-admitting Move needs an explicit order-fidelity check — the one remaining structural
   follow-on. An always-on `Nothing` arm keeps generation from dead-ending when the focused
   subtree empties.
5. ~~**Keystone prep commit (🧠)**~~ — **DONE (2026-06-20).** Cluster cap-trait declarations
   front-loaded; the 7 clusters became fill-in tickets reusing `OpDispatchWriter`.
6. ~~**Cluster peeling**~~ — **DONE (2026-06-20).** All `SutHandle` methods relocated into
   caps over the page-rooted seed + `sql_structural_wide` + per-tick reconcile harness.
7. ~~**`SutHandle` → marker bundle + blanket impl**~~ — **DONE (2026-06-20/21).** `CapMap:
   SutHandle` holds; the composed `CapMap` drives the auto-narrowed full alphabet.
8. **→ NEXT — the remaining endgame.** **★ THE SWAP landed (2026-06-21)** and the **macro
   repoint landed (2026-06-22)**: `general_e2e_composed_pbt` (`tests/general_e2e_composed_pbt.rs`)
   drives `ComposedSut<WideE2E>` over the auto-narrowed production alphabet, additive beside
   `E2ESut`. **(#4) Loro-doc-unification is RESOLVED** for `full_headless` (builder gate
   `!has_turso || frontend_sync_handle.is_some()`; peer transitions admitted). What's left to
   *delete* `E2ESut`: **E4** windowed input (`PressKey`/`ArrowNavigate`/drag) + the per-tick
   windowed loop; **E2** human verdict-parity confirm; then **E3** (delete headless cap impls)
   → **E5** (delete `E2ESut` + `Subsystem`/`min_sut`).

The point of this ordering: **E3's coverage-loss objection (the keystone framing at the top
of this track) evaporated the instant step 7 landed** — the composed `CapMap` now runs the
same full-alphabet exploration the native path does for every transition that does **not**
auto-narrow out. Steps 1–7 are done; the remaining work (step 8) is closing the auto-narrowed
families (#4 + E4) so the *full* aggregate runs composed, plus the lone structural follow-on
(Move order-fidelity check).

##### The deletion ledger — what code is REMOVED once the final state (E5) is reached

The whole point is *less* code, not parallel code. At the end state (`SutHandle` empty +
`CapMap` drives the full alphabet, E3→E5 complete) the following come out. Track this so
"done" is verifiable as net deletion, and so nothing is left as dead parallel machinery.

- **`E2ESut` itself** (`pbt/sut_handle.rs` ~1.2k lines + `pbt/sut_capabilities.rs` ~2k
  lines) — the god-type and all its cap impls. The single largest deletion; the entire
  reason for the track.
- **The `SutHandle` trait body** (`transition_dispatch.rs`) — every `&mut self` method
  collapses into fine `&self` caps; `SutHandle` becomes an empty marker supertrait bundle
  (a blanket impl, no methods). The `+ SutFocusWrite + SutNavHistoryWrite` carried bounds
  on the enum dispatch fold back into the bundle.
- **The harness reconciliation/settle seams once their composed equivalents subsume them**
  — `block_tree_post_action`, `map_unmapped_split_synthetic_ids`, `sync_caret_to_new_split_block`,
  `settle_before_invariants`/`settle_on_snapshot`, `build_doc_uri_map` (`sut_check_invariants.rs`,
  `sut.rs`, `invariant_runner.rs`). Their roles are taken by `OpDispatchWriter`'s
  `IdResolver` + the composed runner (the spike's per-tick reconcile is the seed of this);
  `with_resolved_doc_uris` stays (it is already a pure `ReferenceState` method the composed
  path reuses).
- **The native dispatch split** — `run_proxy_registry`'s `WideProxyCaps` bound + the
  `native_proxy_invariants`/`native_self_invariants` two-path split (`invariant_runner.rs`)
  collapse to one `run_selected(catalog, capmap, ref)` once no `&mut self` self-invariant
  remains (editor caret/text already on `SutEditorMirrorRead`, window-focus on `SutDriver`;
  residual `inv-sql-budget` otel is the last to move).
- **The legacy `Subsystem` / `min_sut` selection machinery** (E5) — `PbtSuiteSpec::select`,
  `ComponentSet`→`Subsystem` derivation, `min_sut` registry fields, the
  `storage_selector_for_wiring`/`native_runner_dispatches_exactly_the_registry` oracle —
  the whole parallel selection path the composed cap-presence selection subsumes.
- **`TestEnvironment` bespoke mirrors** that only `E2ESut` populated (`ui_model`,
  `active_watches` hand-drained `CdcAccumulator`s — already bypassed by the production
  watch-surface redesign for `SutWatchRows`); remove once no consumer remains.
- **NOT removed (kept):** the shared catalog (`composed/`), `ReferenceState` + its
  `Ref*`/`CapProvider` impls, the components, `parse_block_row` (now NULL-parent-robust),
  `with_resolved_doc_uris`, and the per-realization settle for matview invariants. These
  are the surviving single-sourced framework.

Verification: at E5 the diff should be **strongly net-negative**, and `rg "E2ESut|SutHandle|min_sut|WideProxyCaps"` should return nothing in `crates/holon-integration-tests/src/pbt/`.



### E0c 🧠 Eliminate the one real risk FIRST — TestPlatform geometry make-or-break
This is the **only** non-mechanical unknown in Bundle E, and it decides the plan's
*shape* (full dissolution vs. a permanent real-window residue). It is independent of
E1–E3, so spike it now, in parallel. Step-A pattern: kill the make-or-break before
investing. Two tiers, cheapest first:
- **(a) ✅ DONE (2026-06-18) — compile-only, green.** `_assert_capmap_hosts_windowed_bodies`
  in `invariant_runner.rs` (beside `_assert_capmap_hosts_proxy_bodies` /
  `_assert_capmap_drives_runner`) boxes the geometry bodies
  `InvFrontendBoundsRendered` + `InvDisplayedTextWidget` + `InvDisplayedTextViewModel`
  as `Box<dyn DynInvariant<ReferenceState, CapMap>>` — proving they monomorphise over
  a **raw `CapMap` SUT** (`S = CapMap`, the form `run_selected(catalog, &capmap, &ref)`
  runs, *not* the `CachingProxy<CapMap>` the wide-path asserts use). `cargo check -p
  holon-integration-tests --features pbt --tests` green (11.5 s). So the **hosting half
  of E4 is proven before any realization exists**: `CapMap: SutLayout + SutViewModel +
  SutRenderer` already holds via `#[capmap_adapter]`; the only thing E4 still needs is
  a realization returning non-`None` geometry (= E0c-(b)).
  - **Finding (the one real gap this surfaced):** `window_focus_*`
    (`InvWindowFocusMatchesEngineFocus`) is **excluded** — it binds `S: SutDriver +
    SutLayout`, and `SutDriver`, though now all-`&self` (Stage 1), is **not**
    `#[capmap_adapter]`-hosted, so `CapMap: SutDriver` does not hold. **Hosting
    `SutDriver` on `CapMap` is the single real E4 prereq** (add the adapter; it's
    already `&self` so no signature churn, but the macro emits `#[async_trait(?Send)]`
    so the trait + its `E2ESut` impl + the `Arc<dyn UserDriver>` forwards convert
    together — small, mechanical). Until then `window_focus` stays `E2ESut`-only.
- **(b) ✅ DONE (2026-06-18) — PASS. The thesis holds.** New test
  `frontends/gpui/tests/test_platform_geometry_determinism.rs` boots the **real**
  Holon window over `TestPlatform` (reusing `launch_holon_window_rebindable` + the
  proven cross-runtime fixed-point settle — the exact path E4 stands up), settles,
  and reads `BoundsRegistry`. **3 independent boots each produced 67 elements, 62
  non-degenerate, 7 distinct entities, with byte-identical geometry shape** (widget
  type + whole-pixel bounds), modulo per-boot fresh-vault block UUIDs. `cargo test -p
  holon-gpui --test test_platform_geometry_determinism` green in 4.2 s.
  - **Conclusion:** TestPlatform yields **real, deterministic** layout geometry with
    no on-screen window. The occlusion/blur flakiness was a *real-window* property;
    TestPlatform's fake dispatcher + fake clock are reproducible. So the "window is
    just a component, no permanent residue" thesis is **confirmed** — E4 is
    mechanical (wrap this realization in a component + the one settle seam + the
    `SutDriver` adapter). The slim-residue fallback is **not** needed.
  - **Findings for E4:** (i) determinism is on geometry *shape*, not entity identity
    — independent boots mint fresh random UUIDs, but in PBT use ids come from the
    seeded `ReferenceState` (fixed), so this is orthogonal; (ii) `visual_content_fraction`
    was **not** exercised (it reads the screenshot watcher `frontend_visual_state`, a
    separate surface not wired by the launcher) — the `rendered_elements`/`BoundsRegistry`
    path is the load-bearing geometry signal and it is proven; (iii) **the `!Send`
    constraint is confined to the settle layer, NOT the caps.** `BoundsRegistry` is
    auto-`Send+Sync+Clone` (`Arc<RwLock<…>> + Arc<Notify>`), and `E2ESut` already reads
    geometry from an injected `Box<dyn GeometryProvider>` clone **without touching the
    window** — so the `GpuiWindowComponent`'s `SutLayout` cap is an ordinary
    `async fn(&self)` over a `Send` `BoundsRegistry` clone, hosted on `CapMap` exactly
    like the Loro/SQL caps (E0c-(a) proved it monomorphises). What is `!Send`/single-
    threaded + leaked-at-teardown is the gpui `TestApp` **frame-pump**, which lives in
    the harness/settle layer — the `RegistryHost` seam the design already declared
    realization-specific (headless slices settle by awaiting a watch; the windowed one
    settles by pumping paint). The single-threaded driver loop already exists and works
    (`random_pbt_sim.rs` inline proptest, `sim_windowed_replay.rs`). So E4's genuine new
    work is narrow: wire a pump-settle into the composed runner's `check_invariants`
    path and run the windowed slice's `StateMachineTest` single-threaded on the gpui
    thread. The **cap model is unchanged** — "the window is just a component" holds.
- *Note:* the TestPlatform sim-PBT setup (commit `1eb152b`) lives in the `gpui` crate;
  the new test reuses `launch_holon_window_rebindable` + the smoke test's settle.

### E1 🧠 Relocate the orphan headless caps onto components
For each orphan above: add the `impl <Cap> for <Component>`, register it, and prove
it with a selection test (the cap's invariants now run over that realization). No
`E2ESut` deletion yet — both paths coexist. Gate: all five slices + the new
selection tests green sub-second; `storage_consistency_pbt` parity green.

**🟢 `SutOrgRender` LANDED (2026-06-18) — `inv-org-render-fixed-point` on the
composition path.** `HeadlessFrontendComponent` now captures its DI injector
(`fluxdi::Injector` is `Clone`) and provides `SutOrgRender::snapshot_org_render_pairs`
by rendering each tracked org file from the SQL state through the **production**
`CacheBlockReader` (resolved `QueryableCache<Block>` → the doc-scoped recursive CTE
ordered by `sort_key, id`) + `OrgRenderer::render_document`, paired with the on-disk
bytes — mirroring `TestContext::snapshot_org_render_pairs` over the component's own
injector/org_fs, no `TestContext`/`FileSyncController`. **Make-or-break probe first**
(`frontend_slice_org_render_pairs_reach_fixed_point`): disk == rendered headlessly ✅.
Wired `inv-org-render-fixed-point` (`org_render_fixed_point::wire`, `Needs SutOrgRender`,
**no ref**). **Teeth** (`frontend_slice_org_render_fixed_point_bites`): clean `Ok`;
overwrite the disk file → `Fail`. **KEY FIX:** the doc-block id must be cached at boot
(a disk-INDEPENDENT `documents: Vec<(doc_id, path)>` from a clean parse) — deriving it
from the corrupted disk would miss the block_raw row and vacuously skip (the first catch
attempt passed wrongly). Lib 121/2; windowed gpui green (deselects — no `SutOrgRender`).
Remaining org cap: `SutOrgFileWrite` (`&mut` apply-path, no consuming invariant).

**🟢 `SutOrgRead` LANDED (2026-06-18) — `inv-blocks-match-ref/org` on the composition
path.** `HeadlessFrontendComponent` now retains its `org_fs`/`org_root`/org-file paths
and provides `SutOrgRead::org_block_snapshot` by parsing the on-disk org files via the
**production** `holon_orgmode::parser::parse_org_file` (the same parser
`TestContext::parse_org_file_blocks` uses) — NO `TestContext`/`FileSyncController`
coupling for the read path. Wired `inv-blocks-match-ref/org` (`blocks_match::wire_org`,
`Needs SutOrgRead + RefBackend`) into the catalog. **Teeth** (`frontend_slice_org_blocks_match_ref_bites`):
org ids are random-per-boot but the booted session persists `:ID:` drawers so the parse
is deterministic — read the parsed blocks at runtime, seed the ref's `block_state` with
exactly them (`RefBackend::org_blocks` returns non-seed blocks verbatim) → clean `Ok`;
mutate one block's content on the ref → `Fail`. Scoped to `/org` (the component's
`SutBackend` sees the whole vault, so `block_raw` invariants diverge vs the 2-block
partial ref). Lib 119/2; windowed gpui green (`/org` deselects — no `SutOrgRead`).
Remaining org caps: `SutOrgRender` (FileSyncController render-vs-disk pairs) +
`SutOrgFileWrite` (`&mut` apply-path).

**🟢 `SutWatchRows` + B5 invariants LANDED (2026-06-18) — REDESIGNED onto the
production watch surface.** `SutWatchRows` was welded to E2ESut's bespoke
`TestEnvironment.ui_model`/`active_watches` HashMaps (hand-drained `CdcAccumulator`s),
which the headless component doesn't have. Rather than port that test-harness
machinery, the cap was re-pointed at the **production** reactive watch path (user
decision): `HeadlessFrontendComponent::register_query_watch` drives
`ReactiveEngine::watch_query_live` (→ real `ensure_query_watching` → real
`registry`/`watchers` + CDC pump into `ReactiveRenderedRows`), tracking the
`query_id → query:<hash>` key; `watch_rows` reads `ensure_watching(key).snapshot()`
settled to a fixed point (a query watch clears `is_loading` BEFORE its spawned CDC
pump delivers, so a single read races to empty — make-or-break found+fixed by a
probe). `block_raw_query_ids`/`block_raw_field` go straight to `BackendEngine`.
`RefWatches` got `#[capmap_adapter]` + is now registered in `reference_state_ref_caps`.
Both B5 bodies wired (`composed/invariants/watch_rows.rs` → catalog):
`inv-active-watches-match-ref` + `inv-watch-rows-match-ref` (`Needs SutWatchRows +
RefWatches`). **Teeth proven** (`frontend_slice_watch_invariants_bite_over_production_watches`):
a fixed-id `DirectChildren(parent)` watch projecting only `id` (so the row check is
id-set-only — no parent_id/content alignment) → clean both `Ok`; drop c2 from the ref →
`watch-rows` `Fail` (block_raw truth ≠ ref, a real divergence not a CDC-lag skip);
mismatched watch id → `active-watches` `Fail`. Lib 117/2 (same pre-existing reds);
windowed gpui test still green (B5 deselects — no `SutWatchRows`). Relocation, not new
coverage — `E2ESut`'s own `SutWatchRows` impl still exists (deleted in E3).

**🟢 `SutEditorMirrorWrite` LANDED (2026-06-18, Stage-1b collapse).** The standalone
`InProcEditorSut` write target is **deleted**; `InMemEditorComponent` now owns the
commit store (`Arc<dyn CoreOperations>`) and impls `SutEditorMirrorWrite` directly, so
its `CapProvider::register` hosts **both** the read mirror and the write cap on the
`CapMap` — the composed map is now an editor `SutTransitionTarget` (like
`MemoryBackendComponent` is for structural ops). `memory_slice_editor_commit_roundtrip_matches_ref`
now drives the keystroke sequence **through `caps.apply_type_chars(..)`** (the
`#[capmap_adapter]` forward), proving the hosted write cap end-to-end. Added a blanket
`impl<T: SutEditorMirrorWrite + ?Sized> SutEditorMirrorWrite for Arc<T>`
(`capabilities.rs`) so the `Arc`-shared editor (it is also the registered read cap) is
drivable through `apply_to_sut(&mut S)` — the write methods are `&self`, so forwarding
through the `Arc` is sound. Updated the spike's `build_sut`/`apply_op_to_sut` to the new
type. Lib 115/2 (same pre-existing reds); 22 memory_slice tests green. **NOTE:** this is
a relocation/consolidation, not new coverage — `E2ESut`'s own `SutEditorMirrorWrite`
impl still exists (deleted in E3).

### E2 ✅ Full/Loro parity gate — verdict-parity DONE (2026-06-23) for the composed-covered set
Run the still-un-run Full/Loro `general_e2e_pbt` **and** `gpui_ui_pbt` attended; diff
invariant *selection* against the blessed slices (known pre-existing reds — diff
against a baseline, don't read exit codes). This is the evidence that the composed
core covers what `E2ESut`'s headless impls did. **Do not delete on the strength of
the fast slices alone.**

**🟢 Selection-parity half PREPPED (2026-06-18) — static, no PBT run needed.**
The parity gate has two halves, and the *selection* half turns out to be a **pure
function** (no ~25-min run): native selection is `PbtSuiteSpec::select` over
`register_default()`; composed selection is the `Needs` of each
`composed_invariant_catalog()` entry. New fast test module
`crates/holon-integration-tests/src/pbt/composed/parity.rs` (lib suite, <1 ms) captures
the baseline and asserts the E3-readiness coverage:
- `e2_selection_baseline_report` (run `--nocapture`) prints the live three-way diff —
  the artifact the attended reviewer reads. **Native and composed share one body-id
  scheme** (the catalog bridges the literal same `pbt::invariants::bodies::*` structs via
  `BridgedInvariant`), so the diff is a raw id-set comparison, no aliasing.
- `composed_catalog_covers_e1_relocated_caps` — the concrete gate: each of the four
  E1-relocated caps' native invariant ids is present in the composed catalog
  (`SutEditorMirrorWrite`→`inv-editor-{text,caret}-matches-ref`;
  `SutWatchRows`→`inv-{watch-rows,active-watches}-match-ref`;
  `SutOrgRead`→`inv-blocks-match-ref/org`; `SutOrgRender`→`inv-org-render-fixed-point`).
  Reverting a relocation or dropping a `wire()` line fails here *before* an E3 deletion.

Baseline numbers (current tree): native `general_e2e_pbt` selects **39**, `gpui_ui_pbt`
**45**; composed catalog **21**. Against the widest (`gpui`): **19 covered**, **2
composed-only** (`inv-block-{content,parent}-matches-ref/block_raw` — finer store-variant
checks the catalog *adds*, a coverage gain not a gap), **2 native-only-but-slice-covered**
(`NATIVE_ONLY_EXCLUDED`: `inv-block-ids-match-ref`, `inv-block-tags-references-exist` — the
native *runner* never dispatches these, they're covered by targeted slices), and **24
genuinely not-yet-in-catalog** (future Bundle A–E ports — none are E1-relocated caps).

**Still requires the human (verdict-parity half):** confirm the shared bodies produce the
same `Ok`/`Fail`/`Skipped` *dispositions* over a `CapMap` as over `E2ESut`. Each body's
per-cap teeth test already exercises a clean-pass / planted-fail pair over a `CapMap`, so
the attended run is confirmation, not first evidence. Replay fast with
`PROPTEST_CASES=1 PROPTEST_MAX_SHRINK_ITERS=0` + `HOLON_PBT_LAYER_REPORT=warn` and diff the
per-tick layer table against the baseline above.

**🟢 HEADLESS verdict-parity RESULT (2026-06-23).** Ran both paths over the SAME
`full_headless` config with `HOLON_PBT_LAYER_REPORT=always`:
- `general_e2e_pbt` (`E2ESut`, full_headless), cases=8 → **PASS, 194 ticks, ZERO failures**
  (every invariant ✓ Ok or ⊘ Skipped; no ✗).
- `general_e2e_composed_pbt` (`ComposedSut`, full_headless), cases=16 → **PASS** (composed
  `run_selected` asserts no failures; it does not emit the native layer table).

Both green over the same config = **no divergence on either path**. The 21 shared "covered"
ids split into three groups for the verdict comparison:
1. **Run on BOTH (the real headless diff) — parity holds.** `inv-no-orphan-blocks`,
   `inv-no-parent-cycles`, `inv-blocks-match-ref/block_raw`, `inv-loro-{children-match-ref,no-errors}`,
   `inv-source-language-iff-source`, `inv-navigation-focus`, `inv-focus-roots`,
   `inv-viewmodel-no-error-widgets`, `inv-task-state-storage-coherence`: native ✓ across all 194
   ticks; composed green. ✅
2. **Composed-ONLY (native runner doesn't dispatch — already E3-migrated via
   `NATIVE_ONLY_EXCLUDED`), so no native counterpart to diff; covered by composed + teeth.**
   `inv-active-watches-match-ref`, `inv-watch-rows-match-ref`, `inv-block-content-matches-ref`,
   `inv-blocks-match-ref/org`, `inv-org-render-fixed-point` (these show `ok=0 skip=0` natively =
   absent from the native layer report; the composed catalog is their sole host). ✅ (consistent
   with the E1/E3 relocations already done.)
3. **Windowed — deferred to the attended `gpui_ui_pbt` run (needs a display).**
   `inv-displayed-text/{viewmodel,widget}`, `inv-frontend-bounds-rendered`,
   `inv-window-focus-matches-engine-focus`, `inv-editor-{caret,text}-matches-ref`. On the
   *headless* native run these mostly ⊘ Skip (no window/open editor). This is exactly what the
   windowed attended run covers — **the one remaining E2 step.**

   Headless coverage note (a composed *gain*, not a gap): native `inv-editor-text-matches-ref`
   was ⊘ Skipped on all 194 ticks (and `inv-editor-caret-matches-ref` ok=125/skip=69), whereas
   the composed `frontend_wide_pbt` editor arm drives `TypeChars` and bites editor-text headlessly.

**🟢 WINDOWED verdict-parity RESULT (2026-06-23) — E2 COMPLETE for the covered set.** Ran on a
real display:
- `gpui_window_slice` (composed windowed, deterministic) → **PASS (6s)**; its planted runs prove
  the composed windowed oracle **bites**: `[3a]` planted → `inv-displayed-text/{widget,viewmodel}`
  both `Fail`; `[3b]` planted → `inv-window-focus-matches-engine-focus` `Fail`. Bidirectional
  (clean→Ok, planted→Fail) over the composition path.
- `gpui_ui_pbt` (REAL window; native `E2ESut` + the **per-tick composed windowed check**,
  `invariant_runner.rs:381`), `PBT_NUM_STEPS=18` (under the ~25-step PressKey-divergence threshold)
  → **PASS, 88 ticks, ZERO failures**. The per-tick composed check ran every tick and **asserts
  non-vacuity** (`inv-frontend-bounds-rendered` must run, `invariant_runner.rs:439`) — so on all 88
  ticks the composed `CapMap` windowed family (`window_focus_wide`: `GpuiWindowComponent` +
  `GpuiFrontendEngineComponent` + driver) ran over real geometry with no failure.

Native windowed dispositions (this sequence): `inv-frontend-bounds-rendered` ✓×82,
`inv-window-focus-matches-engine-focus` ✓×82, `inv-displayed-text/widget` ✓×65 (all Ok,
non-vacuous on the real window); `inv-displayed-text/viewmodel` ⊘×71 and `inv-editor-{caret,text}`
⊘×88 Skipped (no editor opened in this 18-step sequence — sequence-dependent, not a divergence;
the composed `gpui_window_slice` proves displayed-text/viewmodel *bites*, and the headless
`frontend_wide_pbt` editor arm bites editor-text). **No divergence on either path** (0 native ✗,
0 composed failures across 88 + 194 ticks).

**✅ E2 verdict-parity is COMPLETE for the composed-covered set** (selection-parity static gate +
headless 194-tick + windowed 88-tick, both paths green, every covered invariant Ok-on-both or
proven-to-bite via teeth). **Caveat unchanged:** the **18 native-only-unported** (was 22; 4 ViewModel invariants ported 2026-06-23) ids (the
`inv-viewmodel-*` family, `inv-blocks-match-ref/{loro,matview}`, `inv-matview-consistent-with-ref`,
`inv-focus-matches-ref`) have **no composed twin**, so E2 unlocks only a **partial E3** (the
E1-relocated + covered caps). Deleting the `SutViewModel`/`SutRenderer`/`SutLoroLog` impls needs
those ported first (Bundle C-remainder) — that, not E2, is now the gating work before full E3/E5.

**E3 nuance surfaced during prep (read before deleting):** the native runner splits its
dispatch into `native_proxy_invariants` (`&self`, over `E2ESut`-as-`WideProxyCaps`) and
`native_self_invariants` (`&mut self`, over raw `E2ESut`). The editor read invariants
(`inv-editor-{caret,text}-matches-ref`) and `window_focus` run via the **`_self_`** path
natively (they need `&mut self` `SutDriver`), while the composed catalog runs the *same
body structs* via `BridgedInvariant` over a `CapMap` (`&self`). E3 deletes the relocated
*cap impls* (`SutEditorMirrorWrite` etc.), not the `SutDriver` self-invariant dispatch —
those `_self_` bodies + `SutLayout`/displayed-text stay (matches the E3 scope line below).

### E3 🧠 Delete `E2ESut`'s headless cap impls
Once E1 homes exist **and** E2 parity holds, strip the headless ~⅔ of
`sut_capabilities.rs` (everything except `SutLayout` / window-focus / displayed-text
/ the `SutDriver` native self-invariants). `E2ESut` shrinks to a windowed-only shell
that `gpui_ui_pbt` still drives. Gate: `gpui_ui_pbt` + the composed suite green.

> **⚠ Apply the convergence rule (★ North Star / Design §8.10) per cap.** A cap impl is
> blocked from deletion by whatever test consumes it over `E2ESut`. If the consumer is a
> **standalone slice/PBT** and `full_headless` already provides the cap, **DELETE the
> standalone test** (promote its invariant into `WIDE_REQUIRED_INVARIANTS` if it must be
> guaranteed-exercised; relocate any unique real-SUT teeth into `composed/invariants/<name>.rs`)
> — do **not** rewrite it as a new `ComposedSut` slice. The two deletions below
> (`SutWatchRows`/`SutOrgRead`/`SutOrgRender` "no standalone slice consumed it") are the right
> shape; the 2026-06-24 `TaskStateSlice` mint was the wrong shape and was reverted 2026-06-25.
> A composed slice is justified ONLY when `WideE2E` cannot yet drive the cap (E4/windowed).

**🟢 IN PROGRESS (2026-06-18/19) — `SutWatchRows` + `SutOrgRead` + `SutOrgRender` deleted.**
The deletion mechanic (per cap): (1) drop the cap from the `WideProxyCaps` supertrait
bound *and* its blanket impl (`invariant_runner.rs`) — `WideProxyCaps` is required by
**both** the headless and windowed `run_proxy_registry(self, …)` dispatch, so an
`impl … for E2ESut` can't be deleted alone; (2) drop the body from
`native_proxy_invariants()` + its `use`; (3) add the invariant id to
`NATIVE_ONLY_EXCLUDED` (now "composed-slice-covered") so
`native_runner_dispatches_exactly_the_registry` stays balanced; (4) delete the
`impl … for E2ESut` block + the import. Green gate met: composed lib suite **124 / 2**
(the 2 are the pre-existing `every_body_file_has_a_registry_entry` /
`now_query_compiles_to_canonical_sql` reds), all test binaries compile, windowed
`gpui_window_slice` 1/1.
- `SutWatchRows` ✅ deleted — composed `frontend_slice` is now the sole host of
  `inv-{watch-rows,active-watches}-match-ref`. No standalone slice consumed it.
- `SutOrgRead` ✅ deleted — composed `frontend_slice` is now the sole host of
  `inv-blocks-match-ref/org`. No standalone slice consumed it.
- `SutOrgRender` ✅ **deleted (2026-06-19).** The blocking standalone slice
  `tests/org_render_fixed_point_pbt.rs` (its last native consumer) was **removed this
  session**, so the composed `frontend_slice` is now the sole host of
  `inv-org-render-fixed-point`. Coverage is preserved **with teeth**:
  `frontend_slice_org_render_fixed_point_bites` drives the invariant to both arms over
  the production `CacheBlockReader` + `OrgRenderer` (clean → `Ok`, overwrite garbage →
  `Fail`), and the parity gate `composed_catalog_covers_e1_relocated_caps` asserts the
  composed catalog still covers the id. Cleanup: the now-orphaned
  `TestContext::snapshot_org_render_pairs` helper and the dead
  `tests/fixtures/org_render_fixed_point_pbt/` seed dir were removed (the wide-PBT
  regression seed `wide_pbt_seed_2026-05-19.json` was knowingly retired with it).
  **Caveat:** the deleted slice's real `#+TODO:`-header-drop scenario is no longer a
  dedicated reproducer — the synthetic teeth + composed dispatch cover the invariant,
  but not that exact disk-mutation path. Re-introduce it as a composed
  `StateMachineTest` step if the loop class recurs.
- `SutRenderer` ✅ **deleted (2026-06-23).** Once batches 1–3 ported the whole
  `inv-viewmodel-*` family + `inv-matview-consistent-with-ref` +
  `inv-editable-text-has-draggable` + `inv-displayed-text/viewmodel` into the composed
  catalog (RefRender/RefTaskState/RefGlobalFocus hosted on `ReferenceState`), the
  `SutRenderer` cap was removed from `WideProxyCaps` (supertrait + blanket impl), its 10
  bodies dropped from `native_proxy_invariants`, those ids added to `NATIVE_ONLY_EXCLUDED`
  + the `SutRenderer` `E1_RELOCATED_CAP_COVERAGE` parity entry, and `impl SutRenderer for
  E2ESut` deleted. `inv-displayed-text/widget` **stays native** (binds on `SutLayout`, not
  `SutRenderer`). The headless `widget_tree_snapshot`/`widget_tree_for` render *logic*
  survives as **inherent (non-cap) `E2ESut` helpers** used solely by the Gherkin
  `widget contains` fixture assertion (`fixtures/assert::widget_contains`), which replays
  *headlessly* and so needs a headless render that `SutLayout` (windowed geometry) cannot
  provide — re-pointing that assertion to `SutLayout` was tried and **fails the headless
  `split_block_content_pbt_gherkin_asserts`**, so the inherent-helper form was kept.
- `SutLoroLog` ✅ **deleted (2026-06-23).** Its 4 bodies (`inv-loro-no-errors`,
  `inv-loro-children-match-ref`, `inv-blocks-match-ref/loro`, `inv-live-children-match-ref`)
  are composed-hosted (`LoroBackendComponent` / full-headless Loro arm). Removed from
  `WideProxyCaps`, `native_proxy_invariants`, added to `NATIVE_ONLY_EXCLUDED` + the
  `SutLoroLog` parity entry, and `impl SutLoroLog for E2ESut` deleted. The 3 standalone
  E2ESut-backed reproducers that dispatched these over `E2ESut` — `loro_content_drop_pbt`,
  `cdc_delivery_pbt`, `org_create_ordering_pbt` — were **deleted** (composed catalog + teeth
  are now the sole host; per the `SutOrgRender` precedent, the dedicated bug-class
  reproducers are retired — re-introduce as composed `StateMachineTest` steps if a loop
  class recurs). Gate met: `general_e2e_pbt` (main) PASS, `general_e2e_composed_pbt` PASS,
  oracle + both parity gates green, lib + tests + `holon-gpui` compile clean. Pre-existing
  reds unaffected: `general_e2e_pbt_sql_only` (editor/Loro-settle race in untouched
  `SutEditorMirrorWrite`) and the `gpui_window_slice` Turso-IVM matview ghost row (composed
  `run_selected` path — off every symbol E3 touched; exposed by the batch RefRender wire).
- `SutErrorLog` ✅ **deleted (2026-06-23).** `inv-no-errors` was first ported into the
  composed catalog: `SutErrorLog` is now hosted on `HeadlessFrontendComponent` (over the
  SAME production `FrontendSession` publish-error tracker `E2ESut` read), with a new
  `composed/invariants/no_errors.rs` BridgedInvariant (Needs `SutErrorLog`, no ref) + mod
  decl + catalog wire + a `FixtureErrorLog` test double (positive/deselect/catch teeth).
  Then the cap was removed from `WideProxyCaps` (both sites), `InvNoErrors` dropped from
  `native_proxy_invariants` (+ import), `inv-no-errors` added to `NATIVE_ONLY_EXCLUDED` +
  the `SutErrorLog` `E1_RELOCATED_CAP_COVERAGE` parity entry, the `memory_slice` deselect
  list updated, and `impl SutErrorLog for E2ESut` deleted. `E2ESut::startup_error_count`
  stays (still used via `TestEnvironment`). Gate met: oracle + both parity gates +
  `memory_slice` selection + 3 bridge teeth green; lib + tests + `holon-gpui` compile
  clean; `general_e2e_pbt` (main) PASS; unported native set 4→3. `inv-no-errors` runs
  clean over composed `full_headless` (never appears in a failure). NOTE: the composed
  suite currently has a **pre-existing `inv-blocks-match-ref/matview` red** on
  `CreateDocument` (the doc/page block's `Page` tag is in `block_tags` but missing from
  the `block` matview's `tags` column; same id both sides) — PROVEN orthogonal: it still
  fails with the `inv-no-errors` wire removed. **TRIAGED 2026-06-23 (see below) — it is
  NOT a Turso-IVM bug; it is holon-engine-runtime-specific.**
- `SutSpanMetrics` ✅ **deleted (2026-06-23).** `inv-sql-budget` was first ported into the
  composed catalog. KEY difference from the other E3 caps: the native body binds
  `Invariant<ReferenceState, S>` and its cap takes a concrete `&ReferenceState`, so a
  `BridgedInvariant` (which is `Invariant<CapMap, CapMap>`) **cannot** reuse it — the
  composed ref side is a `CapMap`, not a `ReferenceState`. So the composed slice got its
  OWN ref-less machinery (`composed/span_metrics.rs`): a `ComposedBudget` read cap +
  `SutMetricsLifecycle` cap (`note_transition_start`/`freeze_for_check`) both
  `#[capmap_adapter]`'d, a `ComposedSpanMetrics` host wrapping the SAME `MetricsSut`
  `E2ESut` used, and an `InvComposedBudget: Invariant<CapMap, CapMap>` (reusing
  `InvSqlBudget::ID`). The generic `ComposedSut` harness drives the lifecycle (reset in
  `apply_transition`, freeze in `run_report`, gated on cap presence); `wide_e2e` registers
  the host. Bridge `composed/invariants/sql_budget.rs` (Needs `ComposedBudget`, no ref) +
  catalog wire + `FixtureBudget` (positive/deselect/catch teeth). Then the E3 strip:
  `InvSqlBudget` dropped from `native_self_invariants` (+ import), `inv-sql-budget` added to
  `NATIVE_ONLY_EXCLUDED` + the `SutSpanMetrics` `E1_RELOCATED_CAP_COVERAGE` parity entry, the
  `memory_slice` deselect list updated, `impl SutSpanMetrics for E2ESut` deleted, and the
  orphaned `SutSpanMetrics` trait + native `InvSqlBudget` `Invariant` impl removed (kept the
  `InvSqlBudget::ID` const + `SqlBudgetReport`, both reused by the composed path;
  `MetricsSut::sql_budget_report` retained — the composed host calls it). Gate met: oracle +
  both parity gates + `memory_slice` selection + 3 bridge teeth green; lib + tests compile
  clean; `general_e2e_pbt` (main, E2ESut) PASS; `general_e2e_composed_pbt` PASS on a fresh
  seed (matview-pinned seed bypassed) with `inv-sql-budget` in `WIDE_REQUIRED_INVARIANTS`
  (non-vacuity proof it runs each tick over the production full-headless CapMap). Unported
  native set 3→**2**.
- `SutBackend` ✅ **deleted (2026-06-24).** The cleanest remaining deletion (all 6 bound
  bodies composed-covered, ZERO standalone-test blockers, no windowed coupling — confirmed
  by a fresh readiness audit). Removed `SutBackend` from `WideProxyCaps` (supertrait +
  blanket impl), dropped its 6 bodies from `native_proxy_invariants` (`inv-blocks-match-ref/matview`,
  `inv-blocks-match-ref/block_raw`, `inv-no-orphan-blocks`, `inv-no-parent-cycles`,
  `inv-source-language-iff-source`, `inv-focus-roots` — matview + focus-roots are co-bound
  with `SutSqlProjection` but both composed-covered, so they leave native cleanly), added the
  6 ids to `NATIVE_ONLY_EXCLUDED` + a `SutBackend` row to `E1_RELOCATED_CAP_COVERAGE`, and
  deleted `impl SutBackend for E2ESut`. Verified `SutBackend ∉ SutHandle` first (transition
  apply-path unaffected). **Orphan cleanup (refactor-completely):** the deletion orphaned the
  entire `CdcMirrors` module (the CDC-driven `LiveData<Block>`/`LiveData<FocusRoot>` mirrors
  that backed `live_block_snapshot` + the `live_blocks_cdc_stale` CDC-lag classifier) — it was
  provably inert (mirrors never built; no invariant reads `cdc_in_flight`). Deleted
  `sut_cdc_mirrors.rs` + its `mod`, the `cdc` field, `live_blocks`/`live_focus_roots`/
  `live_blocks_cdc_stale`/`wait_for_live_data_mirrors`, and `FocusRoot`. `SutCdc::cdc_in_flight`
  is kept (E2ESut-only cap surface, consistent with how prior E3 deletions leave `CachingProxy`
  cap forwarding) but now an honest documented `false` (no live mirror left to track; settle is
  via `drain_cdc_events` + Loro/SQL quiescence). Gates met: coverage oracle
  (`native_runner_dispatches_exactly_the_registry`) + parity (`composed_catalog_covers_e1_relocated_caps`)
  + `general_e2e_pbt` (E2ESut, cleaned apply-path) + `general_e2e_composed_pbt` (sole host now)
  all green; compiles clean (no new dead-code warnings). `WideProxyCaps` is now
  `SutSqlProjection + SutViewModel + SutLayout`.
- `SutCdc` ✅ **deleted ENTIRELY (2026-06-24).** Follow-up to the `SutBackend` deletion above,
  which gutted the cap's last consumer (the `live_blocks_cdc_stale` classifier) and reduced
  `cdc_in_flight` to an honest `false`. Post-`SutBackend`, an audit confirmed `SutCdc` was
  **fully vestigial**: `cdc_in_flight` had ZERO callers (its memoised proxy wrapper
  `cdc_in_flight_cached` was uncalled), `drain_cdc` had ZERO callers (only a module-doc example),
  the trait was absent from `WideProxyCaps`, `SutHandle`, the composed `CapMap`
  (no `#[capmap_adapter]`), and every invariant body. So — unlike `SutBackend` where the cap
  method was kept because the composed `CapMap` still hosts it — here the **whole trait** was
  deleted (refactor-completely, no dead "just in case" surface): `trait SutCdc` (capabilities.rs),
  the `impl<S: SutCdc> CachingProxy` block + `cdc_in_flight_cache` field/inits + the `drain_cdc`
  module-doc section (caching_proxy.rs), and `impl SutCdc for E2ESut` (sut_capabilities.rs).
  E2ESut's own settle path is unaffected — it calls `self.ctx.drain_cdc_events()` directly in
  `apply_transition_async` (that's a `TestContext` method, never the trait). Gates: coverage
  oracle + parity + `full_headless_cap_set_admits_peer_transitions` (0.7s) +
  `general_e2e_pbt` (51s) + `general_e2e_composed_pbt` (16 cases + pinned seed, 293s) all GREEN;
  `holon-pbt-core` + `holon-integration-tests --tests` compile clean (no new warnings).
- `SutQueryCompile` + `SutOrgFileWrite` + `SutLifecycle` ✅ **deleted ENTIRELY (2026-06-24, batch).**
  Three more fully-vestigial traits, found by a candidate audit and removed whole (trait +
  E2ESut impl), same refactor-completely shape as `SutCdc`. **`SutQueryCompile`** — the E2ESut
  impl was `unimplemented!()`, never wired; no transition/generator/invariant bound it.
  **`SutOrgFileWrite`** — a redundant wrapper that delegated straight to
  `local_caps::SutFixtureFs::write_org_file`, which is what the `WriteOrgFile` transition binds
  directly; zero callers of the coarse trait. **`SutLifecycle`** — coarse `&mut self`
  start/restart, superseded by the finer `local_caps::SutAppLifecycle` (what `StartApp`/
  `SimulateRestart` actually bind); zero method callers (removing it also orphaned the `HashSet`
  import). None were in `WideProxyCaps`/`SutHandle`/`CapMap`/any invariant, so NO
  `NATIVE_ONLY_EXCLUDED`/parity/registry change. Gates: coverage oracle + parity +
  `general_e2e_pbt` (59s) + `general_e2e_composed_pbt` (16 cases) GREEN; both crates compile clean.
- ⚠️ **`SutLoroTaskState` is NOT a cheap deletion — audit-corrected (2026-06-24).** A readiness
  audit initially ranked it "composed-covered, safe" (its invariant `inv-task-state-storage-coherence`
  is in `NATIVE_ONLY_EXCLUDED` with a composed twin). **That was WRONG about the test dispatch:**
  the standalone slice `tests/task_state_coherence_pbt.rs` runs `InvTaskStateStorageCoherence`
  over `E2ESut` via `component_pbt!` — and **`component_pbt!`/`declare_pbt_slice!` ALWAYS wrap
  `E2ESut`** (`__declare_pbt_slice_wrapper!` hardcodes `inner: E2ESut`; the `set:`/`wiring:`
  argument only picks the *storage backend*, NOT whether the SUT is `E2ESut` vs. a composed
  `CapMap`). So deleting `impl SutLoroTaskState for E2ESut` broke `task_state_coherence_pbt`
  compilation → reverted. **GENERAL LESSON for the remaining caps: a `component_pbt!`/
  `declare_pbt_slice!` slice that lists a cap's invariant is a live `E2ESut` consumer of that
  cap, regardless of its `ComponentSet`.** `SutLoroTaskState` thus needs the SAME composed-slice
  migration as `SutSqlProjection` (its blocker is `task_state_coherence_pbt`).
- **Not yet attempted (the remaining headless caps):** `SutSqlProjection` / `SutViewModel`
  stay in `WideProxyCaps`. `SutSqlProjection` (MEDIUM, **but costlier than the 2026-06-23 audit
  implied — the "swap to backend variant" escape is now STALE**): all 3 native ids composed-covered,
  but 2 standalone blockers (`split_block_content_pbt` + `peer_conflict_pbt`) dispatch the
  `SutSqlProjection`-bound `InvBlockContentMatchesRef` **directly over `E2ESut`** (the
  `component_pbt!` / `declare_pbt_slice!` wrapper hardcodes `inner: E2ESut`, and its
  `check_invariants` runs the listed invariant against that `&E2ESut`). The audit's fallback
  "swap to the `SutBackend` `block_raw` variant `InvBlockContentMatchesRefBackend`" **no longer
  works** — `SutBackend` was deleted off `E2ESut` (above), so the backend-variant invariant
  can't dispatch over `E2ESut` either. The only real repoint is the **composed path**: migrate
  both slices to a `ComposedSlice` impl over `compose_sut(full_headless())` (the composed
  `full_headless` cap set DOES admit both `SplitBlock` and the peer transitions — proven by
  `full_headless_cap_set_admits_peer_transitions` — and its default `run_report` runs the full
  catalog incl. `block_content_sql::wire()`, i.e. `inv-block-content-matches-ref` for free). The
  catch: these slices also carry **deterministic gherkin/JSON fixture replays** (real past
  regressions: the May-2026 SplitBlock content-routing bug; the peer-merge tie-break) via
  `run_feature_strict::<Machine, Sut>` / `run_fixtures`, and `FixtureAssertable` is impl'd **only
  for `E2ESut`** — so a faithful migration must also `impl FixtureAssertable for ComposedSut<S>`
  (or the fixtures get dropped, losing the regressions). That is a deliberate ~150-250-line
  migration, NOT a one-line repoint — schedule it as its own increment. `SutViewModel` (WORST):
  9 bound bodies, and `inv-frontend-no-error-widgets` is neither composed-covered nor excludable
  (windowed: `SutViewModel + SutLayout`) — needs a net-new windowed composed
  `frontend_no_error_widgets::wire()` (like `frontend_bounds_rendered::wire()`) before the cap
  can come off; second windowed dep `inv-frontend-bounds-rendered` ties it to the windowed slice.
  `inv-focus-matches-ref` (`SutDriver`, windowed + E5-coupled) + `SutEditorMirrorWrite` + the
  editor `_self_` invariants are **E5** (the transition apply-path still drives edits through
  `E2ESut`).

- **✅ `inv-blocks-match-ref/matview` Page-tag red — RESOLVED 2026-06-23. It was a TEST-HARNESS
  READER BUG, NOT Turso and NOT the holon engine** (both prior conclusions were wrong).
  Symptom: after `CreateDocument`, the minted doc block's `Page` tag appeared missing from the
  matview side. Captured the failing tooth's exact SQL (`RUST_LOG=holon_turso=trace
  HOLON_TRACE_SQL=1 … --nocapture` — debug builds emit full SQL only via `tracing::trace!`, not
  the release-only eprintln mirror, which is why an earlier `HOLON_TRACE_SQL`-only run saw 1
  line) and replayed the EXACT sequence standalone **7 ways** (orphan right-side insert,
  DELETE-then-INSERT edge pattern, the double `block_raw` write where the txn UPSERT is a
  GROUP-BY-key-changing UPDATE of sort_key/updated_at/created_at, the chained matviews
  `block_requirement_edges`/`block_with_path`/`watch_view` on `block`, file-backed, AND through
  the real `TursoBackend` actor+CDC+transaction wrapper) — **all GREEN**. Turso's IVM and
  holon's wrapper were correct all along; the `block` matview held `['Page']`.
  - **Actual root cause:** `SutBackend::live_block_snapshot` (the `…/matview` invariant's reader)
    delegated to `HeadlessFrontendComponent::all_blocks()`, which queried `SELECT … FROM
    block_raw` — the base table has **no `tags` column**, so `parse_block_row`'s `row.get("tags")`
    was `None` → `tags = []`. The prior "matview tags=[]" finding was the imprecise probe reading
    `block_raw`, not the `block` matview.
  - **Fix:** `live_block_snapshot` now reads the `block` matview (cols incl. `tags`/`requires`);
    `block_raw_snapshot` still reads `block_raw`. Centralised the two snapshot SQLs + a
    `parse_block_rows` helper in `sut_row_parsing.rs` (removed the duplicated `SELECT … FROM
    block_raw` from `frontend_slice/components.rs`, `sut_capabilities.rs`, `sql_slice/components.rs`).
    Verified: `wide_create_document_lockstep` tooth, the `general_e2e_composed_pbt` pinned
    regression seed (was the deterministic red), the native gate, and holon-turso 81/81 all green.
  - **Lesson:** verify what a SUT reader actually QUERIES before blaming the engine — an invariant
    named `…/matview` was reading the base table.

- **✅ `inv-viewmodel-state-toggle-correct` red (`wide_frontend_toggle_state`) — RESOLVED 2026-06-23.
  A STALE INVARIANT, not a render gap.** The check did `find_op("set_field:task_state:")`, but the
  bound ops are the typed `set_state:task_state:…` / `cycle_task_state:task_state:…` (the only
  `set_field` op is the generic `set_field::id,field,value` — empty affected-fields, `field` is a
  runtime param). An empirical diagnostic (dump the resolved widget tree) refuted a first
  "ctx.operations empty" hypothesis — the op list was rich. E2ESut "passed" only VACUOUSLY (its
  `widget_tree_snapshot` uses `interpret_pure`, leaving nested live_blocks as placeholders → no
  task-block toggle resolved); the headless `reactive.snapshot` resolves the full tree and exposed
  the stale check. Fix: the invariant now matches an op whose affected-fields segment contains
  `task_state` (render-faithful), NO prod change. All 22 frontend_slice teeth + `general_e2e_pbt` +
  composed aggregate green.

- **Post-E2 deletion audit (2026-06-23, SUPERSEDED 2026-06-24).** The 2026-06-23 audit
  concluded "NOTHING new is safely deletable" because the bulk proxy caps still fed
  native-only-unported invariants. That was overtaken once the `inv-viewmodel-*` family +
  `inv-no-errors` + `inv-sql-budget` were ported into the composed catalog (the SutRenderer /
  SutLoroLog / SutErrorLog / SutSpanMetrics deletions, then **SutBackend** on 2026-06-24).
  Two durable lessons from it still hold:
  - The `SutHandle` marker-bundle supertraits (`SutLoro`, `SutBlockTreeWrite`, `SutEditorMirror*`,
    `SutFocusWrite`, `SutDriver`, …) are required for `E2ESut: SutHandle`, i.e. the native
    `general_e2e_pbt` transition-apply path → those caps are **E5**, not E3. (Verified
    `SutBackend ∉ SutHandle` before deleting it.)
  - **Even seemingly-dead caps aren't** — check the invariant's *consumers over `E2ESut`*
    (`slice.rs` + standalone `tests/*_pbt.rs`), not just the cap name + general_e2e registry
    (`SutLoroTaskState` looked dead but `task_state_coherence_pbt` drove it). The per-cap
    deletion-readiness method is now: (1) which native bodies bind the cap, (2) are all those ids
    in the composed catalog, (3) which `tests/*_pbt.rs` dispatch them over `E2ESut`. `SutBackend`
    passed all three cleanly; `SutSqlProjection`/`SutViewModel` do not yet (see above).

### E4 🧠 Build `GpuiWindowComponent` — the last component
**🟢 First vertical slice LANDED (2026-06-18).** `GpuiWindowComponent`
(`pbt/window_slice/components.rs`) holds only a `Box<dyn GeometryProvider>` (a `Send`
`BoundsRegistry` clone) and provides `SutLayout` (reusing `E2ESut`'s exact
`rendered_elements` conversion; `visual_content_fraction` honest-`None`; `wait_*`
single-shot against the settled frame). `window_slice/builders.rs::window_layout`
builds the `CapMap`. Test `frontends/gpui/tests/gpui_window_slice.rs` boots a real
TestPlatform window, settles, and reads `SutLayout::rendered_elements` **through the
`CapMap`** (the `#[capmap_adapter]` forward): **67 real elements, 62 non-degenerate,
identical to the raw `BoundsRegistry`** — green 2.9 s. So a composed `CapMap` hosts the
windowed cap and realizes real geometry through the composition path (not via
`E2ESut`). The `!Send` `TestApp`+pump stay in the harness; the component is plain
`Send`, hosted like any other — the design holds in code.

**🟢 Increment 2 LANDED (2026-06-18).** The windowed **registry** invariants now run
over real geometry via `run_selected` — the first time `inv-frontend-bounds-rendered`
executes on the composition path. What landed:
- `RefLayout` got `#[capmap_adapter]` (`capabilities.rs`) and is now registered in
  `reference_state_ref_caps` (`reference_capabilities.rs`), so `CapMap: RefLayout`
  holds and the ref oracle carries the document/focus metadata the bounds invariant
  reads. (Harmless to existing slices — selection ANDs the SUT and ref cap sets.)
- `GpuiFrontendEngineComponent` (`window_slice/components.rs`) provides `SutViewModel`
  (real `frontend_root_vm` + `headless_error_node_count`) **and** `SutRenderer`
  (`widget_tree_snapshot` / `root_render_ready` / … via `interpret_pure` with the
  engine as `BuilderServices`) over the **same** frontend `ReactiveEngine` the window
  paints from — so geometry and the VM it's compared against come from one pipeline.
  `window_slice/builders.rs::window_wide(geometry, engine)` composes it with
  `GpuiWindowComponent`; `window_ref_caps()` builds the minimal oracle.
- Three catalog wires added (`composed/invariants/frontend_bounds_rendered.rs` +
  `displayed_text.rs` → `catalog.rs`): `inv-frontend-bounds-rendered`
  (`SutLayout + SutViewModel`, ref `RefLayout`), `inv-displayed-text/widget`
  (`SutLayout`), `inv-displayed-text/viewmodel` (`SutRenderer`).
- The extended `gpui_window_slice.rs` test boots one TestPlatform window, settles,
  and `run_selected` selects + runs **4** registry invariants (the three above +
  `inv-viewmodel-no-error-widgets`) over the real geometry; the storage/editor
  invariants are correctly **deselected** (no `SutBackend`/editor caps).
  `inv-frontend-bounds-rendered` reaches a verdict of **`Ok`** (asserted — *not*
  `Skipped`), so its strict geometry checks (expected-size, no-error-widgets, VM
  y-order/contiguity) genuinely ran over the window. Green 2.9 s; composed lib suite
  green (the memory slice's exact-deselection assertion was updated to list the three
  new windowed invariants as correctly deselected).
  - *Oracle note:* `window_ref_caps()` is a minimal `fresh_reference_state`, so it
    knows none of the booted vault's random-UUID blocks — `inv-displayed-text` skips
    them (unknown block ⇒ skip, not fail) and `bounds-rendered`'s document-gated
    content checks are disclosed-off. The **vault-matching oracle** that makes the
    text checks bite is increment 3's `StateMachineTest` concern (seed drives the
    window), not this one's. This increment proves the **path** (selection + dispatch
    + real geometry to `Ok`).
- **Remaining E4 increments:** (3) the `StateMachineTest` windowed driver loop
  (single-threaded on the gpui thread, rebind per tick — reuse `sim_windowed_replay`;
  seeds a `ReferenceState` the window boots from, so the text/oracle checks become
  load-bearing); (4) host `SutDriver` on `CapMap` → `window_focus` (see below).

**🟢 Increment 4 + 3b (window_focus) LANDED (2026-06-18).** `SutDriver` is now
`#[capmap_adapter]`-hosted, and the windowed `inv-window-focus-matches-engine-focus`
**runs and bites on the composition path** — the last windowed cap-host gap closed.
- **Inc4 (mechanical):** `#[holon_macros::capmap_adapter]` on `SutDriver`
  (`capabilities.rs:1156`; first *mixed* adapter — 7 async + 1 sync `resolve_ref_block_id`,
  the macro forwards async via `expect`, sync via `expect_ref`); `impl SutDriver for E2ESut`
  gained `#[async_trait(?Send)]`; new `composed/invariants/window_focus.rs` wire
  (`Needs SutDriver + SutLayout`, **no ref** — compares the SUT against itself) appended to
  `catalog.rs`; `_assert_capmap_hosts_windowed_bodies` (`invariant_runner.rs:516`) now includes
  `InvWindowFocusMatchesEngineFocus` (the compile-time proof `CapMap: SutDriver` holds); memory
  slice's exact-deselection list updated. Lib 115/2 (same pre-existing reds), compiles.
- **Inc3b (`GpuiDriverComponent` + driven focus tick):** new `GpuiDriverComponent`
  (`window_slice/components.rs`) holds the **window's own** `ReactiveEngine` and answers
  `engine_focused_block` = `engine.focused_block()` (the V6 one-liner — E2ESut's
  `engine_focused_block` already did this; the matview is `driver_current_focus`). Its
  drive methods are honest `unimplemented!` — transitions apply through the concrete
  `UserDriver`, never `SutDriver` (H7). Builders `window_focus_wide` +
  `window_focus_wide_planted(forced_focus)`. The windowed test (`gpui_window_slice.rs`,
  still ONE `#[test]`) now: boots+grafts (3a), then **drives a real click on `block:c1`**
  via a `SimUserDriver` (added a `pub(crate) fn new`; included `pbt_harness` via `#[path]`),
  settles, and proves c1's `editable_text` is window-focused. Then `run_selected`:
  clean (`window_focus_wide`, live engine) → engine==window==c1 → **Ok**; planted
  (`window_focus_wide_planted`, engine FORCED to c2) → engine c2 vs window c1 → **Fail**.
  Non-vacuous clean/planted pair, the focus-axis analogue of 3a. Green, stable across 3 runs.
- **KEY finding:** on boot **no `editable_text` mounts and nothing is focused** (`focused_block()`
  = `None`), so `window_focus` would Skip ("both authorities unfocused") — a real click to enter
  edit mode is REQUIRED for the Ok arm (no auto-focus). The planted Fail is injected on the
  **SUT side** (a `GpuiDriverComponent` that misreports engine focus), since `window_focus` has
  no ref to plant. V9 is satisfied by construction: the composed `CapMap` forwards
  `rendered_elements_fresh` straight to the live `BoundsRegistry` — no `CachingProxy` memoization
  on the composition path.
- **Scope honesty:** this is a *single deterministic focus tick* through `run_selected`, not yet
  the full `random_pbt_sim` proptest loop running `run_selected` per generated transition. It
  proves the cap is reachable + bites on the composition path (the increment's purpose); wiring
  `run_selected` into every generated tick of the windowed `StateMachineTest` (a per-tick hook in
  `replay_fixture_with_driver_sync_callback`, or a parallel windowed loop) is the remaining
  follow-on before E5.

**🟢 Increment 3a LANDED (2026-06-18).** The windowed `inv-displayed-text` oracle now
**bites** — the first windowed *content* comparison on the composition path, proven
non-vacuous by a clean-pass/planted-fail pair.

- **Seeding = A (direct backend seed, fixed shared ids).** Resolved fork (i) by
  extracting the fixed-id primitives (`fixed_ids`/`Ids`/`PARENT`/`C1`/`C2`/`Plant`/
  `seed_ref_tree`/`apply_plant`) out of the `#[cfg(test)]` `subsystem_seed` module
  into a new `composed/seed_primitives.rs` gated `#[cfg(any(test, feature = "pbt"))]`.
  `subsystem_seed` re-imports them (spike unchanged); the windowed slice consumes them
  in the `pbt` build. No spike un-gating, no dead-code cascade — the clean form of A1
  (reuse, not duplicate).
- **Render target = B1 (graft under the Main focus root).** `focus_roots.root_id` for
  region `main` is `block:journals`; `window_slice::seed::graft_displayed_text_tree`
  creates `parent`/`c1`/`c2` (fixed ids + content) under it via the window's
  `BackendEngine` (`create_block` honors the id ⇒ identity mapping). After a re-settle
  the geometry grows 67 → 100 elements and **renders the grafted blocks**
  (`block:c1`="c1", `block:parent`="parent", `block:c2`="c2") — asserted on-screen so
  the pass is non-vacuous.
- **The oracle = BOTH `inv-displayed-text` arms bite.** `window_ref_caps_seeded()`
  seeds the ref with the same fixed tree → seeded `/widget` (geometry) AND `/viewmodel`
  (ViewModel tree) both reach **Ok** (compare the grafted blocks; unknown vault blocks
  skipped); `window_ref_caps_planted()` (a `Plant::Content` `c1`→`c1-WRONG` divergence)
  makes **both** Fail. That pair, at both layers, is the proof.
- **`/viewmodel` needs the RECURSIVE resolver, not `interpret_pure` — KEY fix
  (probe-verified).** The component's `widget_tree_snapshot` originally used
  `interpret_pure(root_layout_expr, root_layout_data_rows, services)`. The root layout's
  `render_expr` is `render_entity` → a layout **shell** of three `live_block` region
  nodes (left/main/right). The interpreter expands a `live_block` by calling
  `services.get_block_data(child_id)` (render_interpreter.rs:513) — but for the
  windowed `services()` (raw engine) that was a *cold one-level* read (the nested
  Main-panel watch wasn't re-driven inside the synchronous interpret), so the VM tree
  came back as 9 shell nodes with **zero content text** → `/viewmodel` Skipped (it
  Skipped in increment 2 too; 3a's `== Ok` demand surfaced it). **Fix:** build the
  snapshot from `engine.snapshot(root_layout_uri)` — the engine's recursive,
  cycle-detected resolver (the same one `frontend_root_vm` uses) — which descends
  through every `live_block` via the live window's already-warm watches. The VM tree
  now contains the grafted content and `/viewmodel` compares it.
- **The headless `frontend_slice` `/viewmodel` ALSO bites now — NO architectural
  deviation.** It is tempting to think the deep tree is "frontend-driven" (it is not).
  The shared `shadow_builders` (`view_mode_switcher`/`tree`/`render_entity` live in
  `holon-frontend/src/shadow_builders/`, not gpui) produce the whole tree headlessly;
  the ViewModel layer is rich, the frontend thin. `engine.snapshot(root)` recursively
  `ensure_watching`s and resolves, but stops at the first still-loading child, so it
  warms only **one level per call**. `HeadlessFrontendComponent::widget_tree_snapshot`
  now **re-snapshots after a CDC settle until the resolved tree reaches a fixed point**
  (`(total, pending)` stable for 4 iters; `pending` = `loading`/`unknown` nodes) — the
  headless analogue of the windowed pump-settle (observed: tree 1→9→9→**59** nodes,
  content resolved by ~iter 3 ≈ 450 ms). New test
  `frontend_slice_displayed_text_viewmodel_bites_on_nested_content`: graft fixed
  `parent`/`c1`/`c2` under `block:journals`, seeded ref → `/viewmodel` Ok, planted → Fail
  (scoped to the `/viewmodel` arm — the component also provides `SutBackend`, so the
  block-tree-vs-ref invariants select against the partial ref and legitimately diverge;
  `run_with_seeded_ref` drops the `ReferenceState`'s tokio runtime off-thread). So **both**
  the windowed slice (warm window watches) and the surviving headless slice (warm-loop)
  carry the deep `/viewmodel` content oracle. (`E2ESut`'s own `widget_tree_snapshot` still
  uses `HeadlessBuilderServices` → shallow; it could adopt the same warm-loop +
  `engine.snapshot`, but `E2ESut` is being retired so it is not worth changing.)

**🟢 Per-tick composed loop LANDED (2026-06-22) — the windowed analogue of
`general_e2e_composed_pbt`.** The windowed proptest loop (`gpui_ui_pbt` xcap +
`gpui_ui_pbt_sim` TestPlatform) now drives the windowed invariants on the COMPOSITION
path **per generated tick**, not only via `E2ESut`'s native registry. What landed:
- **A SHARED per-tick hook** in `E2ESut::run_invariant_registry_gated`
  (`invariant_runner.rs`), gated on `has_window` (`frontend_geometry.is_some()`):
  after the native report it calls `run_windowed_composed_check(&resolved)`. BOTH the
  xcap real-window and the TestPlatform-sim harnesses reach it via
  `replay_steps::<_, E2ESut>`, so the composed check is **single-sourced and shared**
  (the user's "share as much as possible between xcap and TestComponent"). Headless
  `general_e2e_pbt` (no geometry) skips it — unaffected.
- `run_windowed_composed_check` builds `window_focus_wide(geometry.clone_box(), engine)`
  over the SUT's installed live geometry+engine, the ref via
  `reference_state_ref_caps(Resolved::identity(resolved.get().clone()).map(Arc::new))`,
  and runs `run_selected(composed_invariant_catalog(), &sut, &ref)`. Asserts no failures
  **and** non-vacuity (`inv-frontend-bounds-rendered` ran). Selection picks the windowed
  family (bounds-rendered, displayed-text/*, window-focus); block/storage deselect.
- New `GeometryProvider::clone_box` (the stored geometry is a non-`Clone`
  `Box<dyn GeometryProvider>`); impl'd on all three providers
  (`SharedBoundsRegistry` / gpui `BoundsRegistry` / tui `TuiGeometry`, all Arc-backed
  `Clone` so the clone shares the live registry).
- **Verified:** `gpui_ui_pbt_sim` (loro) + `gpui_ui_pbt_sim_no_loro` (sql_only) green
  @ `PBT_NUM_STEPS=8 PROPTEST_CASES=1` with the composed check running per-tick,
  non-vacuous; `gpui_window_slice` single-tick still green; `cargo check -p holon-gpui
  -p holon-tui --tests` clean (xcap inherits the hook; running it needs a display).
- **Orthogonal pre-existing finding (NOT this increment):** `gpui_ui_pbt_sim` @ 25 steps
  panics in the **native** `report_findings` (`invariant_runner.rs:914`) on
  `inv-blocks-match-ref/{loro,block_raw,matview}` + `inv-displayed-text/widget` after a
  `PressKey` ("trouble begins at: Loro"). That is the untouched `E2ESut` windowed path
  finding a real `PressKey`→Loro/blocks divergence at depth; the composed check runs
  strictly after the native report so it never fired on that tick. It gates the windowed
  PBT being green at default depth (50 steps), a separate investigation, NOT E4.

**Earlier scoping (still valid).** Contrary to an even-earlier draft, **no
`&self` flip is needed** — the windowed caps are already `&self`/object-safe and
proxy-hosted, E0c-(a) *proved* the geometry bodies host over a raw `CapMap`, and
E0c-(b) *proved* `TestPlatform` yields real deterministic geometry. The real work is:
(a) productionise the E0c-(b) window-boot-and-settle: the component's `SutLayout` cap
is an ordinary `Send` `async fn(&self)` over a `BoundsRegistry` clone (same as
`E2ESut` reads today), hosted on `CapMap` normally; (b) the **one shared
`RegistryHost`/single-threaded settle** seam — the `!Send` gpui `TestApp` frame-pump
lives in the *harness/settle* layer, not the caps, and the single-threaded driver loop
already exists (`random_pbt_sim.rs` / `sim_windowed_replay.rs`); the new glue is just
wiring a pump-settle into `check_invariants`;
(c) **host `SutDriver` on `CapMap`** (add `#[capmap_adapter]`; already `&self`, so
mechanical — the trait + `E2ESut` impl + `Arc<dyn UserDriver>` forwards convert to
`#[async_trait(?Send)]` together) so `window_focus` (binds `S: SutDriver`) joins the
geometry bodies on the composed path — the one gap E0c-(a) surfaced. Back it with
`TestPlatform` (not a real window) per §8.7,
down-weighted in generation. `gpui_ui_pbt` becomes **the full config composed slice**
(all components + `GpuiWindowComponent`). Gate: windowed invariants
(`frontend_bounds_rendered`, `displayed_text`, window-focus) run through the composed
runner, green and non-flaky on `TestPlatform`.

#### A2b RESOLVED (2026-06-20) — re-level `SutBlockInteract` from input-mechanism → intent caps

**Decision:** the four block interactions (select / move / expand+collapse /
trigger_command) re-level from input-mechanism verbs (`click_block` / `drag_drop_block` /
`expand_toggle`+`collapse_toggle` / `trigger_slash_command`) to **intent** verbs, with the
**headless realization dispatching the resolved `OperationIntent` by `block_id`** — no
`UserDriver`, no geometry, no `BoundsRegistry`. `press_key` and `click_at_element` stay
geometry/keymap-realized (E4 windowed residue). Verified against the production intent
layer (ADR 0010, "ViewModels carry the intent, drivers dispatch it").

**The headless dispatch spine (verified, all in `holon-frontend`):**
- `find_click_intent_in_region(root: &ViewModel, entity_id: &EntityUri, region: &str) ->
  Option<OperationIntent>` — `crates/holon-frontend/src/focus_path.rs:235`. **`pub`**,
  reachable from `holon-integration-tests`. **Pure function over a resolved `ViewModel` +
  `entity_id` + region string — NO geometry/coordinates/`BoundsRegistry`.** It resolves
  the region name to a static panel id (`find_region_panel`, focus_path.rs:204:
  left_sidebar→`block:default-left-sidebar`, main→`block:default-main-panel`,
  right_sidebar→`block:default-right-sidebar`), then walks that subtree via
  `find_click_intent_in_view_model` (focus_path.rs:161) reading
  `OperationWiring::descriptor.is_click_triggered()`. Siblings: `find_click_intent_oneshot`
  (focus_path.rs:131, `&ReactiveViewModel`), `region_contains_entity` (focus_path.rs:253).
  Returns `crate::operations::OperationIntent` (`entity_name`, `op_name`, `params`).
- `apply_intent` is defined **only** on the `UserDriver` trait
  (`user_driver.rs:107`) as a thin wrapper around `synthetic_dispatch`. **But it is not
  needed headlessly** — its body bottoms out in the engine/session path that
  `HeadlessFrontendComponent` already calls. Trace:
  `ReactiveEngineDriver::synthetic_dispatch` (user_driver.rs:444) →
  `engine.dispatch_intent_sync(intent)` → **`session.execute_operation(entity, op, params)`**
  (`reactive.rs:2009-2046`, dispatch bottoms out at reactive.rs:2034). This is the SAME
  `FrontendSession::execute_operation` (`lib.rs:595`) that the component already calls for
  `navigation.focus` / `navigation.go_home`
  (`pbt/frontend_slice/components.rs:775,814,1075`). So a headless realization extracts the
  intent and calls `self.session.execute_operation(&intent.entity_name, &intent.op_name,
  intent.params)` directly (plus the structural-focus mirror that
  `dispatch_intent_sync` applies, reactive.rs:2027-2043, if a focus barrier is wanted).

**Per-interaction reachability verdicts (by `block_id`, windowless):**

| Interaction | Headless? | Call chain |
|---|---|---|
| (a) **select / focus** (was `click_block`) | **YES** | `engine.snapshot_resolved(root)` → `find_click_intent_in_region(vm, id, region)` → if `Some(intent)`: `session.execute_operation(intent)`; if `None` (e.g. editable_text in Main): `session.execute_operation("navigation","focus",{block_id})` (the existing component path). This is exactly what `ReactiveEngineDriver::click_entity` does (user_driver.rs:475-510), minus the driver wrapper. |
| (b) **move / reparent** (was `drag_drop_block`) | **YES** | Resolve the drop op from the target's `ViewKind::DropZone { op_name }` (view_model.rs; `drop_zone` prop `op`/`op_name`, default `DEFAULT_DROP_OP_NAME`) → `build_drop_intent(source_id, target_id, target_entity, op_name)` (user_driver.rs:51, builds `{parent_id, ...}` params by id) → `session.execute_operation(intent)`. No coordinates — the `op_name` is a declarative widget prop, resolved by walking the snapshot tree by id (mirrors `drop_entity`'s `walk_tree` lookup, user_driver.rs:704-725, minus the bounds wait). |
| (c) **expand + collapse** (was `expand_toggle`/`collapse_toggle`) | **YES** (already geometry-free) | The current `E2ESut` impl does NOT click — it walks the reactive tree and flips the `expand_toggle` node's `expanded` signal gate directly (`set_expand_toggle_gate`, `pbt/sut_render.rs:114-179`; `ViewKind::ExpandToggle{expanded}` is in-memory tab UI state, ref side is `state.ui.tab.expanded_toggles`, expand_toggle.rs:93). Pure `ReactiveEngine` snapshot + signal `set` — no geometry already. Re-levels cleanly to an `expand(id)`/`collapse(id)` intent over the component's `reactive` engine. |
| (d) **trigger_command** (was `trigger_slash_command`) | **YES for the resolved op; NO for the popup-UI path** — see nuance below | `slash_command_on_enter(engine, block_uri, current_text, cursor_byte)` (`headless_editor_mirror.rs:332-396`) resolves the `/cmd` to an `OperationIntent` BY block_id (sources ops from `services.entity_operations(scheme)`, NOT a rendered node — geometry-free), using the same pure `CommandProvider`/`check_triggers`/`build_command_items`/`on_select` logic GPUI drives. Dispatched at headless_editor_mirror.rs:272 via `engine.dispatch_intent_sync`. The net op (e.g. `block::delete{id}`) is reachable headlessly by id. |

**Slash / trigger_command nuance (the riskiest, explicit):** the slash command has
**two layers**. (1) The *resolved operation* (`/delete` → `block::delete{id}`) IS
reachable headlessly and geometry-free — `slash_command_on_enter` already builds and
dispatches that exact intent (headless_editor_mirror.rs:387-393). (2) The *popup-menu UI
path* — typing `/`, the per-keystroke `on_text_changed` popup-state advance, the filter
narrowing, the `popup_item_selected` widget — is **transient UI that does not reach
`BoundsRegistry` headlessly** (the current `apply_trigger_slash_command_to_sut`,
`transitions/trigger_slash_command.rs:47-89`, drives it via `send_raw_keystroke` "/" +
"delete" + Enter and asserts the popup via `wait_for_widget_kind`, which is a no-op
headlessly per its own comment at line 77). **Re-leveling to an intent cap dispatches the
resolved op but drops the popup-UI-path coverage** — that coverage is keystroke-realized
and belongs with `press_key` in the E4 windowed/driver tier. The intent cap should
therefore target the *resolved command*, and the popup-filter fidelity stays a separate
driver-realized concern.

**Residue that genuinely cannot go headless (stays E4 / driver-realized):**
- `press_key` — keymap/chord resolution through the real editor input pipeline. Its
  *structural consequences* are already independently drivable via existing intent caps:
  `SutBlockTreeWrite::split`/indent/outdent (Enter/Tab map to `structural_block_action`,
  headless_editor_mirror.rs:274-288; the SutHandle-decomposition keystone already drives
  Split/Join/Indent/Outdent against `ReferenceState`). So re-leveling does NOT lose
  structural coverage; only the keymap-resolution leg stays driver-realized.
- `click_at_element` (geometry HANDLE ids like `<kind>::<block-uri>`, drawer/vms toggles)
  and drag *hit-test at coordinates* fidelity — `BoundsRegistry` hit-testing is the
  window's job (E4 `GpuiWindowComponent` / `SutLayout` + `SutDriver`).
- The slash popup-filter UI path (above).

**Net:** select / move / expand / collapse / trigger_command(resolved-op) all re-level to
intent caps realized headlessly via `find_click_intent_in_region` (or `DropZone.op_name` /
`slash_command_on_enter` / the expand-gate) + `session.execute_operation`. `press_key`,
geometry hit-test, and the slash popup-UI path remain the E4 driver/windowed residue.

### E5 🧠 Delete `E2ESut` + the parallel `Subsystem`/`min_sut` selection machinery
With E4 done, `E2ESut` has no remaining caller. Delete it, `sut_capabilities.rs`, and
the legacy `Subsystem`/`min_sut` selection path (the convergence harness + composed
selection fully subsume it). This is the literal §6 / F2 end state.

**Dependency order:** **E0c ✅ DONE — both make-or-breaks eliminated (2026-06-18):**
(a) proved the geometry bodies host over `CapMap`; (b) proved `TestPlatform` geometry
is real + deterministic. The endgame is therefore **full dissolution, no permanent
residue.** Remaining: E1 → E2 → E3 (the cheap headless dissolution, evidence-gated),
then E4 → E5 (the windowed component last — now de-risked to mechanical: productionise
the E0c-(b) boot/settle + host `SutDriver` + the single-threaded settle seam — best
done against an already-proven composed core).

---

## Framework track (🧠, standalone)

- **F1** — Step 3: backfill per-tick caching onto the `CapMap` wrapper; retire the
  `sut_capabilities.rs` absent-sentinel tax as components absorb the real impls.
- **F2** — Subsume the legacy `Subsystem`/`min_sut` slices (`storage_consistency`,
  `general_e2e_sql_only`/`full`, `loro_backend_pbt`) so E2E runs through the shared
  catalog ("E2E" = the full component list). The big convergence; unlocks deleting
  the parallel selection machinery. **Stage 1 LANDED (2026-06-18, see Status); the
  dependency-ordered finish is now `Bundle E` (E2ESut dissolution).** This entry is
  the *why*; Bundle E is the *how*.
- **F6 — Subsystem-config shrinking (faithful harness LANDED 2026-06-17).** The endgame is not
  just "a new component re-runs the catalog for free" but "the shrinker tells you
  which components are *causally necessary* for a bug." The five hand-written
  slices (memory/loro/sql/frontend/sql_loro) collapse, in principle, into **one
  config-generated slice**: the active subsystem set is part of the
  `ReferenceStateMachine::State` (a `BTreeSet<Subsystem>`), generated via
  `subsequence` and shrunk toward fewer; `init_test` builds the `CapMap` from it;
  `check_invariants` runs the shared `run_selected`. **Landed (mechanism proven):**
  `src/pbt/subsystem_shrink.rs` — two **real** optional axes (`Loro` = real
  `LoroBackendComponent`, `EditorState` = real `InMemEditorComponent`; *no fixture
  stand-ins* that would become dead code), config-in-state, and precondition-replay
  transition invalidation (editor transitions gated on `EditorState`; tripwire
  never fired while shrinking `EditorState` off — no fallback needed). Subset
  isolation proven across the powerset: `loro_order`→`{Loro}`, `editor`→
  `{EditorState}`; `content`'s causal minimum `{BlockTree}` is proven by regression,
  though the greedy proptest shrink retains the irrelevant editor (see below).
  Planted via wrong *reference* data, not faked components. All evidence is
  **committed green regression tests** (`regression` + `universe` modules), not
  manual env-gated runs. See Design §8.7.

  **F6.2 — faithful convergence harness LANDED + spike deleted (2026-06-17).** The
  in-process spike (`src/pbt/subsystem_shrink.rs`) is **deleted**; its shared
  seed/plant/proof helpers were lifted into `composed/subsystem_seed.rs` (incl. the
  deterministic shrink-causality regression tests). The faithful version landed as
  `tests/subsystem_convergence_pbt.rs` — it boots the real `E2ESut` via `StartApp`
  (so startup *is* covered, unlike the spike) and can subsume the wide
  `general_e2e_pbt`. It generates `Wiring` via `wiring_axes`/`any_valid_wiring`
  (default storage `{Loro, Org, Turso}`, Turso down-weighted; **no UI** until headless
  editor faithfulness is verified), parameterized through the existing
  `__declare_pbt_full_slice!` machinery (not a clone). `#[ignore]`d; scope a run with
  `HOLON_PBT_WIRING_AXES`.

  **F6.1 — oracle integrated onto real `ReferenceState` (2026-06-16 pm).** The
  bespoke `SpikeRef`/`RefEditor` oracle was replaced by the production
  `ReferenceState` via the new `impl CapProvider for ReferenceState` +
  `reference_state_ref_caps` keystone (Design §8.8). The spike's `State` *is* a
  `ReferenceState` now: fixed shared ids on both sides, editor opened only under the
  UI actor, mirror-only `apply`, plants injected into a clone at check time. **Teeth
  proven:** breaking `editor_caret` reddens the `none`-plant run on
  `inv-editor-caret-matches-ref` with real multi-byte typed text (transitions fire +
  differential bites). **Env→capability cleanup landed in the same change:** editor
  gating is now purely `has_editor_buffer()`, so `PBT_ATOMIC_EDITOR`
  (`atomic_editor_enabled`) was dead and removed; `PBT_REAL_EDITOR`'s commit-on-blur
  effect became a `ReferenceState.real_editor` field set by the driver harness.

  **Deferred / next (dependency-ordered):**
  1. **Committed-content parity** — the deferred half of mirror-only: per-keystroke
     (Loro) / on-blur (SQL, via the new `real_editor` field) commit on both sides, the
     `block_raw` editor invariant, matched `normalize_content_for_org_roundtrip`.
  2. **Structural transitions** (Split/Join/Indent/Outdent/Move) — now viable since
     `ReferenceState` carries a real tree + honest `apply_to_ref`.
  3. **Widen the universe** past in-process subsystems (Turso/Org/frontend —
     cost-asymmetry, gated behind `HOLON_PBT_SUBSYSTEMS`).
  4. ✅ **DONE (2026-06-17)** — migrated the slices + catch triads off
     `FixtureRef`/`FixtureEditorRef` onto `reference_state_ref_caps`, then **deleted
     `FixtureRef`/`EditorModel`/`FixtureEditorRef`/`EditorPureRef`**. The §5/§6
     parallel-ref-model retirement is complete; `ReferenceState` is the single oracle.
  5. **Retire `E2ESut`** (= F2). Still open.
  6. **Shrink-quality** — investigate whether `proptest-state-machine` shrinks
     `init_state` for pre-transition failures (the `content`→`{BlockTree,EditorState}`
     greedy artifact); full ddmin remains out of scope (greedy-only today). Still open.

  Landed on `main` (HEAD `3d811e5`); the spike worktree is reconciled.
- **F3** — Run the still-un-run **full Full/Loro `general_e2e` parity** confirm
  (attended; known pre-existing reds — diff against baseline before deleting any path).
- **F4** — (folded into the bundles) host remaining owned-return ref caps only
  alongside the component that consumes them — never standalone (would be dead code).
- **F5 — Shared generation → folded into F2.** A first attempt at deduping the
  per-slice mutation loops introduced a bespoke `SliceDriver` trait + CRUD generator;
  that **reinvented the write-cap mechanism** (§4.4/§5.1/§6: drive the concrete SUT
  through `SutBlockTreeWrite`/`SutTransitionTarget`; a slice is the component list).
  Reverted. The correct shared generation is to reuse the existing `E2ETransition` +
  `aggregate_transitions` + `ReferenceState` by making each composed slice a concrete
  SUT impl'ing `SutTransitionTarget` — that **is** F2 (started on the memory slice).
  Until then each storage slice keeps its small bespoke CRUD mutation proptest. The
  `sql_loro_slice` randomised task_state/escaping exploration stays a separate
  slice-local item (empty-ref focused proptest; not block-tree).

---

## Cap-host reference

| SUT cap | Provided by | Status |
|---|---|---|
| `SutBackend` | memory + Loro + SQL + frontend components | ✅ have |
| `SutBlockTreeWrite` | `MemoryBackendComponent` (Stage 1) | ✅ have |
| `SutEditorMirrorRead` | `InMemEditorComponent` | ✅ have |
| `SutEditorMirrorWrite` | `InMemEditorComponent` | ✅ have (E1 Stage-1b — `InProcEditorSut` collapsed in; CapMap-hosted) |
| `SutLoroLog` | `LoroBackendComponent` (A1) | ✅ have |
| `SutLoroTaskState` | `LoroBackendComponent`; consumed in `sql_loro_slice` (B6) | ✅ have |
| `SutLoro` (live-tree) | `LoroSut` (peer mesh, hosted via `compose_sut`'s Loro arm) | ✅ have (gated `!has_turso \|\| frontend_sync_handle.is_some()`; drives `AddPeer`/`PeerEdit`/`MergeFromPeer`) |
| `SutSqlProjection` | `SqlProjectionComponent` (B1) | ✅ have (nav/focus/watch honest-empty) |
| `SutRenderer` | `HeadlessFrontendComponent` (C1) | ✅ have (real render pipeline) |
| `SutViewModel` | `HeadlessFrontendComponent` (C1) | ✅ have (headless parts; gpui-engine parts honest-None) |
| `SutCdc` | reactive-engine component | 🔜 E1 (apply-only `drain_cdc` stays `&mut self`) |
| `SutWatchRows` | `HeadlessFrontendComponent` (PRODUCTION reactive watch surface) | ✅ have (E1 — redesigned off E2ESut's `ui_model`; B5 invariants bite) |
| `SutQueryCompile` | `QueryCompileComponent` (pure) / frontend | 🔜 E1 |
| `SutOrgRead` | `HeadlessFrontendComponent` (production `holon_orgmode` parser over its org_fs) | ✅ have (E1 — `inv-blocks-match-ref/org` bites) |
| `SutOrgRender` | `HeadlessFrontendComponent` (production `CacheBlockReader` + `OrgRenderer`) | ✅ have (E1 — `inv-org-render-fixed-point` bites) |
| `SutOrgFileWrite` | `HeadlessFrontendComponent` (org_fs write) | 🔜 E1 (`&mut` apply-path) |
| `SutLifecycle` | booted-session component(s) | 🔜 E1 |
| `SutLayout` + displayed-text | `GpuiWindowComponent` + `GpuiFrontendEngineComponent` (TestPlatform geometry) | ✅ have (E4 inc1–3a) |
| `SutDriver` (→ `window_focus`) | `GpuiDriverComponent` (window engine's `focused_block`) | ✅ have (E4 inc4 + 3b; `#[capmap_adapter]`-hosted) |

All `Ref*` caps are owned-return (host with a one-line `#[capmap_adapter]`) except
`RefBlockTree`/`RefEditorMirror` (already hosted, via the `expect_ref` borrow path).
