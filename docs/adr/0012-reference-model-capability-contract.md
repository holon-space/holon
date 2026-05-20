# ADR 0012: The reference-model capability contract

**Status:** Accepted (2026-06-11; documents architecture that is implemented and load-bearing)
**Deciders:** Martin
**Context:** The PBT reference model is the executable behavioral spec of Holon —
caret conventions, commit-point semantics, sibling-order authority, focus
observability. Until now its contract surface (the capability traits in
`crates/holon-pbt-core/src/capabilities.rs`) was documented only in doc-comments
and session memory ("folklore"), despite being the layer ADR 0007 and ADR 0009
build on.
**Relates to:** ADR 0007 (Wiring manifest), ADR 0009 (component-subset PBTs and
bisection), ADR 0010 (in-memory focus authority), ADR 0005 (children as ordered
list). How-to companion: `docs/Testing/PbtSlicing.md`.

## Problem

Holon runs one reference model against many systems-under-test: the wide
headless E2E SUT (Turso + Loro + Org + reactive ViewModel), the SqlOnly
variant, the windowed GPUI runner, the TUI runner, and narrow component slices
(pure in-memory editor, Loro-backend-only, org-ordering-only). Three forces
collide:

1. **One model, many wirings.** A transition like `SplitBlock` must apply to a
   pure in-memory slice (microseconds) and to a real GPUI window (click, `home`,
   N×`right`, `Enter`) without duplicating its reference-side logic. An
   invariant like `inv-blocks-match-ref/loro` must run when Loro is wired and
   *visibly skip* — not silently pass — when it is not.

2. **Monolithic traits don't scale.** The pre-capability design was one
   `ReferenceState` and one `Sut` god-object. Every narrow slice either
   duplicated the scaffolding or joined the monolith and paid seconds per case.
   Gating by trait bounds on one giant SUT type "scales poorly past a handful
   of capabilities" (ADR 0007's own diagnosis).

3. **The contracts are the bugs.** Multi-day debugging sessions in this repo
   repeatedly bottomed out not in product code but in an *unstated convention
   of the reference model*: where the caret sits after a split, whether a blur
   commits pending editor text, whether sibling order is compared against
   `sort_key` or insertion sequence, whether "no focus" means "no engine".
   Each of these is now encoded in a specific trait method with a specific
   doc-comment — but nothing above the code level says so.

ADR 0007 defined *which adapters a run wires*; ADR 0009 defined *how subsets
form a bisection lattice*. Neither defined the layer both stand on: the typed
contract between the reference model, the transitions, the invariants, and the
concrete SUTs. This ADR documents that layer.

## Decision

### 1. Capability traits are the contract surface

`crates/holon-pbt-core/src/capabilities.rs` (~1500 lines) defines **39 traits
plus ~13 supporting value types** (≈52 public types total). They split along
two axes:

- **Side**: `Ref*` traits are queries/mutations of the *reference model*
  (pure, synchronous); `Sut*` traits are observations/drives of the *system
  under test* (native `async fn`, no `#[async_trait]` boxing).
- **Access mode**: read traits (`RefBlockTree`, `RefEditorMirror`, `RefFocus`,
  …) are separated from write traits (`RefBlockTreeMut: RefBlockTree`, …), so
  invariants — which only read — never carry write bounds.

The reference side (capabilities.rs):

| Cluster | Traits | Owns |
|---|---|---|
| Block tree | `RefBlockTree` (L71), `RefBlockTreeMut` (L152) | structure, content, sibling order, layout/seed/page predicates |
| Editor mirror | `RefEditorMirror` (L190), `RefEditorMirrorMut` (L217) | active editor text + caret + dirty flag |
| Focus | `RefFocus` (L230), `RefFocusMut` (L255), `RefFocusRoots` (L1219), `RefGlobalFocus` (L1389) | per-region nav focus, focus roots, engine-global focus |
| Lifecycle | `RefLifecycle` (L285) | `app_started`, `enable_loro`, `renders_block_interactively`, Markov history |
| Peers | `RefPeers` (L397), `RefPeersMut` (L411) | modeled peer-Loro replicas |
| Layout/render | `RefLayout` (L1226), `RefRender` (L1271) | layout-block sets, render-expr metadata, visible columns |
| Watches / tasks / backend | `RefWatches` (L1332), `RefTaskState` (L1397), `RefBackend` (L1412) | expected watch rows, task states, typed `Block` snapshots |

