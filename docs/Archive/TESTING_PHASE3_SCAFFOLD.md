# Phase 3.1 scaffold — invariant registry landed (metadata-only)

## What landed

- `crates/holon-integration-tests/src/pbt/invariants/{mod,registry}.rs` — registry types + the canonical 25-invariant manifest derived from `docs/TESTING_INVARIANT_AUDIT.md`.
- Seven unit tests in `pbt::invariants::registry::tests` — **all pass.**
- `pbt/mod.rs` gains `pub mod invariants;`.
- **No changes to `check_invariants_async`.** Bodies still live inline.

## The shape

```rust
pub enum Subsystem { BlockTree, Loro, TursoProjection, Cdc,
                     ViewModel, Renderer, EditorState,
                     FrontendBounds, Driver }

pub enum RunMode { Strict, Warn }

pub struct InvariantSpec {
    pub id: InvariantId,               // matches `[inv-…]` labels
    pub description: &'static str,
    pub min_sut: BTreeSet<Subsystem>,
    pub mode: RunMode,
}

pub struct InvariantRegistry { … }      // owns Vec<InvariantSpec>

pub struct PbtSuiteSpec {
    pub name: &'static str,
    pub subsystems: BTreeSet<Subsystem>,
}

impl PbtSuiteSpec {
    pub fn select<'a>(&self, registry: &'a InvariantRegistry)
        -> Vec<&'a InvariantSpec>;
}

pub fn register_default() -> InvariantRegistry;
```

A PBT entry point describes the subsystems its SUT supplies; `select` returns the invariants whose `min_sut ⊆ subsystems`. The scaffold makes the Phase 0/C audit *executable* — drift between the audit doc and the registry now causes test failure, not silent rot.

## Tests as guardrails

| Test | What it pins |
|---|---|
| `registry_size_matches_audit` | Exactly 25 invariants registered. |
| `gpui_wide_pbt_selects_all` | All-subsystems spec picks up every invariant. |
| `headless_wide_pbt_drops_frontend_bounds_invariants` | Headless spec drops exactly 5 `FrontendBounds`-touching invariants (matches audit). |
| `phase5_editor_loro_picks_up_expected_count` | Phase 5 candidate spec picks up 10–12 invariants (matches audit's 11). |
| `under_scoped_spec_rejects_multi_subsystem` | Negative test: ViewModel-only spec rejects every multi-subsystem invariant. |
| `warn_mode_invariants_preserved` | The 3 `Warn`-mode invariants (`inv-backend-blocks-match-ref`, `inv-watch-rows-match-ref`, `inv-focus-roots`) are preserved. CDC-lag tolerance is non-negotiable. |
| `every_invariant_has_a_non_empty_min_sut` | Sanity: no zero-subsystem invariants. |

## What this is *not* yet

- **Not wired into the actual check path.** `check_invariants_async` still runs its inline assertions verbatim. Phase 3.2+ migrates invariant bodies one at a time into closures registered against the manifest.
- **Not a runtime dispatch mechanism.** The registry stores metadata only. Bodies need a `BoxFuture`-shaped closure (or async-trait-based context) added in Phase 3.2.
- **Not consumed by any T1 PBT yet.** Phases 4/5/6 will declare their own `PbtSuiteSpec` instances; the scaffold is ready for them.

## Phase 3 — remaining sub-phases

- **3.2 — Body migration shape.** Define a `BoxFuture`-shaped `InvariantBody`, or an async-trait `InvariantCtx` the SUT implements. Pick one and add a single migrated invariant (`inv-loro-no-errors` is the obvious first — pure sync, no SUT-shape baggage) as the proof. The wide PBT's `check_invariants_async` should still pass; the registry's body for the migrated id is what executes when the body is non-null, the inline assertion fires otherwise.
- **3.3 — Bulk migration.** Move bodies one by one until `check_invariants_async` is just a dispatch loop over the registry. ~5000 LOC of body movement across ~25 invariants.
- **3.4 — Consumer adoption.** T1 PBTs (Phases 4+) get a `PbtSuiteSpec` and dispatch through the registry instead of calling `check_invariants_async`.

## Trade-offs locked in

- **`InvariantId` is a `&'static str`.** Stable, log-greppable, identical to the `[inv-…]` prefix already in use. Cost: invariant additions need a literal string each. Worth it for grep-friendliness.
- **Metadata-first.** This phase ships *no* runtime behavioural change. The audit is now load-bearing — but `check_invariants_async` still owns execution. Optionality preserved.
- **`Warn` mode is explicit.** A migration that "tightens" a Warn invariant to Strict will fail `warn_mode_invariants_preserved`. Re-introducing CDC-lag flakes is now a deliberate decision someone has to argue for in code review, not an accidental cleanup.

## How to add a new invariant

When you grow a new `[inv-…]` label in `sut.rs`:

1. Add an `InvariantSpec` line to `register_default()` in `pbt/invariants/registry.rs`.
2. Bump `registry_size_matches_audit`'s expected count.
3. If new, decide and document the min-SUT set in `docs/TESTING_INVARIANT_AUDIT.md`.
4. If it's a multi-subsystem invariant predicted by one of the T1 PBT-count tests (`phase5_editor_loro_picks_up_expected_count`), adjust the expected range.

## Test runtime

`cargo nextest run -p holon-integration-tests --features pbt --lib invariants` → **7 passed, ~14 ms total.** Cheap enough to keep as a permanent guardrail.
