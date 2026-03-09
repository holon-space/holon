# ADR 0009: Component-subset PBTs and component bisection

**Status:** Accepted (2026-06-09; revised after senior review + headless-engine landing)
**Deciders:** Martin
**Context:** PBT feedback-cycle speed, GPUI↔headless unification, bug localization
**Supersedes/extends:** ADR 0007 (Wiring manifest for PBT subsets)

## Problem

Two goals are currently under-served:

1. **Fast, scoped feedback.** When working on functionality that touches a specific
   pair of components (e.g. Loro + ReactiveViewModel), there is no cheap PBT that
   exercises *only* those components. The blessed slices are either large
   (`general_e2e_pbt_full`) or backend-shaped (`loro_backend_pbt`), and the
   expensive UI path (`gpui_ui_pbt`) is a separate harness entirely. The desired
   workflow — *iterate fast on a small component set, then widen to bigger sets to
   confirm nothing broke under more realistic wiring* — has no first-class support.

2. **Bug localization from a GPUI observation.** When a bug is observed in the
   GPUI app, the path from "I see it" to "the PBT reproduces it, in component X,
   at line Y" is manual. The cross-layer report (`invariant_runner.rs`, "trouble
   begins at: <lowest layer>") helps, but it *infers* a frontier from a single run
   rather than *isolating* by toggling components in and out.

ADR 0007 introduced the `Wiring` manifest and got us most of the conceptual way
there: transitions declare `RequiredWiring`, and a manifest selects the active
alphabet + reference fragments. But three things diverged from that vision in
practice, and they block both goals:

- **Invariants are gated by a *separate* axis (`Subsystem` / `min_sut`), not by
  `Wiring`.** `Subsystem` (`BlockTree`, `Loro`, `TursoProjection`, `Cdc`,
  `ViewModel`, `Renderer`, `EditorState`, `FrontendBounds`, `Driver` — 9 variants,
  `registry.rs:11-33`) and `Wiring` (storage/sync/actor adapters) are two
  independent knobs kept consistent by hand. Selection today is the *intersection*
  of `min_sut ⊆ suite.subsystems` (`registry.rs:341`) **and**
  `required_wiring_for_subsystems(min_sut).satisfied_by(wiring)`
  (`invariant_runner.rs:318,532`).
- **UI is not in the wiring at all.** Whether the UI component is checked is
  inferred at *runtime* from `frontend_geometry.is_some()`
  (`invariant_runner.rs:254`, selecting `Subsystem::all()` vs
  `Subsystem::headless_wide()`), and whether the UI is *driven* is a runner choice
  (proptest-macro headless vs. the phased GPUI loop in
  `frontends/gpui/tests/gpui_ui_pbt.rs` + `pbt_harness/mod.rs`). The component set
  is therefore implicit and split across three mechanisms.
- **There is no way to hold a *sequence* fixed and vary the component set.** That
  operation is precisely what bug localization (goal 2) and "widen the set"
  (goal 1) both need — and, crucially, it is **not** what replaying a proptest RNG
  seed does (see §4).

The alternative to fixing this — keep minimizing per-combination boilerplate but
accept a combinatorial set of hand-written harnesses — does not scale and does not
deliver component bisection.

## Decision

Make a single **`ComponentSet`** the source of truth for a PBT run, and derive
*everything* from it: the transition alphabet, the invariant selection, the
reference fragments, **and** the runner. `ComponentSet` is `Wiring` (ADR 0007)
extended to include the UI as a first-class component, paired with the rule that
the invariant `Subsystem` selection is *derived from* the component set rather than
chosen independently.

We keep **one parametrized engine** plus **thin, named per-combo entry points** —
not a single fully-runtime-parametrized test, and not N hand-written harnesses.

Bisection and cross-runner replay operate on **recorded concrete transition
sequences**, never on proptest RNG seeds (§4). This is the single most important
correction relative to the first draft of this ADR.

### 0. `ComponentSet` is a precisely-typed thing (not a heterogeneous bag)

A `ComponentSet` is `Wiring` plus an explicit set of *observable subsystems that
can be toggled as bisection nodes*. Concretely it is a struct, and its constructors
take a closed enum — not a mixed `[Loro, ViewModel, UI]` literal of three different
types:

```rust
/// A component that can be present/absent in a run and toggled during bisection.
/// Storage/sync/actor map onto ADR 0007 Wiring; the rest are observable projections.
enum Component {
    Storage(StorageAdapter),   // Loro | Turso | Org | Markdown   (ADR 0007)
    Sync(SyncAdapter),
    Actor(Actor),              // UI | MCPServer | ActionEngine    (ADR 0007)
    ViewModel,                 // the ReactiveEngine projection — a real, droppable node
    EditorState,               // InputState + active-editor mirror
}

struct ComponentSet { wiring: Wiring, projections: BTreeSet<Projection> }
//                                    ^ ViewModel, EditorState — the toggleable observers
```

