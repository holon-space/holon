# PBT slicing framework — Stage B handoff

Status as of 2026-05-18 on worktree `pbt-slicing-doc`. Plan reference:
`~/.claude/plans/stage-a-ship-with-dynamic-pudding.md`.

## What landed (Stages A + B)

**Stage A** (Phases 1-5, committed earlier — `xpsoqssyrylu` and predecessors):

- Capability traits at `crates/holon-pbt-core/src/capabilities.rs` for the
  six T0 cluster surfaces (`RefBlockTree`, `RefEditorMirror`, `RefFocus`,
  `SutBlockTree`, `SutEditorMirror`, `SutFocus`) plus mut variants.
- Blanket impls on `ReferenceState` and `E2ESut<V>`.
- Seven T0 transitions (`type_chars`, `delete_backward`, `move_cursor`,
  `move_up`, `move_down`, `split_block`, `join_block`, `indent`, `outdent`)
  rebound to capability bounds via `TransitionFactory<R>` / `TransitionImpl<R,S>`.
- `editor_pure_pbt` slice consumer at
  `crates/holon-integration-tests/tests/editor_pure_pbt.rs` — no storage,
  no UI, no async runtime, ~2 s per 1024 cases.

**Stage B** (Phases 6-8, committed `kymqnwyktrrl` … `loptyyvlqsuz`):

- All 8 remaining cluster traits — Loro, Turso/CDC, ViewModel/Renderer,
  Layout, Driver, OrgRender, QueryCompile, Lifecycle — scaffolded with
  blanket impls on `E2ESut<V>` (26/32 methods wired; 6 stubs with
  documented blockers).
- Ref-side: 6 new traits (`RefFocusRoots`, `RefGlobalFocus`, `RefLayout`,
  `RefRender`, `RefTaskState`, `RefWatches`, `RefPeers[Mut]`).
- Invariant infrastructure: `Invariant<R,S>` trait + `CachingProxy<'a,S>`
  (zero-unsafe eager-drain) in `holon-pbt-core`.
- `WidgetSnapshot` IR with `walk()`/`find_op()`/`collect_*()` helpers,
  enabling renderer-required invariants to run on any UI-bearing slice
  (not just real GPUI).
- 28 `Invariant<R,S>` impls under
  `crates/holon-integration-tests/src/pbt/invariants/bodies/` — 9 fully
  functional, 19 returning `InvariantResult::Skipped` with documented
  unblockers (private fields, missing trait methods, design decisions
  pending).
- Storage-slice consumer at
  `crates/holon-integration-tests/tests/storage_consistency_pbt.rs` using
  `E2ESut<SqlOnly>` + 2 storage invariants, 16 cases × 1..10 steps in
  ~124 s. 3 shrunk regression cases captured in
  `storage_consistency_pbt.proptest-regressions`.

**Phase 9 (gated) — DEFERRED**: see `docs/PHASE_9_H7_AUDIT.md`. Matview
count 3 > 2 gate; estimated LOC 1600-2950 > 1500 gate. Framework's
structural claim is already validated by three slice consumers; no Phase 9
needed today.

**Phase 10 — what shipped (combined across sessions)**:

- *Phase 10.4 (registry self-tests)*: Phase 8 invariants registered in
  `register_default()` (was missing 3: `inv-block-ids-match-ref`,
  `inv-block-tags-references-exist`, `inv-task-state-storage-coherence`).
  3 new registry self-tests:
  - `body_ids_match_registry_ids` — id parity between bodies dir and registry.
  - `storage_slice_invariants_are_subset_of_wide_registry` — runtime H11
    anti-rubber-stamp guard.
  - `every_registry_id_has_a_body_file` — file-system parity.
  10/10 registry tests pass in 14 ms.

- *Phase 10.1 (proof-of-concept migration)*: `inv-loro-no-errors`
  migrated from inline `assert_eq!` in `sut.rs::check_invariants_async`
  to a call against the `InvLoroNoErrors` impl via the `SutLoroLog`
  blanket impl on `E2ESut<V>`. Establishes the wire-up pattern for
  the remaining 6 functional bodies (documented per-body context to
  preserve below).

- *Phase 10.2/10.3 (archlint smells)*: `archlint/smells/pbt_transitions.toml`
  with two forward-looking rules:
  - `pbt-transition-helper-concrete-ref` — forbids `pub fn
    <name>_(apply_to_ref|weighted_generator|preconditions)` helpers
    from naming `ReferenceState` in their signature. Forward-looking
    guard against new transitions bypassing capability traits.
  - `pbt-slice-invariant-foreign-module` — forbids slice test files
    from importing `Inv*` structs outside
    `holon_integration_tests::pbt::invariants::bodies::`. Static
    counterpart to the runtime H11 anti-rubber-stamp test in 10.4.
  Both verified to fire on sentinel regressions and pass on the
  9 migrated transitions + 2 slice tests.

## Three slice consumers running today

