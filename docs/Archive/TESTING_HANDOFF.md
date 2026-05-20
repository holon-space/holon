# Testing-Strategy Work — Session Handoff (2026-05-17)

Resume here in a fresh session. The plan is at `/Users/martin/.claude/plans/luminous-baking-marble.md` (v4); all detail docs are under `docs/TESTING_*.md`. Worktree: `.claude/worktrees/docs-pbt-strategy` (branch already isolated).

## TL;DR

- Plan locked in (v4) — phased migration from unit-test heavy → fast narrow PBTs sharing `holon-pbt-core` traits.
- **Phases 0, 1, 2, 3.1, 4 are done.** Phase 5 is the next concrete code work.
- Two phases (`Phase 4` and `Phase 0/D`) turned out cheaper than expected because relevant work already existed in the repo. The remaining phases are real and need real code.

## What's done

| Phase | Status | Key artifact |
|---|---|---|
| 0/A — Baseline | ✅ | `docs/TESTING_BASELINE.md` |
| 0/B — Historical-bug back-test | ✅ | `docs/TESTING_BACKTEST.md` — 92% (clean+borderline) of MEMORY.md holon-side bugs have a T1 catcher under this plan |
| 0/C — Invariant audit | ✅ | `docs/TESTING_INVARIANT_AUDIT.md` — 25 invariants tagged by min-SUT set; 8% need 3+ subsystems (gate passes) |
| 0/D — CI-budget spike | ✅ | Subsumed by Phase 4 measurement (6.33s / 1024 cases). See `TESTING_PHASE4_CLOSEOUT.md`. |
| 1 — First fold | ✅ | `docs/TESTING_PHASE1_FINDINGS.md` — `CollapseToggle` → shared `holon_pbt_core::ToggleCollapse`; zero trait-surface changes needed |
| 2 — Pattern doc + sequel survey | ✅ | `docs/TESTING_PATTERNS.md` + `docs/TESTING_NEXT_FOLD_SURVEY.md` |
| 3.1 — Invariant registry scaffold | ✅ | `crates/holon-integration-tests/src/pbt/invariants/{mod,registry}.rs` + `docs/TESTING_PHASE3_SCAFFOLD.md`. 7 guardrail tests pass in ~14 ms. |
| 4 — Block-tree org round-trip T1 | ✅ | `crates/holon-orgmode/tests/org_block_round_trip_pbt.rs` (pre-existing — bumped cases 50 → 1024, 6.33s wall). See `TESTING_PHASE4_CLOSEOUT.md`. |

## What's next, in priority order

### Phase 5 T0 — pure MutableTree proptest (recommended next)

Smallest scope of the remaining T1 candidates; validates the "microsecond-fast pure-logic PBT" claim Phase 5 rests on.

- **Where**: `crates/holon-frontend/src/editor_view_model.rs` — add `#[cfg(test)] mod proptest`.
- **SUT**: just `MutableTree` + `InputState`. No Loro, no SQL, no UI.
- **Generators**: dense keystroke + cursor-move + structural-op streams. Re-use *only* the relevant transitions from `crates/holon-integration-tests/src/pbt/transitions/` if pure-logic compatible (`TypeChars`, `DeleteBackward`, `MoveCursor*`, `SplitBlock`, `JoinBlock`, `Indent`, `Outdent`) — don't import the wide-PBT harness, lift just what's needed.
- **Invariants**: cursor monotonicity, text-content trim, tree-structural integrity, no panics on adversarial inputs. **New `inv-tree-cursor-*` family** — register in `pbt/invariants/registry.rs` before writing the bodies. The current `register_default()` only covers the 25 wide-PBT invariants; this T0 needs its own min-SUT set: `{BlockTree, EditorState}`.
- **Budget**: target <5s wall.
- **Watch out**: don't accidentally pull in Loro or Turso — the value proposition is microsecond-per-case coverage of the cursor/text state machine the seconds-scale Phase 5 T1 PBT can't economically reach.

### Phase 5 T1 — editor + Loro PBT (after T0)