The SUT side mirrors it per *observable component*, not per invariant
(`docs/Testing/PbtSlicing.md` §2's load-bearing rule: "caps correspond to
abstracted system COMPONENTS, never to individual invariants"):

| Cluster | Traits | Component observed/driven |
|---|---|---|
| Tree/editor/focus writes | `SutBlockTreeWrite` (L328), `SutEditorMirrorWrite` (L338), `SutFocusWrite` (L363) | the seven T0 structural/editing transitions |
| Editor reads | `SutEditorMirrorRead` (L349) | live `InputState` text + tracked caret |
| Quiescence | `SutQuiesce` (L371) | CDC drain / reactive flush / Loro sync barrier |
| Storage projections | `SutSqlProjection` (L584), `SutBackend` (L649), `SutLoroLog` (L559), `SutLoroTaskState` (L676), `SutOrgRender` (L1151), `SutOrgRead` (L1170) | Turso matview/base table, CDC `LiveData` mirrors, Loro tree, org files on disk |
| Loro peers | `SutLoro` (L488) | real LoroDoc import/export peers |
| ViewModel / renderer / geometry | `SutViewModel` (L765), `SutRenderer` (L909), `SutLayout` (L1028) | ReactiveEngine VM, widget tree (`WidgetSnapshot`), window geometry (`RenderedElement`) |
| Driver / lifecycle / misc | `SutDriver` (L1113), `SutLifecycle` (L1204), `SutCdc` (L697), `SutErrorLog` (L546), `SutOrgFileWrite` (L689), `SutWatchRows` (L1362), `SutQueryCompile` (L1190) | input synthesis, app start/restart, CDC state, error logs, org-file writes, watch rows, query compilation |

Two umbrella bounds keep dispatch ergonomic without re-monolithizing:

- `SutTransitionTarget: SutBlockTreeWrite + SutEditorMirrorWrite +
  SutFocusWrite + SutQuiesce` with a blanket impl (capabilities.rs:378-386) —
  the seven T0 transitions' target.
- `SutHandle` (`crates/holon-integration-tests/src/pbt/transition_dispatch.rs:158`),
  the wide-E2E coarse bundle, declared as a supertrait of
  `SutEditorMirrorWrite + SutBlockTreeWrite + SutLoro`. Transitions are
  dispatched generically over a concrete `S: SutHandle` (no `dyn`), which is
  what lets each transition's impl narrow to fine-grained capability bounds
  while the transition enum still dispatches uniformly.

### 2. Transitions and invariants *declare required* capabilities

The binding points live in `crates/holon-pbt-core/src/lib.rs`:

- `TransitionFactory<Ref>` (L51) — generator: `weighted_generator(&Ref)`
  returns `Validated<(weight, strategy), Reason>` (a rejected variant reports
  *why*), plus `required_wiring() -> RequiredWiring` (default
  `Any`) for the structural manifest gate.
- `TransitionRef<Ref>` (L115) — `preconditions` + `apply_to_ref`, generic over
  `Ref` only and **independent of any SUT type**, so S-less contexts (the
  proptest state-machine driver, generators) can run reference logic alone.
- `TransitionImpl<Ref, Sut: ?Sized>` (L128) — `apply_to_sut`. The `Sut` bound
  is *where a transition declares which SUT capabilities it needs*.

Example (`crates/holon-integration-tests/src/pbt/transitions/split_block.rs`):
`SplitBlock` implements `TransitionFactory<R>` for any
`R: RefBlockTree + RefLifecycle`, `TransitionRef<R>` for any
`R: RefBlockTree + RefBlockTreeMut + RefFocusMut + RefEditorMirrorMut +
RefFocus + RefLifecycle`, and `TransitionImpl<ReferenceState, S>` for any
`S: SutBlockTreeWrite` (L275). A separate capability-bound free function
`apply_split_block_input_pipeline_to_sut<S: SutLayout + SutDriver>` (L51)
drives the *physical* GPUI path (wait-for-widget-kind → click-to-focus with
re-click → `home` + N×`right` → `Enter`).

Invariants do the same. `Invariant<R, S>`
(`crates/holon-pbt-core/src/invariant.rs:62`) is deliberately bare —
`id()` + `async check(&R, &S) -> InvariantResult` — and **capability bounds go
on each impl block**, not on the trait, so one registry holds heterogeneous
requirements. E.g. `InvBlocksMatchRefMatview` is
`impl<R: RefBackend, S: SutBackend + SutSqlProjection> Invariant<R, S>`
(`invariants/bodies/blocks_match_ref.rs:58-62`).

### 3. Concrete models *provide* capabilities via blanket forwarding impls

- `crates/holon-integration-tests/src/pbt/reference_capabilities.rs` — blanket
  impls of every `Ref*` trait on the wide `ReferenceState`. Pure forwarding;
  "zero behaviour change: every method delegates to an existing
  `ReferenceState` field or method" (module header).
- `crates/holon-integration-tests/src/pbt/sut_capabilities.rs` — the same for
  `E2ESut` (thin forwards over inherent/`SutHandle` methods; e.g. the
  `SutLoro` impl forwards to the owned `LoroSut` and *panics loud* if Loro is
  not wired, because peer transitions gate on `enable_loro` first).
- Pure slices implement only the traits they have state for; defaults encode
  the degenerate answer (`is_layout_block` → `false`, `RefPeers::peers_len` →
  `0`, `open_active_editor` → no-op for editor-less refs). A slice's `Sut` is
  a plain product struct of capability impls (`PbtSlicing.md` §4).

Capability methods deliberately do **not** take `ref_state`: the SUT keeps any
ref→SUT translation (e.g. synthetic-doc-URI → real UUID `doc_uri_map`) in
interior state (capabilities.rs:324-327, `SutDriver::resolve_ref_block_id`
L1139-1143).

### 4. Eligibility = compiler × manifest × registry; bisection derives from the lattice

Three independent gates, each visible:

1. **Compile-time**: a transition or invariant simply cannot be instantiated
   against a slice missing its capability bounds. The compiler computes the
   slice's alphabet.
2. **Manifest (ADR 0007)**: `Wiring` (`wiring.rs:85`) declares provided
   storage/sync/actor adapters; `RequiredWiring` (`wiring.rs:257`, with
   disjunction via `AnyStorageOf`/`AnyOf`) declares needs. *Necessary, not
   sufficient* — `weighted_generator` still gates dynamically.
   `Wiring::validate()` (`wiring.rs:226-244`) enforces the three validity
   rules; `blessed_manifests()` pins the four CI sets.
3. **Registry**: each invariant's `InvariantSpec`
   (`invariants/registry.rs:290-302`) carries `min_sut: BTreeSet<Subsystem>`,
   a `RunMode` (`Strict`/`Warn`/`SkipOnCdcLag`), and a `TickGate`. The runner
   (`invariant_runner.rs::run_one`, L595) executes a body iff subsystem
   selection ∧ tick gate ∧ `required_wiring_for_subsystems(min_sut)` all hold;
   metadata lives only in the registry, dispatch tables carry bodies only, and
   the oracle test `native_runner_dispatches_exactly_the_registry`
   (`invariant_runner.rs:1044`) proves the disjoint union covers the registry.

`ComponentSet` (`component_set.rs:67`) = `Wiring` + toggleable observable
projections (`ViewModel`, `EditorState`). Its `valid_children()` /
`valid_parents_within()` (L243-312) define the bisection lattice;
`bisect_downward` / `bisect_upward` / `bisect` (`bisect.rs:39-98`) walk it and
report `DownwardMinimal` / `UpwardMinimal` / `NotReproduced` honestly
(absent-component bugs invert the direction — ADR 0009 §3). The `Stepper` seam
(`stepper.rs:62`) plus `ReplayMode::SkipGated` (`stepper.rs:37-46`) make a
recorded concrete sequence replayable across lattice nodes, with gated-out
transitions becoming `StepOutcome::SkippedByGating` instead of desyncs.

`CachingProxy<'a, S>` (`caching_proxy.rs:47`) is the per-tick read view: it
implements the read-side `Sut*` traits over a borrowed SUT and memoizes the
expensive component snapshots (`rendered_elements`, `live_block_snapshot`,
`frontend_root_vm`, `all_block_ids`, …) so the ~40 invariant bodies that run
per tick share one observation instead of re-querying. It intentionally does
**not** implement `SutCdc::drain_cdc` — draining inside a tick would let two
invariants see different generations.

### 5. The key behavioral contracts (formerly folklore)

These are the contracts whose violation has historically cost multi-day
debugging sessions. They are now *part of the capability surface* — each is a
trait method (or shared helper) with the convention in its doc-comment. This
section makes them citable.

#### 5.1 Editor-mirror caret conventions

- **After split: focus and active editor move to the NEW block at caret 0.**
  `RefFocusMut::open_active_editor` (capabilities.rs:262-272): "`split_block`
  returns the freshly-created block as the focus target at position 0 (op
  response, applied in-process)" per ADR 0010 — so a *subsequent* Enter splits
  the NEW block, not the prior `FocusEditableText` target. The ref-side apply
  (`split_block.rs::split_block_apply_to_ref`, L211-247) sets both
  `set_focus(..)` and `open_active_editor(new_block, new_content, 0)`; leaving
  `active_editor` stale was the root of the settled-consistent
  `inv-blocks-match-ref` content-divergence family.
- **After join: caret sits at the join boundary.**
  `RefBlockTreeMut::join_block` returns "the cursor position of the join point
  in the merged block's content" (capabilities.rs:165-168). The prod mirror
  arms the same seed ("split → 0, join → boundary",
  `crates/holon-frontend/src/headless_editor_mirror.rs:74-77`).
- **Click seeds the caret to end-of-text — unless that block's editor is
  already active, in which case the click is a no-op on the caret.**
  `model_chord_click_focus`
  (`crates/holon-integration-tests/src/pbt/transitions/mod.rs:51-78`): early
  return when `active_editor_block() == Some(block_id)`; otherwise blur-commit
  the previous editor (dirty-gated, §5.2), then
  `open_active_editor(id, content, content.len())`. Production equivalent:
  `HeadlessEditorMirror::seed_for_click`
  (`headless_editor_mirror.rs:70-92`) — a click *overrides* any armed
  op-followup seed, "just like a real mouse click re-places a GPUI caret".
- **Split positions are byte offsets and MUST sit on a char boundary.** Prod
  positions come from an editor caret (always boundary-aligned); the
  generator enumerates `char_indices`, and a mid-codepoint byte (possible only
  via hand-edited captures) must *reject in preconditions*, not panic
  (`split_block.rs:157-165`).
- **Caret/live-text are observable-or-disclosed.** `SutEditorMirrorRead`
  (capabilities.rs:349-360) distinguishes `Err(reason)` = "caret unobservable
  in this SUT/driver medium" (invariant reports a disclosed Skip) from
  `Ok(None)` = "observable medium, no caret tracked yet". No silent green.

#### 5.2 Commit-point and settle semantics

- **Structural ops are commit points.** From
  `docs/Architecture/UI.md` §"Field authority and intent capture" (L67):
  pending editor state flushes through the normal merge path *before* the op
  executes, in one ordered dispatch. Canonical violation: "`Split position 8
  exceeds content length 3`" — split computed against backend content using a
  cursor byte into the editor's pending text. The reference encodes this as
  `commit_active_editor_if_dirty` called at the top of every structural
  `apply_to_ref` (`split_block_apply_to_ref` L211-226, `model_chord_click_focus`
  L70).
- **Blur is a flush *hint*, not the commit mechanism** (UI.md L69). SqlOnly
  prod still commits on blur today; the ref models the blur-commit on
  click-away — but **dirty-gated**.
- **Dirty-gating is load-bearing.** `RefEditorMirror::active_editor_dirty`
  (capabilities.rs:203-213): only text authored by *modeled* typing/deleting
  commits. A clean mirror that merely diverged from `block.content` is stale
  against an external change (prod's data subscription refreshes idle
  editors); committing it writes old text into the ref. An unconditional
  commit here produced the Full/Loro divergence of 2026-06-11
  (`split_block.rs:218-221`). Helpers: `commit_active_editor_if_changed`
  (capabilities.rs:1443) and the dirty-gated wrapper
  `commit_active_editor_if_dirty` (capabilities.rs:1472).
- **Invariants read only quiesced state.** `SutQuiesce::quiesce`
  (capabilities.rs:371-373) is the uniform barrier (pure slice: no-op; wide
  PBT: drain CDC + flush reactive engine + await Loro sync). All of
  `SutSqlProjection` reflects Turso "AFTER CDC quiescence — invariants must
  call `quiesce()` first" (capabilities.rs:583-584). The wide runner
  additionally runs `settle_before_invariants` / `settle_on_snapshot`
  (`invariant_runner.rs:164-252`): poll the convergent snapshot until the
  non-seed id set AND per-block normalized content match the reference —
  content is part of the barrier because some prod writes detach from the
  request path. CDC mirrors drain to a quiet-window watermark
  (`sut_cdc_mirrors.rs::wait_quiescent`, L139-166).
- **Eventual-consistency downgrades are classified, not assumed.** Matview
  invariants carry `RunMode::SkipOnCdcLag` plus a `block_raw` truth-check
  (`SutSqlProjection::nav_history_open_rows` doc, capabilities.rs:631-638;
  `SutWatchRows::block_raw_query_ids`, L1372-1378) so a divergence is
  attributed to CDC delivery lag *only when the write side has already
  converged* — otherwise it fails as a real write-pipeline bug.

#### 5.3 Ordering: what `RefBlockTree` promises vs Loro/SQL authority

- **Canonical sibling order has exactly one owner per wiring.**
  `Wiring::ordering_authority()` (`wiring.rs:215-220`): fixed priority
  `Loro > Org > Markdown > Turso` (`ORDERING_PRIORITY`, `wiring.rs:76-81`) —
  "Turso never decides sibling order on its own"; the org re-ingest place-loop
  has the SQL owner *adopt* org line order. This resolves ADR 0007 open
  question #3 and implements ADR 0005's authority rule.
- **The SQL projection orders by `sort_key`; the reference orders by
  `(sequence, id)`.** `SutSqlProjection::sorted_children`
  (capabilities.rs:593-598) returns children "ordered by `sort_key` (the
  authoritative fractional index)"; `inv-live-children-match-ref` compares it
  against `RefBlockTree::sorted_children` (capabilities.rs:99-102, "sorted by
  sort_key" on the ref's own bookkeeping).
- **Source/render artifacts are order-exempt, membership-enforced.**
  `RefBlockTree::is_order_exempt_sibling` (capabilities.rs:135-146): `::src::`
  / `::render::` artifacts' relative order legitimately differs between the
  two orderings after a file-sync round trip reassigns sort_keys — the
  invariant relaxes intra-group *reordering* only, never membership.
- **Seed blocks are outside the comparison universe.**
  `RefBlockTree::all_non_seed_block_ids` (capabilities.rs:124-133): blocks
  with sentinel/no-parent docs are inserted via direct SQL, never
  reverse-synced to Loro, and don't appear in the compared matview.
  `RefBackend::seed_block_ids` (L1418-1422) filters them out of the CDC-lag
  truth check; `RefBackend::org_blocks` (L1423-1431) additionally drops page
  blocks (org files hold none).

#### 5.4 Focus observability

- **"No engine" ≠ "no focus".** `EngineFocus`
  (capabilities.rs:1096-1110) is a three-state enum
  (`NoEngine`/`Unfocused`/`Focused(id)`) precisely because conflating the two
  in an `Option` "made the focus steal-back bug family read as green: a lost
  focus looked identical to SqlOnly mode and was skipped instead of failed."
- **Engine focus moves synchronously; window focus follows asynchronously.**
  `RenderedElement::focused` (capabilities.rs:1019-1024): "the divergence
  window is exactly the steal-back/zombie-editor bug family." Input-bearing
  transitions must gate on `SutLayout::wait_for_window_focused_editor`
  (L1077-1088) — keystrokes dispatched before window focus lands are consumed
  by the previously-focused editor. Post-click,
  `SutDriver::wait_for_engine_focus` (L1122-1126) is the explicit barrier
  because GPUI clicks go through fire-and-forget `dispatch_intent`.

### 6. Relationship to ADR 0007 and ADR 0009

The three ADRs are one stack:

- **ADR 0012 (this): the contract layer.** Capability traits define *what
  behaviors exist* and what each promises (caret, commit, ordering, focus,
  quiescence). Transitions/invariants consume them; reference model and SUTs
  provide them.
- **ADR 0007: the provisioning layer.** `Wiring` declares which providers a
  run has; `RequiredWiring` is the value-level shadow of a transition's
  capability needs, used where the type system can't reach (alphabet
  selection, replay gating).
- **ADR 0009: the lattice layer.** `ComponentSet` extends `Wiring` with
  toggleable projections; subset replay + bidirectional bisection localize a
  failure to the smallest component set where reference and projections
  disagree. Bisection is only sound because §5's contracts make the reference
  apply wiring-independently and make gated-out steps deterministic skips.

ADR 0007's enums and validity rules, including the previously-open
`ordering_authority` (#3) and blessed-vs-valid `is_blessed` guard (#4), are
implemented in `wiring.rs`; ADR 0009's `ComponentSet`/`Localization` in
`component_set.rs`/`bisect.rs`. Its production-DI alignment ("production
binaries also accept a `Wiring` at startup") remains unrealized — `Wiring` is
consumed only by the PBT stack today.

## Consequences

### Payoff

- **One reference model, every runner.** `apply_to_ref` is written once per
  transition against `Ref*` bounds and runs unchanged under the proptest
  headless engine, the GPUI windowed runner, the TUI runner, fixture replay,
  and every bisection node (`run_sequence` over the `Stepper` seam,
  `stepper.rs:97`).
- **Slice-based localization with/without Turso/Loro/Org.** A capture from a
  full run replays down the lattice; the failing frontier names the component.
  The cross-layer report (`invariant_runner.rs::format_layer_report`) sorts
  findings by deepest subsystem ("trouble begins at: …") for single-run
  triage; bisection isolates by actually removing components.
- **No silent coverage loss.** Skips are disclosed (`InvariantResult::Skipped`
  with reason; `Err(reason)` observability results), the registry oracle test
  fails when an invariant is registered but dispatched nowhere, and rejected
  generator variants report `Reason`s instead of vanishing.
- **The behavioral spec is executable and cited.** Every §5 convention is a
  trait method; a convention change is a compile-visible diff plus a failing
  invariant, not a wiki edit.

### Cost

- **Capability sprawl.** ~52 public types / 39 traits in one 1500-line module,
  plus two forwarding files (`reference_capabilities.rs`,
  `sut_capabilities.rs`) that are almost pure boilerplate. Each new observable
  component adds a trait, a forward, a registry entry. Accepted: the
  forwarding is mechanical, and the alternative (god-traits) is what this
  design replaced. The discipline that keeps sprawl bounded is the
  "caps = components, never invariants" rule.
- **Doc-comments are load-bearing spec.** The §5 contracts live in
  doc-comments; nothing machine-checks that e.g. `seed_for_click`'s prod
  behavior and `model_chord_click_focus`'s model stay in sync beyond the PBT
  runs themselves. (That is the point of a PBT — but the *first* divergence
  presents as a confusing model-vs-prod fight, as the 2026-06 caret sessions
  showed.)
- **Where-clause weight.** Transition impls carry 4–6 trait bounds; adding a
  capability to a shared helper ripples through every caller's bounds.
  Mitigated by umbrella traits (`SutTransitionTarget`, `SutHandle`) at the
  dispatch boundary only.
- **Partial implementations exist.** `SutLoroTaskState::loro_task_state_of` is
  declared but `unimplemented!()` on `E2ESut` pending the LoroSyncController
  tag-projection plumbing (capabilities.rs:676-683) — the trait surface runs
  ahead of providers by design, but each such gap is a latent
  panic-on-first-use.

## Known weaknesses / open questions

1. **Stale module header.** capabilities.rs:1-12 still reads "Status — DRAFT
   (Phase 1) … not wired into any consumer". It is wired into every consumer
   (`SutHandle` supertraits it, the invariant registry binds on it, the
   caching proxy implements it). Should be updated to point here.
2. **`Localization::Combination` never landed.** ADR 0009 §3 specifies a
   `Combination{a, b}` verdict; `bisect.rs:17-29` has only `NotReproduced` /
   `DownwardMinimal` / `UpwardMinimal`. Combination bugs currently surface as
   a `DownwardMinimal` two-element set, which is adequate but unlabeled.
3. **`CapCursor` is structurally line/column but conventionally byte-offset.**
   `CapCursor` (capabilities.rs:61-65) carries `line`+`column` "to mirror
   GPUI", yet the caret contracts in §5.1 are byte-based and
   `open_active_editor` takes a separate `cursor_byte`. Call sites mostly pass
   `CapCursor::default()`. Candidate for collapse to a byte newtype.
4. **`RequiredWiring` duplicates what trait bounds already say.** A
   transition's capability bounds and its `required_wiring()` are kept
   consistent by hand (the no-Loro editor-gating asymmetry in ADR 0009
   §"Asymmetries" was exactly such a drift). Deriving one from the other is
   open.

## References

- `crates/holon-pbt-core/src/capabilities.rs` — the contract surface.
- `crates/holon-pbt-core/src/{lib.rs, wiring.rs, component_set.rs, bisect.rs, invariant.rs, caching_proxy.rs}`.
- `crates/holon-integration-tests/src/pbt/{reference_capabilities.rs, sut_capabilities.rs, transition_dispatch.rs, invariant_runner.rs, invariants/registry.rs, stepper.rs, sut_cdc_mirrors.rs}`.
- `docs/Testing/PbtSlicing.md` — the how-to companion (slice construction, anti-patterns).
- `docs/Architecture/UI.md` §"Field authority and intent capture" — the production contract §5.2 models.
- ADR 0005 (children as ordered list), ADR 0007 (Wiring manifest), ADR 0009 (component subsets + bisection), ADR 0010 (in-memory focus authority).