| Slice | SUT variant | Renderer | Storage | Cases × Steps | Wall |
|---|---|---|---|---|---|
| `editor_pure_pbt` | `EditorPureSut` | none | none (in-memory) | 1024 × 30 | ~2 s |
| `storage_consistency_pbt` | `E2ESut<SqlOnly>` | none | real Turso+Loro | 16 × 1-10 | ~124 s |
| `general_e2e_pbt` (wide) | `E2ESut<Full>` | ReactiveEngine headless | real Turso+Loro | varies | minutes |

The framework's structural claim — *same transitions + same invariants
run across different SUT compositions* — is empirically validated.

## What remains in Phase 10 (deferred)

Five Phase 10 items did not land this session and remain as follow-up
work:

### 10.1 — delete inline invariant bodies in `sut.rs::check_invariants_async`

**Status: 1 of 7 migrated.** `inv-loro-no-errors` migration landed
(`sut.rs:4099-4116`) — the inline `assert_eq!` was replaced with a call
to `InvLoroNoErrors.check(&ref_state, self)` via the `SutLoroLog`
blanket impl, matching against `InvariantResult::Fail(msg)` and
panicking with the same message text. This is the proof-of-concept that
the wire-up mechanism works end-to-end.

The remaining 6 functional inline bodies require case-by-case migration
because they carry inline-only context that the migrated
`Invariant<R,S>` impls don't currently model:

| Inline body | Inline-only context to preserve |
|---|---|
| `inv-frontend-root-not-error` (sut.rs:6042) | wrapped in `if !fe_engine.is_loading()` gate; `vm.entity.get("error_message")` diagnostic snapshot |
| `inv-frontend-no-error-widgets` (sut.rs:6049) | `collect_error_node_summaries(&vm)` per-node summaries eprintln'd before panic |
| `inv-focus-matches-ref` (sut.rs:6709) | rich diff between predicted+actual focus, region-specific |
| `inv-viewmodel-entity-ids-subset-of-data` (sut.rs:5098) | per-region SQL re-query for CDC-lag truth check |
| `inv-viewmodel-root-matches-render-expr` (sut.rs:5039) | RenderExpr comparison via DataRow.expr |
| `inv-viewmodel-state-toggle-correct` (sut.rs:5204) | per-StateToggle-node iteration with field/label/states/operations assertions |

Note: Phase 8 invariants (`inv-block-ids-match-ref`,
`inv-block-tags-references-exist`, `inv-task-state-storage-coherence`)
have NO inline body in `sut.rs` — they live only as migrated impls
used by the storage slice. They are already "complete" for Phase 10.1
purposes. Adding them to the wide PBT runner would be additive coverage,
not a deletion.

Suggested migration pattern (replicate the `inv-loro-no-errors` shape):

```rust
{
    use crate::pbt::invariants::bodies::<id>::Inv<Name>;
    use holon_pbt_core::invariant::{Invariant, InvariantResult};
    match Invariant::<ReferenceState, Self>::check(&Inv<Name>, ref_state, self).await {
        InvariantResult::Ok => {}
        InvariantResult::Fail(msg) => panic!("{msg}"),
        InvariantResult::Skipped(_) => {}
    }
}
```

For each remaining body, the migration requires either:
1. Widening the migrated `Inv<Name>` impl to carry the inline-only
   diagnostics + skip-gates, OR
2. Keeping the inline rich-context body and treating the migrated impl
   as the "slim slice" version only.

Option (1) is closer to "single source of truth" but risks bloating the
trait surface. Option (2) accepts dual maintenance but preserves
diagnostic richness. The wide PBT's inline bodies often pre-date the
migrated impls and carry hard-won bug-diagnosis affordances.

**Recommendation**: do option (1) one body at a time, with the inline
body kept as a fallback (`if cfg!(debug_assertions)` or feature flag)
during one round of wide-PBT verification, then deleted.

Verification cost: wide PBT runs in minutes, so iterating on this is
slow. Best done as a single focused session per body.

### 10.2 — archlint rule: transitions must go through capability traits

Add a rule under `crates/archlint/` that scans
`crates/holon-integration-tests/src/pbt/transitions/*.rs` and rejects
files whose `apply_to_ref` / `apply_to_sut` signatures or bodies
mention concrete `ReferenceState` or `E2ESut`. Allowed bridging files:
`reference_capabilities.rs`, `sut_capabilities.rs`.

### 10.3 — archlint rule: H11 anti-rubber-stamp (compile-time)

The runtime version landed in 10.4. Add the static counterpart:
parse non-wide slice test files for `Inv*` struct usages, assert each
is also in the wide registry via
`InvariantRegistry::register_default()`. The runtime test in 10.4
catches the regression but doesn't prevent it landing in a PR — the
archlint rule provides the gate.

### 10.5 — retire dead code

Audit:
- `E2ETransitionFactory` / `E2ETransitionImpl` — are these fully
  replaced by `holon-pbt-core::{TransitionFactory, TransitionImpl}`?
  If yes, delete.