`ComponentSet::of([..])` lowers a literal into `{wiring, projections}`. The point
of naming the member type is the project's own *parse-don't-validate* rule: a
`ComponentSet` is parsed once and never re-validated as a loose tuple of strings or
mixed enums downstream.

### 1. `Subsystem` selection is derived from `ComponentSet`, and the mapping is total

UI becomes a component, not a runner-level runtime sniff. `Actor::UI` in the set is
what turns on the `FrontendBounds` subsystem and selects the GPUI runner —
replacing the `frontend_geometry.is_some()` inference. **The mapping is total over
all 9 `Subsystem` variants** (the first draft omitted four — `BlockTree`, `Cdc`,
`Renderer`, `Driver` — which would have silently *stopped* checking them):

```rust
impl ComponentSet {
    /// Invariant subsystems to check. Total over the Subsystem enum; derived, never
    /// chosen by hand. Must reproduce today's headless_wide() / all() selections
    /// exactly (acceptance test: per-slice selected-invariant diff, Task 1a).
    fn subsystems(&self) -> BTreeSet<Subsystem> {
        let mut s = BTreeSet::new();
        // Always-on observers (present in every headless run today):
        s.extend([Subsystem::BlockTree, Subsystem::Driver]);
        if self.has(Projection::ViewModel) {
            s.extend([Subsystem::ViewModel, Subsystem::Renderer]);
        }
        if self.has(Projection::EditorState) { s.insert(Subsystem::EditorState); }
        if self.has_storage(Turso) { s.extend([Subsystem::TursoProjection, Subsystem::Cdc]); }
        if self.has_storage(Loro)  { s.insert(Subsystem::Loro); }
        if self.has_actor(UI)      { s.insert(Subsystem::FrontendBounds); }
        s
    }
}
```

> Note vs first draft: `ViewModel` and `EditorState` are **gated by membership**, not
> hardcoded always-on. This resolves the contradiction where the flagship bisection
> example (§3) drops `ViewModel` while §1 asserted it was always present. The exact
> always-on set (`BlockTree`, `Driver`, possibly `Renderer`) is pinned by Task 1a,
> not by assertion in this doc.

The reference model is always present and is the **oracle by convention, not by
proof** (see §6 — in this codebase the ref has repeatedly been the buggy party).
Every *enabled* component is checked against the ref; disabling a component drops it
as a *target* of cross-component invariants (e.g. `inv-blocks-match-ref` checks the
ref against whichever of {Loro, Turso, ViewModel, UI} are on), never weakening the
checks on the components that remain. This is what makes "arbitrary valid subset"
well-defined: a subset is "check the ref against the projections that are on."

### 2. One engine, thin named entry points (chosen naming model)

The engine is a single parametrized runner:

```rust
fn run_component_pbt(set: ComponentSet, src: SeedSource) -> TestOutcome
```

`SeedSource` is an explicit enum, because the first draft left it undefined exactly
where the design's correctness lives. It builds on the **JSON capture/replay system
that already exists** (`tests/.captures/*.captured.json`; `Fixture`/`replay_steps`,
`fixtures/mod.rs:153`; concrete `serde`-tagged `Vec<E2ETransition>`,
`transition_dispatch.rs:386`):

```rust
enum SeedSource {
    Generate { cases: u32 },          // proptest TestRunner; headless only; shrinks
    ReplayCapture(Fixture),           // EXISTING concrete Vec<E2ETransition> JSON — portable across sets (§4)
    ReplayRegressionFile(PathBuf),    // proptest RNG seeds; ONLY valid for the SAME set
}
```

Blessed combinations are declared in **one line each** — enough to give CI a named
`#[test]` and (for headless sets) a *per-set* regression file:

```rust
component_pbt!(loro_vm_fast   = ComponentSet::of([Loro, ViewModel]));        // fast inner loop
component_pbt!(full_headless  = ComponentSet::all_headless());               // wide, cheap
component_pbt!(full_gpui      = ComponentSet::all());                        // wide + real UI
```

This is **not** combinatorial in source: the per-combo declaration is one line, and
we only *name* the combos worth blessing — not all 2ⁿ. Arbitrary unblessed subsets
remain runnable at runtime for ad-hoc work, without a `#[test]` each.

> **Caveat (per-set regression files):** only `SeedSource::Generate` sets own a
> `.proptest-regressions` file. UI-on sets run the phased GPUI loop, which uses no
> proptest macro and produces **no** regression file (§5, open q 1). Their
> reproducibility comes from `ReplayTrace` of a recorded sequence, not from a seed
> file. The declaration macro must not promise a regression file for UI sets.

### 3. Component bisection (the headline new capability)

