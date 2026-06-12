# Contributing to the PBT γ Composition

How to extend the composed PBT slices — step by step, copy-paste ready. Pairs with
the design (`PbtCompositionDesign.md`) and the task list (`PbtCompositionBacklog.md`).

**Mental model (one line):** a slice is a *component list*; the framework runs the
**one shared catalog** of invariants through `run_selected` against that slice's
caps and keeps the subset whose `Needs` are satisfied. You almost never wire an
invariant "into a slice" — you add it to the catalog once and it lights up in every
slice whose components provide its caps.

Two kinds of contribution:
- **🤖 Add an invariant** (Recipe 1) — mechanical, parallel-safe, one new file + one
  catalog line. This is the bulk of the work.
- **🧠 Add a component / cap** (Recipes 2–3) — needs a judgment call; unlocks a batch
  of 🤖 invariant-adds.

Paths below are relative to `crates/holon-integration-tests/src/pbt/`.

---

## Recipe 1 — Add an invariant to the catalog (🤖)

### Step 1 — Find the body and read its bounds

Invariant *bodies* live in `invariants/bodies/*.rs`, generic over `R`/`S` with cap
bounds. The bounds **are** the spec for what you need:

```rust
// invariants/bodies/no_parent_cycles.rs
impl<R, S> Invariant<R, S> for InvNoParentCycles
where
    S: SutBackend,          // ← one SUT cap, no R bound (ignores the reference)
{ ... }
```

So: `Sut*` bounds → SUT caps; `Ref*` bounds → Ref caps. That's all the `Needs`
needs. (If the body you want doesn't exist yet, authoring it is a separate, larger
task — out of scope for a 🤖 ticket.)

### Step 2 — Confirm the caps are hostable

A cap can be put on a `CapMap` only if its trait carries `#[holon_macros::capmap_adapter]`
(in `holon-pbt-core/src/capabilities.rs`) **and** some component in your target slice
provides it. Check both:

```bash
# Is the cap hostable at all?
grep -B1 "pub trait SutBackend" crates/holon-pbt-core/src/capabilities.rs   # look for the attribute
```

If a required cap is **not** hosted, or **no component provides it**, your ticket is
blocked — it becomes a 🧠 task (Recipe 2/3). See `PbtCompositionBacklog.md` for which
caps need which component. Don't fake a cap to get unblocked (see Anti-patterns).

### Step 3 — Create `composed/invariants/<name>.rs`

Copy the nearest exemplar by *shape*:

| Body shape | Copy |
|---|---|
| one SUT cap, ignores ref | `no_parent_cycles.rs` |
| SUT + `RefBackend` | `no_orphan.rs` |
| SUT + `RefBlockTree` | `block_parent.rs` |
| editor (`SutEditorMirrorRead` + `RefEditorMirror`) | `editor_caret.rs` |

The `wire()` is the whole production surface. Build `Needs` straight from the bounds:

```rust
use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefBlockTree, SutBackend};   // ← the body's bounds
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::block_parent_matches_ref_backend::InvBlockParentMatchesRefBackend;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvBlockParentMatchesRefBackend,          // the body (a unit struct)
        RunMode::Strict,                          // Strict unless the body documents Warn
        Needs {
            sut_present: vec![CapId::of::<dyn SutBackend>()],     // every Sut* bound
            sut_absent:  Vec::new(),                             // [] unless a degraded twin
            ref_present: vec![CapId::of::<dyn RefBlockTree>()],  // every Ref* bound
        },
    ))
}
```

**The Needs rule (do not deviate):** `sut_present`/`ref_present` mirror the body's
`where` bounds exactly. `sut_absent` is empty **except** for a degraded-mode twin,
where it lists the cap whose *absence* selects the twin (e.g. `dyn SutQueryResults`).
Needs is a property of the *body*, not of any slice — that's why it lives in the
shared catalog.

### Step 4 — Write the test triad