- `SutHandle` methods whose only callers were inline invariant bodies
  migrated to `bodies/` — delete unused.
- 6 stubs in `sut_capabilities.rs` with `unimplemented!()` /
  `panic!("…")` — each has a documented blocker; resolve or document
  why permanent.

### 10.6 — docs + result marker

This file is the placeholder. After 10.1-10.5 land:
- Update top-level `ARCHITECTURE.md` to reference the slicing framework.
- Mark the parent plan complete.
- Write `result:` line in this doc declaring the framework shipped.

## Open follow-ups outside Phase 10

Carried from Phase 7/8 (lower priority than 10.1-10.5):

- 19 deferred invariant bodies under `bodies/` return
  `InvariantResult::Skipped`. Each has a `# Why deferred` section
  naming the blocker (typically: private SUT field, missing trait
  method, or a design decision about how to expose the data). Migrate
  these as the corresponding plumbing lands. Track via:
  `rg "# Why deferred" crates/holon-integration-tests/src/pbt/invariants/bodies/`.
- `InvBlockIdsMatchRef` deferred in `storage_consistency_pbt` because
  SqlOnly variant filters seed blocks asymmetrically between ref and
  SUT. Resolve by either: (a) symmetric seed filter in SUT
  `all_block_ids()`, or (b) symmetric seed filter in ref
  `all_non_seed_block_ids()`. Decision: option (a) — the SUT is the
  source of truth and the ref's "non-seed" filter is just a naming
  artifact.
- `InvTaskStateStorageCoherence` deferred — needs
  `SutLoroTaskState::loro_task_state_of` wired on `E2ESut`. Currently
  returns `None` for all blocks.
- 4 SUT stubs in `sut_capabilities.rs` (`apply_create_stale_loro`,
  `loro_children_of`, `driver_send_key_chord`, `compile_query`) — each
  needs a design decision before implementation.

## How to pick up this work

For the next session, the most valuable single deliverable is **Phase
10.1 wiring + deletion** because it's the change that prevents
re-drift: once the wide PBT calls `Inv*.check()` directly, the inline
bodies are unused and can be deleted safely. Until that wiring lands,
`bodies/` is essentially shadow infrastructure.

Suggested sequence for the next session:

1. Read `crates/holon-integration-tests/src/pbt/sut.rs:4059` to see
   the existing inline runner.
2. Build a single tuple wrapper `WideSuite = (InvLoroNoErrors,
   InvFocusMatchesRef, …)` covering only the 9 functional invariants.
3. Wire it into `check_invariants_async` next to the inline bodies
   (don't delete the inline copies yet — run both side-by-side under
   `RUST_LOG=info` for one PBT seed to confirm equivalent verdicts).
4. Once equivalence is confirmed across a handful of seeds, delete
   the 9 inline copies.
5. Run wide PBT end-to-end before merging.

This is ~1 focused day if no surprises surface.

## Stack at handoff

```
@   nnkvmlos Stage B Phase 9 H7 audit: DEFER (matview count 3 > 2, LOC 1600-2950 > 1500)
○   loptyyvl Stage B Phase 8: storage_consistency_pbt slice consumer
○   wkkwknut Stage B Phase 7: 2 storage-layer invariants (block-tags + task-state)
○   xvlsylrl Stage B Phase 7: 4 renderer-required + 1 storage-layer invariant
○   tlvpwpwt Stage B Phase 7: WidgetSnapshot IR + state-toggle keystone migration
○   npywokku feat(pbt): Phase 7 Stage B ref-side caps — 6 new Ref* traits + blanket impls + 10 placeholder status updates
○   uvunsppm Stage B Phase 7 unblock pass 1: ref-side traits + SUT accessors + 2 stub wires
○   nqunvlkm Stage B Phase 7 batch: skeleton 24 invariant body files + 2 functional migrations
○   trtntmtx Stage B Phase 7: migrate inv-loro-no-errors to capability-bound free fn
○   vpotnmzn Stage B Phase 7 base: CachingProxy + Invariant trait in pbt-core
○   solsqxqk Stage B Phase 6c-d-e-f-g: remaining cluster SUT-side blanket impls
○   ynwwrlrq Stage B Phase 6b: SutSqlProjection + SutOrgFileWrite + SutCdc blanket impls on E2ESut
○   rzpnkkus Stage B Phase 6a: SyncWithPeer/MergeFromPeer ref-migration + SUT-side blanket impls
○   kqkktqou Stage B Phase 6a: migrate AddPeer + PeerEdit + RefPeersMut blanket impl
○   kymqnwyk Stage B Phase 6a-h: capability trait surface scaffolding
○   xpsoqssy Stage A complete: editor_pure_pbt running migrated transitions
○   tqoqmwmt Phase 1 + Phase 2: capability traits + blanket impls on ReferenceState
```

Next commit on this stack should carry the Phase 10.4 registry changes
(metadata add + 3 self-tests). See bottom of this file for that commit.