Given a failing **recorded sequence** (an existing `tests/.captures/*.captured.json`
fixture, or a fresh capture from the GPUI runner — *not* a raw RNG seed), **replay
the same sequence across a lattice of component sets** and report the *smallest* set
that still reproduces the failure:

```
full_gpui          fails   →  drop UI
full_headless      fails   →  drop Turso
{Loro, ViewModel}  fails   →  drop ViewModel
{Loro}             passes        ⇒  bug enters at the ViewModel projection of Loro
```

This is more *isolating* than the single-run cross-layer report, because it removes
the component rather than inferring a frontier. **But its output must be framed
honestly:** bisection reports *"the smallest set where the reference and the enabled
projections disagree"* — it does **not** assert which side is correct. In this
codebase the divergence has frequently been a reference-model fidelity gap, not a
prod bug (see §6). The bisector localizes *where* the disagreement enters; deciding
*who is wrong* remains a human step.

The bisection driver walks a **downward-closed lattice of valid subsets** of the
failing set (validity per §"Migration" step 6: `UI ⟹ ≥1 storage + ViewModel`),
reusing the engine per node. Because validity prunes the powerset, the minimal
*valid* reproducing set may be coarser than the true minimal cause; the report says
so.

**Bidirectional bisection (absent-component bugs).** Standard delta-debug assumes
downward closure: "if S reproduces, some subset does too." A large and *recurring*
class of bugs in this project violates that — they manifest only when a component is
*absent* (missing-fallback / not-driven bugs: click-to-focus, drawer-closed render).
For these the minimal reproducer is at the *small* end and naive "shrink until it
passes" mis-localizes. The bisector therefore probes **both directions** and labels
the result:

- `DownwardMinimal{set}` — failure present in `set`, absent in all valid children
  (the classic "bug enters when component X is added").
- `UpwardMinimal{set}` — failure present in `set`, absent in all valid *parents*
  (a missing-fallback bug: "bug enters when component X is removed").
- `Combination{a, b}` — present iff both present; minimal reproducer `{a, b}`.

### 4. Concrete-sequence portability is the load-bearing invariant (NOT seed portability)

For bisection and cross-runner replay to be meaningful, **a recorded concrete
sequence generated under a superset must be deterministically replayable against a
subset** — a transition the subset gates out becomes a deterministic skip/no-op on
replay, never a desync that produces a *different* failure.

This must be built on **the existing JSON capture system, not proptest RNG seeds** —
and the existing system already gives us most of it. What it does *not* yet give us
is cross-set skip semantics; that is the actual net-new work.

**What already exists (verified):**

- `E2ETransition` derives `serde::Serialize/Deserialize`; the JSON is
  variant-name-tagged, capturing **concrete values** (block IDs, text, URIs), not an
  RNG seed (`transition_dispatch.rs:386`).
- On a panicking proptest case, the SUT `Drop` impl writes the concrete
  `Vec<E2ETransition>` to `tests/.captures/<test>.captured.json` (`slice.rs:680`,
  idempotent first-writer-wins).
- `replay_steps()` re-applies that concrete sequence step-by-step, **independent of
  proptest's RNG** (`fixtures/mod.rs:153`): ref `apply` → SUT `apply` → invariants.
- Transition gating is *structural* — a gated variant is simply absent from the
  alphabet, and variant identity is **not renumbered** (`transition_dispatch.rs:478`).
- The reference model applies the same delta regardless of components (`apply` is
  pure/wiring-independent, `state_machine.rs:235`).

> Why **not** proptest RNG seeds (the trap the first draft fell into): a
> `.proptest-regressions` entry is an **RNG seed**, and proptest regenerates the
> sequence by replaying RNG draws against the *current alphabet*. Change the
> `ComponentSet`, change the alphabet, and the same seed picks **different
> transitions** — a different run, not the same run with skips. Use the JSON
> capture, never the regression seed, for anything cross-set.

**What is net-new (the actual gap for bisection):**

1. **Cross-set skip semantics.** Today `replay_steps()` is **strict**: a transition
   inapplicable under the replay's wiring is a **hard panic** ("fixture encodes a
   stale assumption", `fixtures/mod.rs:167`). That is correct for same-set fixture
   regression testing, but **wrong for bisection** — replaying a superset capture
   against a subset *must* turn a gated-out transition into a deterministic
   `SkippedByGating`, recorded per step, not a panic. Bisection needs a replay mode
   (`ReplayMode::SkipGated` vs the existing `ReplayMode::Strict`) that distinguishes
   a gating-skip from a genuine divergence. **This is the core of step 3.**
2. **Wiring is not in the capture.** The JSON omits wiring (it lives on the test
   declaration). For bisection that is a *feature*: the bisector supplies the wiring
   per lattice node and replays the same capture against each.
3. **GPUI capture.** The auto-capture `Drop` path is on the `declare_pbt_slice!`
   (headless) wrapper. The phased GPUI loop does not write captures; it must, so a
   UI-observed failure becomes a `tests/.captures/*.json` the headless lattice can
   replay.