- **Where**: `crates/holon-frontend/tests/editor_loro_pbt.rs` (new).
- **SUT**: `headless_editor_mirror` + `InputState` + `MutableTree` + **in-memory Loro doc** + `BlockCellRegistry` (at `crates/holon-core/src/cell_registry.rs`) routing. **No Turso, no frontend window, no OrgFile.**
- **Invariants** the registry already predicts will fire (per `TESTING_INVARIANT_AUDIT.md` line "Phase 5 T1 component"): 11 invariants. **Body migration needed first** — Phase 3.2 (see below) must land before Phase 5 T1 dispatches through the registry.
- **Budget**: target <90s wall.
- **Watch out**: the back-test (`TESTING_BACKTEST.md`) lists this as catching 4 historical bugs — verify each is reproducible by replaying its captured op log before claiming exit-criterion met.

### Phase 3.2 — invariant body migration (interleaves with Phase 5+)

The Phase 3.1 scaffold registered the 25 invariants as *metadata only*. Bodies still live inline in `sut.rs::check_invariants_async`. Phase 3.2 begins migrating bodies into closures. Two open design questions:

1. **Body shape**: `BoxFuture` closure (`Fn(&dyn InvariantCtx, &ReferenceState) -> BoxFuture<'_, InvariantResult>`) vs. an `async_trait`-based `InvariantBody` trait. The async-trait crate is already in the workspace; either works. Pick the one that's cheapest to call from a `for inv in spec.select(&reg)` loop inside `check_invariants_async`.
2. **`InvariantCtx` shape**: small trait the wide-PBT `E2ESut<V>` and (eventually) the T1 PBT SUTs implement. Initial methods, minimal: `loro_sync_error_count(&self) -> usize`, then grow per-invariant as bodies migrate.

**Suggested first body migration**: `inv-loro-no-errors` (in `sut.rs:4058-4072`). Pure sync; min-SUT is just `{Loro}`; no surrounding context needed. The migration validates the closure shape; subsequent bodies follow the same pattern.

### Phase 6 — BlockCellRegistry routing PBT

- **Where**: `crates/holon-integration-tests/tests/block_cell_registry_pbt.rs` (new).
- **SUT**: `BlockCellRegistry` (at `crates/holon-core/src/cell_registry.rs`) + in-memory Loro + Turso pair via `SqlBlockOperations` + `LoroSyncController`. No frontend rendering.
- **Critical**: matview invariants gated on the open upstream Turso `json_group_array multiset went negative` bug. Land scaffold + non-matview invariants with the matview invariant `#[ignore]`'d; un-ignore when `cargo update -p turso` brings in the fix.
- **Back-test risk concentration** (`TESTING_BACKTEST.md`): 3 of 4 borderline historical bugs need Phase 6's invariant catalogue to explicitly cover (a) constructor wiring, (b) CDC delete shape via `block_tags`, (c) tag-default deserialization. If Phase 6 ships with only Loro↔SQL convergence + EventOrigin routing, the back-test rate drops from 92% to 67%, below the gate.

### Phase 7 — SqlOperationProvider + event-bus

**Demote.** Per `TESTING_BACKTEST.md`: zero MEMORY.md historical bugs would have been caught here. Plan order in v4 still lists Phase 7, but practical advice is to defer until a real bug demands it.

### Phases 8–12 — fold remaining narrow PBTs, prune unit tests, CI tiering, docs

Detail in the plan file.

## Key landed code (since this work started)

```
M crates/holon-integration-tests/src/pbt/transitions/mod.rs        ← rename in fold
A crates/holon-integration-tests/src/pbt/transitions/toggle_collapse.rs ← Phase 1 fold
D crates/holon-integration-tests/src/pbt/transitions/collapse_toggle.rs ← deleted in fold
M crates/holon-integration-tests/src/pbt/reference_state.rs        ← doc-comment rename
M crates/holon-integration-tests/src/pbt/mod.rs                    ← exposes invariants module
A crates/holon-integration-tests/src/pbt/invariants/mod.rs         ← Phase 3.1 scaffold
A crates/holon-integration-tests/src/pbt/invariants/registry.rs    ← Phase 3.1 scaffold (+ 7 tests)
M crates/holon-orgmode/tests/org_block_round_trip_pbt.rs           ← Phase 4 case-count bump
```