In the same file, under `#[cfg(test)] mod tests`, using the shared doubles
(`composed::fixtures::*` gives you `uri`, `fixture_slice`, `ref_map`,
`editor_ref_map`, `buggy_editor_map`, `Block`, `EntityUri`, `run_selected`,
`CapMap`, `composed_invariant_catalog`). Write all three:

**(a) Positive** — caps wired ⇒ selected and passes:
```rust
assert!(report.ran_ids().contains(&"inv-...-id"), "must be selected; ran={:?}", report.ran_ids());
assert!(report.failures().is_empty(), "must pass on valid input: {:?}", report.failures());
```

**(b) Negative containment** — required cap absent ⇒ *deselected*, not silently
passing. Build a ref/SUT map missing the cap and assert:
```rust
assert!(report.deselected.iter().any(|id| id.0 == "inv-...-id"),
        "must be deselected without the cap; deselected={:?}", report.deselected);
```

**(c) Catch** — inject a divergence the *real* API can't produce (that's what the
hand-crafted `FixtureBackend`/`BuggyEditor` are for) and assert it's caught:
```rust
let failures = report.failures();
assert!(failures.iter().any(|(id, _)| *id == "inv-...-id"),
        "the divergence must be caught; failures={failures:?}");
```
If the catch can collide with other invariants, add an **isolation** check that the
unrelated ids stay green (see `block_parent.rs` for the pattern).

### Step 5 — Register it

One line in `composed/catalog.rs`:
```rust
invariants::<name>::wire(),
```
Add the `pub mod <name>;` line in `composed/invariants.rs` if your file is new.

### Step 6 — Gate

```bash
cargo test -p holon-integration-tests --features pbt --lib <name>
```
Green = done. Then run the slice's full set to confirm no selection drift:
```bash
cargo test -p holon-integration-tests --features pbt --lib memory_slice
```

---

## Recipe 2 — Add a component / slice (🧠)

A component is a `CapProvider` that contributes one or more caps. This is where the
judgment lives — **before writing code, answer the honesty gate**:

> *Does this component wrap real production logic, or is it a re-implementation that
> would only test itself?* If the latter, stop — it produces vacuous green.

Good: `MemoryBackendComponent` wraps the real `holon::api::MemoryBackend`;
`InMemEditorComponent` runs the *same byte-offset caret math* as production's
`headless_editor_mirror.rs`. A component that re-derives the thing it checks against
the ref is worthless.

Then:
1. **Provide the cap** — implement the cap trait, and `CapProvider::register` that
   inserts `self as Arc<dyn ThatCap>`. If the component is also *driven* in the apply
   phase (an editor, a store you mutate), use interior mutability (`Mutex`) so the one
   `Arc` is both write-driven via `&self` and hosted as the read cap (§4.4).
   Exemplar: `memory_slice/components.rs`.
2. **Add a builder** in the slice's `builders.rs` (`Config::new().with(...).build()`,
   or hand-build a `CapMap` if you need to keep an `Arc` handle to drive writes — see
   `memory_wide_with_editor`).
3. **Selection tests** — assert which catalog invariants now light up over the new
   component (the payoff: existing SUT-cap invariants run for free). Exemplar:
   `integration_tests::memory_slice_runs_ref_comparison_when_ref_is_wired`.
4. The invariants the new cap unlocks are now **🤖 tickets** (Recipe 1).

A whole new *slice* (e.g. Loro-only) is just: a new `pbt/<name>_slice.rs` thin root +
`components.rs` + `builders.rs` + selection tests. **Do not copy the catalog** — call
`composed_invariant_catalog()`. (For a *mutation*-sequence proptest, the design's
intent is to drive the concrete SUT through the write caps / `SutTransitionTarget` and
reuse `E2ETransition` + `ReferenceState` — the F2 convergence — not to hand-roll a
per-slice op-loop.)

---

## Recipe 3 — Host a new cap (🧠)