- proptest RNG seeds remain useful **only for replay against the identical set**
  (`SeedSource::ReplayRegressionFile`) — never across the lattice.

### 5. Runner selection is derived, not chosen

`ComponentSet::needs_real_window()` (≡ `has_actor(UI)`) selects the runner:

- **No UI** → the proptest `state_machine!` runner: shrinking, `cases: N`,
  `.proptest-regressions` replay. Cheap, CI-default, the fast inner loop.
- **UI** → the phased GPUI loop (`harness = false`, window on the main thread,
  driver on a background thread; `frontends/gpui/tests/pbt_harness/mod.rs`).
  Expensive, **display-bound** (needs a real DISPLAY; ~hundreds of seconds per run —
  may not be a CI default), no proptest shrinker. It **replays headless-found
  failures as `ReplayCapture(Fixture)`** (the existing `tests/.captures/*.json`) so
  every headless bug is re-checked on a real window, and it **writes its own
  captures** on failure (§4, net-new item 3). It does *not* replay headless *RNG
  seeds* (§4).

Both runner backends sit behind the one engine signature — and the unification can
go **deeper than signature**, because `proptest-state-machine` has no thread
affinity. Its `test_sequential` (`test_runner.rs:75`) is a plain synchronous
`for`-loop (`init_test` → for each transition: `ref.apply`, `sut.apply`,
`check_invariants` → `teardown`); the crate spawns nothing and runs nothing in
parallel. The main-thread constraint is **entirely GPUI's** (macOS window event
loop), not proptest's. Therefore:

- **The loop body is shared for the headless runner and for all replay.** Extract
  `test_sequential`'s body into `run_sequence(.., stepper, mode)` parametrized over a
  `Stepper` seam. Headless generation routes through it (via `SmtStepper<T>`), as does
  every replay/bisection run on either runner. **The live GPUI *generator* is a
  deliberate exception** (see GPUI finding in §"Migration" 2): mid-sequence window
  launch, seed-reproducible incremental generation, and per-step
  gesture/screenshot/`catch_unwind` wrappers keep its loop shell hand-rolled. It still
  shares the per-step *primitives* (`drive_transition`/`apply_transition_async`/
  `run_invariant_registry`) with the engine. GPUI *replay* uses `GpuiReplayStepper`,
  which borrows the live SUT + driver — no main-thread bridge needed (the driver
  inside the SUT already owns the window round-trip).
- **The only genuine divergence is the shrinker, and the cause is SUT
  re-instantiation cost, not threading.** proptest shrinks by re-running
  `test_sequential` (hence `init_test`) many times; for GPUI `init_test` builds a
  fresh window/`Application`, which is expensive and effectively process-singleton on
  macOS. UI sets therefore forgo the proptest shrinker and rely on
  component/sequence bisection plus replayed headless captures — already the plan.
  - **SUPERSEDED for the GPUI path (2026-06).** `RebindHandle`
    (`frontends/gpui/src/lib.rs`) re-points ONE window at a fresh `E2ESut` per
    candidate, so the fresh-`Application`-per-shrink cost that forced "forgo the
    proptest shrinker" is gone. `gpui_ui_pbt` now runs a real
    `proptest-state-machine` runner (`WindowedRefMachine::sequential_strategy`)
    that shrinks the failing `Vec<E2ETransition>` in-process through the shared
    `pbt_harness::windowed_replay` service, with signature pinning so shrinking
    can't drift to a different failure. The hand-rolled incremental *generator*
    (`phased::run_pbt_with_driver_sync_callback`) is retained only because the
    **TUI** PBT (`frontends/tui/tests/tui_ui_pbt.rs`) still uses it.
- **Generation can move to the GPUI driver thread too.** Strategy sampling
  (`transitions(ref_state)`) is pure CPU with no main-thread need, so `full_gpui` can
  *generate* fresh sequences (not only replay) on its background driver, writing a
  JSON capture on failure (§4). It loses nothing it has today.

The shared `ReactiveEngine` is already unified to one instance
(`sut_check_invariants.rs` `ensure_reactive_engine`), so §"Asymmetries" 2 is narrow.

## Consequences

- **Goal 1 (fast scoped feedback)** is met: `component_pbt!(x = of([Loro, ViewModel]))`
  is a one-line, fast, shrinking, headless test. Widening = changing the set (or
  replaying the recorded shrunk sequence up the lattice).
- **Goal 2 (localization)** is met by bisection over **recorded sequences**: observed
  GPUI bug → capture sequence → reproduce in `full_gpui` → bisect down/up → smallest
  reproducing set names where ref/projection disagreement enters.
- The GPUI/headless **superset relationship becomes explicit and derived** rather
  than a runtime `frontend_geometry.is_some()` branch. `full_gpui`'s subsystems ⊇
  `full_headless`'s by construction (the change-detector test generalizes to
  "subsystems(set) is monotonic in set").