(Note: a linter/post-edit hook materially improved the Phase 1 fold's `apply_to_sut` after my initial write — it now delegates fully to the shared `holon-layout-testing` impl via `SutClickAdapter` + `LayoutSut`. The doc `TESTING_PHASE1_FINDINGS.md` says `apply_to_sut` stayed local; the current code is more aggressive. Reconcile when next touching that file.)

## Key decisions to keep

1. **Read-only ref state in shared crates.** `LayoutRef` is `&'a R` by design; shared `apply_to_ref` impls are empty; consumers own ref-state mutation locally. Do **not** add a `LayoutRefMut` — add a separate small capability trait if mutation must be shared. (Phase 1 finding F1.)
2. **Strict ≥2-consumer leaf-crate gate.** `holon-layout-testing` is the precedent, not the template. No new leaf crates until a PR can prove two in-tree consumers.
3. **Warn-mode invariants preserved.** `inv-backend-blocks-match-ref`, `inv-watch-rows-match-ref`, `inv-focus-roots` downgrade to log lines under CDC-lag conditions. A migration that quietly promotes them to Strict re-introduces flakes the WARN path was added to handle — the `warn_mode_invariants_preserved` test will fail CI.
4. **Editor state machine gets both T0 and T1 coverage.** Don't collapse them into one. T0 catches the pure-logic class; T1 catches the editor↔Loro interaction class.
5. **Phase 7 is dormant unless a real bug demands it.** Zero historical demand; don't speculatively build it.

## Open follow-ups

- The Phase 1 fold's `apply_to_sut` was post-edited to delegate to the shared path via `SutClickAdapter::click_at_element` → `SutHandle::apply_click_at_element`, whose default impl panics. `ToggleCollapse` is dormant (no fixture corpus produces `expand_toggle` blocks), so the panic landmine is asleep. When the corpus grows toggles, implement `apply_click_at_element` on `E2ESut`, `GpuiUserDriver`, `TuiUserDriver` *before* the test runs. (See `docs/TESTING_PATTERNS.md` finding F2.)
- `crates/holon/tests/turso_block_round_trip_pbt.rs` (212 LOC, 30 cases) is a Phase-6-adjacent T1 PBT that already exists on the shared generator surface. Acknowledge in the plan; no fold needed.
- The "deliberately-introduced bug shrinks to ≤10 blocks" Phase 4 exit criterion was *not* enforced (deliberate — testing the framework). Document in `TESTING_STRATEGY.md` Phase 12 that shrink-quality is a framework concern, not a PBT concern.

## How to verify the current state

```bash
# Registry guardrails (~14 ms):
cargo nextest run -p holon-integration-tests --features pbt --lib invariants

# Phase 4 PBT (~6.3s wall):
cargo nextest run -p holon-orgmode --test org_block_round_trip_pbt

# Phase 1 fold sanity (compiles + arch tests):
cargo check -p holon-integration-tests --features pbt --tests
cargo nextest run -p holon-integration-tests --features pbt --lib transitions::arch_tests
```

All four should pass green. If they don't, the worktree drifted — read the diff against `main` first.

## Suggested first action in fresh session

1. Read `docs/TESTING_HANDOFF.md` (this file).
2. Run the four verifier commands above to confirm baseline.
3. Read `docs/TESTING_INVARIANT_AUDIT.md` for the SUT-subsystem vocabulary.
4. Open `crates/holon-frontend/src/editor_view_model.rs` and start Phase 5 T0.

## Plan file

- `/Users/martin/.claude/plans/luminous-baking-marble.md` — current source of truth for phasing. v4 (3 feedback edits applied) + revised Phase 2 (per Phase 1 findings).