To put a cap trait on `CapMap`, add `#[holon_macros::capmap_adapter]` above it in
`holon-pbt-core/src/capabilities.rs`:
- **Sync trait** (owned returns, or a borrowing `Option<&str>`/`&T` return) → no
  `#[async_trait]` is emitted; existing concrete impls are untouched. Borrow-returning
  methods forward through `CapMap::expect_ref` automatically — no special handling.
- **Async trait** → the macro emits `#[async_trait(?Send)]`; every concrete impl must
  also carry it (the read caps already do).
- **`&mut self` methods** become a fail-loud `unimplemented!` on `CapMap` (they're
  apply-phase ops driven on the concrete SUT, never through the shared map). That's
  intended — don't try to route a drain through the map.

Object-safety: methods with a default body are skipped by the adapter; generic or
`where Self: Sized` methods aren't supported.

---

## Recipe 4 — Use the real `ReferenceState` as the ref `CapMap` (🧠)

Since 2026-06-16 `ReferenceState` implements `CapProvider` (Design §8.8), so the
**single real oracle** can be the ref side of `run_selected` — no bespoke parallel
ref model:

```rust
let ref_caps = reference_state_ref_caps(Arc::new(my_reference_state));
let report = run_selected(&composed_invariant_catalog(), &sut_caps, &ref_caps).await;
```

`reference_state_ref_caps` (in `reference_capabilities.rs`) registers the read caps
the catalog consumes (`RefBackend` + `RefBlockTree` + `RefEditorMirror`) — the same
surface `FixtureRef` + `FixtureEditorRef` expose, so selection is unchanged. Prefer
this over `FixtureRef` for any new slice or generated PBT: the fixtures
(`FixtureRef`/`FixtureEditorRef`/`EditorModel`/`EditorPureRef`) are the parallel
models §5/§6 are retiring, and migrating slices onto `reference_state_ref_caps` is
exactly that retirement (Backlog F6.1 → F2).

Two rules carry over from the `subsystem_shrink.rs` integration:
- **Selection is SUT-gated.** `ReferenceState` provides *all* its `Ref*` caps, but
  `Needs::selected_against` is an AND over the SUT *and* ref cap sets, so an invariant
  still deselects when the SUT lacks the paired cap. Registering the full ref surface
  is therefore safe, not scope creep.
- **Mutate the ref through caps, not the committing apply.** For a mirror-only editor
  oracle, call `RefEditorMirrorMut::{type_chars,…}` directly — *not*
  `type_chars_apply_to_ref`, which commits typed text into block content (correct only
  when the SUT also persists). `current_focus(Main)` reads `navigation_history`, so
  seed focus by pushing a nav-history entry (as `NavigateFocus` does), not just
  `set_focus`.

---

## Recipe 5 — Author a transition's SUT dispatch with `cap_transition!` (🤖)

A transition file hand-writes a struct + `TransitionFactory` + `TransitionRef` +
`impl<S: Cap> TransitionImpl`. The **cap-coupled part** — the `TransitionImpl` block and
`TransitionFactory::required_caps` — now goes through `cap_transition!` (in
`transition_dispatch.rs`), which single-sources the cap so the two can't drift (Design §8.9).

Replace the hand-written `impl<S: Cap> TransitionImpl<ReferenceState, S>` block:

```rust
// single cap
cap_transition! {
    SplitBlock: SutBlockTreeWrite,
    |me, _state, sut| { sut.apply_split_block(&me.block_id, me.position).await; }
}
// no cap (full SutHandle bundle; required_caps stays empty)
cap_transition! { Nothing, |_me, _state, _sut| {} }
```

Then point the `TransitionFactory::required_caps` body at the macro's generated helper:

```rust
fn required_caps() -> Vec<CapId> { Self::declared_caps() }   // single-cap form only
```

Rules:
- The body names cap **handles** (`me`, `state`, `sut`) and calls `sut.<cap-method>(…)`. Write
  it the same way regardless of whether dispatch is generic-`S` (today) or `&mut CapMap` (later)
  — that identity is what makes the macro a migration seam, so don't reach for `sut.expect::<…>()`.