- `Subsystem` stops being an independently-chosen knob; it is a *projection* of
  `ComponentSet`. One source of truth, no hand-kept consistency — once Task 1a
  proves the derivation reproduces today's selection.
- ADR 0007 open question #4 (combinatorial test surface) is answered: bounded by the
  blessed list for CI, with the *valid* tail reachable on demand.

## Asymmetries to resolve as part of this work

These exist today and would corrupt bisection/portability if left:

1. **No-Loro editor-transition gating.** Atomic-editor transitions declare
   `RequiredWiring = HasStorage(Loro)`, so under `sql_only` wiring they are filtered
   from the alphabet *before* their `real_editor_enabled()` precondition can apply
   (`type_chars.rs:105`, `press_key.rs:33`). Decide: should "edit content" require
   `AnyStorageOf({Loro, Turso})` (ADR 0007 §disjunction; the disjunction variant
   exists, `wiring.rs:255`) so the on-blur `set_field` path is exercised as a
   *transition* under Turso-only, not only via raw driver keystrokes? If yes, a
   `required_wiring()` change makes the editor path bisectable across the storage
   axis.

2. **Which `ReactiveEngine` the shared invariants observe.** Already largely
   resolved — there is one shared engine instance (`ensure_reactive_engine`). The
   remaining decision: when `Actor::UI` is present, the GPUI window's engine must be
   the *same instance* the ViewModel invariants observe (not a parallel one), so
   `full_gpui ⊋ full_headless` is a genuine strengthening. Document and assert
   instance identity.

## Migration / build sequence

Incremental; each step is independently valuable and leaves the suite green.

1. **Derive `Subsystem` from `ComponentSet`.** Replace the runtime
   `frontend_geometry.is_some()` branch with `set.subsystems()`. Add `Actor::UI` to
   the GPUI harness's set.
   - **1a (acceptance gate, do not skip):** capture the *selected invariant set per
     blessed slice* (`full`, `sql_only`, `loro_backend`, `org_create`, `gpui`)
     before and after, and assert they are **identical**. This is the only proof
     that the total `subsystems()` mapping is behaviour-preserving — the first draft
     asserted "no behaviour change" while silently dropping four subsystems.
   - **Status (2026-06-09): LANDED.** The total `subsystems(&ComponentSet)` mapping
     lives in `invariants/registry.rs` (9-variant complete). The runner rewire is done:
     `run_invariant_registry` (`invariant_runner.rs`) now builds a `ComponentSet` from
     `ref_state.wiring` (with `Actor::UI` set from the *runtime* `frontend_geometry`
     fact, since today's `Wiring::full()` carries UI even headless) + the always-present
     ViewModel/EditorState projections, then selects via `subsystems(&set)` — replacing
     the `frontend_geometry.is_some()` branch. 1a met at two levels: the unit anchor
     (`subsystems_reproduce_blessed_slice_selection`: `full_headless == headless_wide`,
     `full_gpui == all`) **and** runtime — `loro_backend_pbt`, `org_create_ordering_pbt_full`,
     `general_e2e_pbt_sql_only` all green through the rewired selection (the `full`
     slice exceeded nextest's 600s terminate-after and is being re-confirmed under a
     plain `cargo test` with no per-test timeout). The behaviour-preservation argument
     for scoped headless sets (loro/org dropping Turso/Cdc subsystems) holds because
     those invariants are wiring-gated out anyway — the gate is *derived* from `min_sut`
     (`required_wiring_for_subsystems`), so a dropped subsystem can never drop an
     invariant that would otherwise have run.
2. **Lift the engine signature.** Extract `run_component_pbt(set, src)` from the
   current slice macro + phased loop; route runner choice through
   `needs_real_window()`; define `SeedSource` (§2). Express today's blessed slices as
   one-line `component_pbt!` declarations.
   - **Status (2026-06-09): headless landed; GPUI scope corrected.** `pbt/stepper.rs`
     provides the shared `run_sequence` engine (`proptest-state-machine`'s
     `test_sequential` factored over a `Stepper` seam) + `ReplayMode`/`StepOutcome`.
     Both slice macros (`__declare_pbt_slice_wrapper!`, `__declare_pbt_full_slice!`)
     override `StateMachineTest::test_sequential` to route their per-case loop through
     it via the generic `SmtStepper<T>` adapter — every blessed headless slice now
     shares one loop body with no behaviour change (`loro_backend_pbt`,
     `org_create_ordering_pbt_full` green). Added a value-level
     `E2ETransition::required_wiring()` for replay-time gating.
   - **Status (2026-06-09): one-line `component_pbt!` LANDED.** `slice.rs` now exports
     `component_pbt!`, a thin sugar that lowers a `ComponentSet` to its `.wiring` and
     delegates to `declare_pbt_slice!` (native full-coverage *and* explicit-slice
     forms). `loro_backend_pbt` is now expressed as the one-liner
     `component_pbt! { set: ComponentSet::loro_vm_fast(), … }` (behaviour-identical —
     `loro_vm_fast().wiring == Wiring::loro_backend()`; re-run green 13s). The
     projection axis is intentionally not threaded into a single slice's selection
     (the runner observes both projections at runtime); it matters only to which
     lattice node a *bisection* oracle builds — so `set → set.wiring` loses nothing a
     standalone slice could express.
   - **GPUI finding (corrected from the first draft).** Reading the real harness
     (`phased.rs::run_pbt_with_driver_sync_callback`) overturned the assumption that
     the live GPUI *generator* should collapse into `run_sequence`. It should not:
     (a) the window is launched *mid-sequence* (between the pre- and post-startup
     loops); (b) generation is incremental and `PROPTEST_SEED`-reproducible —
     pre-building a `Vec` would change the seed→sequence mapping; (c) each
     post-startup step wraps apply+check in real-input gestures
     (`driver.try_ui_interaction`), a CDC sync wait, per-step screenshots, and a
     `catch_unwind`-with-screenshot. The sketched "`MainThreadBridge`" was also wrong:
     the driver lives *inside* the SUT and already owns the window round-trip. The
     live generator already shares the per-step *primitives* with headless
     (`drive_transition`/`apply_transition_async`/`run_invariant_registry`); its loop
     shell stays GPUI-specific by design.
   - **Where the seam DOES fit GPUI: replay.** A fixed `Vec` (a JSON capture or a
     bisection candidate) driven through an *already-launched* window has none of the
     above obstacles. `GpuiReplayStepper<'a>` (borrows the live SUT + driver) makes
     `run_sequence(.., SkipGated)` the GPUI replay/bisection path — the same capture
     replays across a `ComponentSet` lattice (headless nodes via `SmtStepper`, the UI
     node via `GpuiReplayStepper`). Verified: GPUI binary builds + runs here (real
     window; reaches StartApp, launches, drives post-startup steps); the headless
     refactor is non-regressive to it (the seed-12345 failure is the pre-existing
     click-to-focus stealback, not this change).
3. **Cross-set replay semantics (the load-bearing step).** The JSON capture
   artifact already exists (`tests/.captures/*.captured.json`, `replay_steps`); the
   work is making it portable across `ComponentSet`s.
   - Add a `ReplayMode::SkipGated` to `replay_steps()` alongside today's strict mode:
     a transition gated out by the node's wiring becomes a recorded
     `SkippedByGating` step instead of the current hard panic (`fixtures/mod.rs:167`).
     Assert a `SkippedByGating` step never changes ref state.
   - Extend GPUI failure handling to write `tests/.captures/*.json` (net-new item 3).
   - **3a (spike, blocks bisection):** take an existing headless capture, replay it
     under `SkipGated` against every valid subset of its set, and assert only skips
     appear — never a *new* failure class. This is the §4 property; verify it
     empirically before building step 4.
   - **Status (2026-06-09): SkipGated path + §3a spike LANDED.** `stepper::run_sequence`
     already routes `SkipGated` (a gated transition `continue`s without advancing the
     reference state); the spike now *proves* it empirically. `tests/bisection_pbt.rs`
     (always-on, no SUT via the new `stepper::NullStepper`):
     `skip_gated_replay_is_portable_for_committed_capture` replays the committed
     `loro-content-drop-set-field` capture across `full_headless` ∪ `valid_children()`
     and asserts the engine's applied sequence equals exactly each node's *applicable*
     subsequence (computed independently — applicability is a pure function of
     `(transition, fixed wiring)`); `editor_transition_skips_purely_under_storeless_node`
     drives an editor `PressKey` against an Org-only node (neither Loro nor Turso) and
     asserts it is `SkippedByGating` and never reaches `apply`. Equality is over the
     *applied transition sequence* (logical, order-stable), **not** a whole-`ReferenceState`
     `Debug` — the latter embeds the interpreter's hash-ordered builder list and is
     unstable across instances; since reference `apply` is pure, an identical applied
     sequence implies an identical resulting state.
   - **Status (2026-06-09): GPUI capture-write LANDED (net-new item 3 — step 3
     complete).** `run_pbt_with_driver_sync_callback` now arms the slice wrapper's
     thread-local capture (`reset_capture`) and a `CaptureOnPanic` `Drop` guard that
     writes `tests/.captures/<name>.captured.json` (name via `HOLON_PBT_CAPTURE_NAME`,
     default `gpui_ui_pbt`) when the phased GPUI loop unwinds; the pre/post loops call
     `record_transition` before each apply. A UI-observed failure is now a capture the
     headless lattice bisects. Verified: a forced fault (`HOLON_PBT_FORCE_FAIL_AT_STEP`,
     a deterministic on-demand capture generator) panicked the GPUI run at step 7 and
     wrote a valid 7-transition `Fixture` JSON.
4. **Bisection driver.** Thin lattice walk over the engine, **bidirectional**
   (§3: `DownwardMinimal` / `UpwardMinimal` / `Combination`). CLI/env entry to bisect
   a given `RecordedSequence`. Wire GPUI-failure capture → headless lattice replay.
   - **Status (2026-06-09): pure lattice search landed.** `holon_pbt_core::bisect`
     (`bisect.rs`): `Localization{NotReproduced, DownwardMinimal, UpwardMinimal}` +
     `bisect_downward` (greedy ddmin over `valid_children`), `bisect_upward` (greedy
     over `valid_parents_within` — the absent-component direction), and `bisect`
     (routes downward if the ceiling reproduces, else upward). The node-oracle is a
     `FnMut(&ComponentSet) -> bool` closure, so the search is fully unit-tested
     against synthetic oracles (combination bug → downward minimal; absent-component
     bug → upward minimal) with **no SUT**.
   - **Status (2026-06-09): SUT-backed oracle + CLI/env entry LANDED.** The thin
     adapter is `stepper::BisectionStepper` (builds an `E2ESut` per node via
     `E2ESut::new_with_backend(storage_selector_for_wiring(node.wiring))` — unlike
     `SmtStepper<E2ESut>`, which is Turso-only because `E2ESut::init_test` hard-wires
     `StorageSelector::Turso`) plus `pbt::bisect_driver`: `reproduces_under(set, caps)`
     replays the capture through `run_sequence(.., SkipGated)` inside a
     `catch_unwind` (panic hook muted/restored) and reports whether it still fails;
     `bisect_capture(ceiling, caps)` computes the floor as the ceiling's minimal valid
     descendant and calls `holon_pbt_core::bisect`. The CLI/env entry is the
     `bisect_capture_from_env` test (`HOLON_BISECT_CAPTURE` + optional
     `HOLON_BISECT_CEILING`; no-op when unset, since each node builds a real SUT).
     Verified end-to-end: bisecting the committed `loro-content-drop-set-field` capture
     under `full_headless` builds the ceiling + floor SUTs, replays, and reports
     `NotReproduced` in ~22s (correct — that bug is fixed).
   - **Status (2026-06-09): CI triage entry LANDED; oracle hardened against
     replay-infidelity false positives.** New env affordances on
     `bisect_capture_from_env`: `HOLON_BISECT_PROBE=1` (cheap single-node "does the
     ceiling reproduce?"), `HOLON_BISECT_SLICE=<slice>` (resolve the capture from a
     slice name — the CI triage entry), `HOLON_BISECT_SIGNATURE` (pin the exact
     failure message), `HOLON_BISECT_REPEAT` (flakiness), `HOLON_BISECT_VERBOSE`
     (print the panic). `scripts/pbt-bisect.sh <slice> [--probe]` wraps it.
   - **CORRECTION + oracle fix (2026-06-09).** An earlier note here claimed
     `general_e2e_pbt.captured.json` localized to `DownwardMinimal({Loro})` — **that
     was an artifact** of an `any-panic ⇒ reproduces` oracle. The capture does **not**
     replay faithfully headlessly: a Turso-generated sequence aborts at *every* node
     with **harness/settle errors, not invariant divergences** — `SplitBlock`'s
     Turso-only `probe_block_sql_state` diagnostic crashing `test_ctx()` ("App not
     started") on a no-Turso node, and a `send_raw_keystroke` settle-timing abort
     ("create intent hasn't landed in the Loro tree") even at `full_headless`. Counting
     those as reproduction made *any* no-Turso node "reproduce" and the walk descend
     spuriously to `{Loro}`. **Fix:** `reproduces_under` now counts a panic as a
     reproduction **iff its message contains the reproduction signature** — by default
     the cross-layer `trouble begins at:` marker (`format_layer_report`, present iff an
     invariant actually diverged), overridable via `HOLON_BISECT_SIGNATURE`. Anything
     else is logged as an *inconclusive node* (replay-infidelity), not a reproduction —
     so the downward walk never localizes into a node it cannot faithfully replay.
     Verified: `general_e2e` now reports `reproduces = false` at both `loro_vm_fast`
     (App-not-started abort) and `full_headless` (send_raw_keystroke abort), each
     logged as inconclusive. **Net:** the oracle is honest; a confirmed end-to-end
     `DownwardMinimal` on a *faithfully-replayable* failing capture is still pending
     (the captures on hand either replay-abort or are fixed bugs — and gpui captures
     are "runner-coupled", reproducing only on the real window).
5. **Resolve the two asymmetries** (editor `required_wiring`, ViewModel engine
   instance identity) with their own focused PBTs.
   - **Status (2026-06-09): asymmetry #1 (editor `required_wiring`) RESOLVED.**
     `TypeChars` and `PressKey` now gate on `RequiredWiring::any_storage_of([Loro,
     Turso])` instead of `HasStorage(Loro)`, so "edit content" is structurally
     available under Turso-only wiring (where the on-blur `set_field` path persists it)
     — making the editor path bisectable across the storage axis. **Behaviour-preserving
     for headless:** the transitions' `preconditions` still require `enable_loro() ||
     real_editor_enabled()`, so a headless Turso-only slice (no real editor) rejects
     them dynamically exactly as the old structural gate did; the change only *adds*
     them to the alphabet of the real-editor GPUI no-Loro path. Focused test
     `editor_transitions_gate_on_any_of_loro_or_turso` pins the new gate; registry
     self-tests + the headless slices stay green. Asymmetry #2 (ViewModel engine
     instance identity under UI) is still open — already narrowed by the single shared
     `ReactiveEngine` (`ensure_reactive_engine`); wants an explicit instance-identity
     assertion when `Actor::UI` is present.
6. **`ComponentSet::validate()`** extends ADR 0007's `Wiring::validate()` with the
   UI rule (`UI ⟹ ≥1 storage + ViewModel`) and the validity-lattice PBT (ADR 0007
   open question #2): blessed sets valid; rule-violating sets rejected. The bisector
   walks only valid nodes (§3).
   - **Status (2026-06-09): landed.** `holon_pbt_core::ComponentSet` (`component_set.rs`):
     `{wiring: Wiring, projections: BTreeSet<Projection>}`, the typed `Component` enum
     + `of()` lowering (ADR §0), blessed presets (`full_gpui`/`full_headless`/
     `loro_vm_fast` + slice equivalents), `validate()` (+ `ComponentSetError`),
     `needs_real_window()`, `is_subset_of()`, and the `valid_children()` lattice walk
     (validity-pruned — dropping `ViewModel` under `UI` is not offered). 7 unit tests
     green. **Remaining for bisection:** `valid_parents()` for the upward (absent-
     component) direction (§3), and the lattice-walk driver itself.

## Known weaknesses / open questions

1. **GPUI runner can't be the proptest *macro*** — but the reason is **SUT
   re-instantiation under shrinking, not threading** (`proptest-state-machine` is a
   thread-agnostic synchronous loop; the main-thread need is GPUI's window). So the
   two backends *can* share the sequential-loop engine via a `Stepper` seam (§5); the
   only irreducibly-divergent part is the proptest *shrinker*, which re-builds the
   SUT/window per shrink step (impractical for a process-singleton `Application`). UI
   relies on replaying captures shrunk by the headless runner plus component
   bisection. UI sets have no `.proptest-regressions` file — their reproducibility is
   `ReplayCapture`. *Possible future:* a window pool / reset-between-cases could make
   even shrinking viable on UI, but that is out of scope here.
2. **Display / CI feasibility of UI bisection.** Each UI lattice node needs a real
   DISPLAY and runs for hundreds of seconds; a multi-UI-node walk may exceed CI
   budget or not run headless-CI at all. Mitigation: bisect the *cheapest* axis first
   (drop UI before a backend) — but this fails for UI-*only* bugs, where every
   reproducing node is a UI node. Treat UI bisection as a local/developer capability,
   not a CI default, until measured.
3. **The reference model is assumed correct; in this codebase it frequently is
   not.** Most recent PBT wins were reference-fidelity gaps (drawer-closed render,
   split focus-handoff, click-to-focus stealback, org-roundtrip normalization), not
   prod bugs. Bisection names *where* ref and projection disagree but must **not**
   assert which is wrong; the report wording (§3) is load-bearing for not
   mis-blaming a correct component.
4. **Bidirectional monotonicity.** With both `DownwardMinimal` and `UpwardMinimal`
   probes, a pathological bug that is non-monotone in *both* directions (present only
   in a scattered, non-convex region of the lattice) is still possible. The bisector
   reports "smallest reproducing set found, monotonicity: none" honestly rather than
   guaranteeing minimality.
5. **Validity-pruned lattice.** Because `validate()` forbids some subsets
   (`UI ⟹ ViewModel`), the minimal *valid* reproducing set can be coarser than the
   true minimal cause. The report distinguishes "minimal valid set" from "minimal
   cause."

## References

- ADR 0007 — `Wiring` manifest, `RequiredWiring` (incl. `AnyStorageOf`), validity,
  blessed-vs-valid. This ADR extends it: UI into the set, `Subsystem` derived from
  the set, concrete-sequence bisection.
- ADR 0004 / 0006 — tiers and actor naming the set names.
- ADR 0005 — children-as-ordered-list; ordering authority when multiple storage
  adapters are wired (ADR 0007 open question #3) applies unchanged.