- A transition authored this way needs **no** entry in
  `required_caps_match_transition_impl_bounds` — delete its line from the guard (the cap is
  stated once, in the macro). The guard test is retired once every transition is migrated.
- Multi-cap transitions don't exist (the per-cap-bound dispatch is exactly one cap per
  variant). If you think you need two, you're probably splitting one transition into two.

**Anti-pattern:** restating the cap in both the `TransitionImpl` bound and a hand-written
`required_caps()` (the drift `cap_transition!` exists to remove), or hand-writing a
`#[allow(async_fn_in_trait)] impl<S: Cap> TransitionImpl` block for a new transition instead of
using the macro.

---

## Definition of done

- The gate test(s) are green; `cargo test … --lib memory_slice` (the slice) green.
- **No new warnings** (`cargo test` output is clean for your files).
- **The catch test actually has teeth** — if you're unsure, temporarily break the
  body or fixture and confirm the catch *fails*, then revert. A catch that can't fail
  is worse than no test.
- The triad is complete (positive + negative-containment + catch). A missing
  negative-containment test is the most common gap — it's what proves selection isn't
  silently faking a pass.

---

## Anti-patterns (these fail review)

- **Vacuous invariant** — always passes (e.g. compares two empty sets because the
  slice models neither side). If the data can't diverge in this slice, don't wire it
  here; note it in the backlog instead.
- **Faked cap** — returning `None`/empty/`Ok(0)` from a cap method to dodge a check
  instead of *disclosing* absence. Caps must answer honestly; absence is expressed by
  *not providing the cap* (→ the invariant deselects), not by a lying impl.
- **Redundant invariant** — duplicates coverage an existing one already has (e.g.
  `block_ids_match_ref` vs `blocks_match`'s id-set equality). Check before wiring.
- **Dead-code ref cap** — hosting a `Ref*` cap whose paired SUT cap no component
  provides, so nothing selects. Host caps only alongside the component that uses them.
- **Per-slice invariant copy** — re-listing an invariant in a slice instead of the
  shared catalog. The catalog is the single source; slices contribute only components.
- **Reinventing the write-cap mechanism** — a bespoke per-slice mutation driver trait
  instead of driving the concrete SUT through `SutBlockTreeWrite`/`SutTransitionTarget`
  and reusing `E2ETransition` + `ReferenceState` (the F2 convergence). Mutation is a
  write *cap*, not a new abstraction.
- **Treating the shrinker as the oracle** — a config-generated slice (Design §8.7,
  `subsystem_shrink.rs`) still owes the *same* honesty gate as any slice: the shared
  catalog **and** a differential oracle (real SUT vs independent reference). The
  subsystem-config shrinker only tells you *which subsystems are causally necessary*
  for a failure it was already given — it does **not** detect the failure for you, and
  it does **not** replace the catch triad or the ref comparison. A slice whose only
  "check" is "the shrinker minimized something" has no teeth.
- **Minting a `ComposedSut` slice to unblock an E3 cap deletion** — when E3 needs to
  delete an `E2ESut` cap impl and the blocker is a standalone test consuming that cap,
  the fix is to **delete the standalone test** (the ONE PBT, `general_e2e_composed_pbt`/
  `WideE2E`, already covers the cap when `full_headless` provides it) — *not* to rewrite
  it as a new per-cap composed slice. A fresh slice adds a deletion obligation the North
  Star will have to pay later; it is *negative* progress even when green. Promote the
  invariant into `WIDE_REQUIRED_INVARIANTS` if it must be guaranteed-exercised, relocate
  any unique real-SUT teeth into `composed/invariants/<name>.rs`, and delete. The only
  time a new composed slice is right is a cap `WideE2E` cannot yet drive (E4/GPUI/windowed
  input). Full rule + worked example: `PbtCompositionDesign.md` §8.10.
