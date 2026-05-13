# Property-Based Testing Strategy

This document captures the testing vision for holon and how the current code base maps onto it. It is descriptive of intent and prescriptive about direction; the current state lags the target in specific, named ways.

## Why PBT-first

Most unit tests sit somewhere between useless and harmful:

- They give false confidence — a green suite proves only the cases the author imagined.
- They slow down refactoring — every internal seam they pin is a tax on every rename.
- They miss the bugs that actually hurt: integration between components, ordering races, state-machine reachability, edge inputs the author never considered.
- The single property they reliably enforce — "this module is callable in isolation" — can be obtained from architecture lint rules and module boundaries instead.

Property-based tests address all of these when written *as PBTs*, not as parametrized unit tests:

- A single test method takes generated input that covers as much of the input space as feasible.
- It runs as long as feasible (cases + per-step budget).
- It checks as many invariants as it can on every step, not one assertion per test.
- Shrinking and replay infrastructure turns failures into minimal reproducers.

The PBT is the unit of test investment. Everything else is a supplement to cover what PBTs structurally can't.

## Reusable, orthogonal components

A PBT decomposes into four reusable parts:

1. **Generators** — strategies that produce inputs (transitions, blocks, docs, keystrokes, queries).
2. **Reference state** — a simple, hand-written model of "what should be true" after a sequence of operations. The reference state is a source of *hypotheses* about production, not pre-baked truth.
3. **SUT abstractions** — traits that hide the differences between deployment shapes. `UserDriver` is the canonical one: it lets the same harness run against the headless reactive engine, a real GPUI window, or a TUI session.
4. **Invariants** — assertions that hold over every reachable state of the system, irrespective of which transitions got us there.

The win is multiplicative: every new PBT entry point picks a subset of generators, plugs in one driver, and chooses which invariants to enable. Adding a new transition or invariant lights up every PBT at once.

## Current state

### Shared toolkit (in place)

The toolkit crate is `crates/holon-pbt-core/` (the home for what was previously discussed as a "holon-pbt-toolkit"). The harness modules currently live in `crates/holon-integration-tests/src/pbt/`:

- `state_machine.rs`, `phased.rs`, `transition_dispatch.rs` — the harness loop.
- `reference_state.rs` — `ReferenceState` + `apply_to_ref` pattern.
- `sut.rs` (~5900 lines) — invariants (`check_invariants_async`, inv10_live, inv16, …).
- `transitions/` — ~52 transition modules, each carrying its own `weighted_generator` and `apply_to_ref`.

Over time the reusable pieces (generators, reference state, invariants, driver trait surface) should land in `holon-pbt-core` so every PBT can depend on it directly without pulling the integration-tests crate.

`UserDriver` is declared in `crates/holon-frontend/src/user_driver.rs`. Implementations:

- `ReactiveEngineDriver` — headless, integration-tests crate.
- `GpuiUserDriver` — `frontends/gpui/src/user_driver.rs`.
- `TuiUserDriver` — `frontends/tui/src/user_driver.rs`.

### Wide PBTs sharing the toolkit

| File | Driver | Slice |
|---|---|---|
| `crates/holon-integration-tests/tests/general_e2e_pbt.rs` | `ReactiveEngineDriver` | Full stack: Loro + Turso + OrgFile + frontend logic (no real UI) |
| `frontends/gpui/tests/gpui_ui_pbt.rs` | `GpuiUserDriver` | Same harness inside a real GPUI window with `BoundsRegistry` + xcap screenshots |
| `frontends/tui/tests/tui_ui_pbt.rs` | `TuiUserDriver` | Same harness inside the TUI frontend |
| `crates/holon-integration-tests/tests/cross_frontend_pbt.rs` | Multiple | Cross-frontend convergence |

### Narrow PBTs (today, **not** sharing the toolkit)

These exist but each invented its own generators and (where applicable) reference state. Folding them onto `holon-pbt-core` is part of the target end state.

- `frontends/gpui/tests/layout_pbt.rs` — pure layout/render oracle, `TestAppContext` direct.
- `crates/holon-integration-tests/tests/loro_sync_controller_pbt.rs`
- `crates/holon-engine/tests/pbt.rs`
- `crates/holon/tests/turso_block_round_trip_pbt.rs`
- `crates/holon/tests/petri_e2e_pbt.rs`
- `crates/holon-orgmode/tests/{round_trip,org_block_round_trip,sync_controller_mutation}_pbt.rs`
- `crates/holon-org-format/tests/inline_marks_proptest.rs`
- `crates/holon/src/api/{sync_pbt,loro_backend_pbt}.rs`, `storage/turso_pbt_tests.rs`, `storage/turso_ivm_bug_proptest.rs`
- `crates/holon-todoist/src/pbt_test.rs`

### Unit-test mass (rough `#[test]` counts)

- `crates/holon` — ~470
- `crates/holon-frontend` — ~222
- `crates/holon-org-format` — ~72
- `crates/holon-core` — ~47
- `frontends/gpui` — ~44
- `crates/holon-markdown` — ~39

A large fraction of these pin internal seams whose contracts are covered combinatorially by PBTs. They are candidates for deletion as the narrow-PBT layer fills in.

## Target end state

### Promote the toolkit

`holon-pbt-core` is the home for the reusable pieces. Target module shape:

- `generators` — every transition's generator is reusable in isolation, gated by feature so each PBT picks only what it needs.
- `reference_state` — composable; subsystem PBTs use slim subsets.
- `invariants` — named, individually enableable.
- `drivers` — `UserDriver` impls (or the trait surface they implement).

Existing narrow PBTs migrate onto `holon-pbt-core` so generators don't drift. The integration-tests crate keeps the wide entry points and any toolkit pieces that need the full stack to compile, but the core library should stand alone.

### Five-tier pyramid by speed

| Tier | Latency | What lives here | Purpose |
|---|---|---|---|
| T0 | ms | Boundary/parser proptest in each crate (Unicode, BOM, CRLF, malformed SQL, …) | Pin pathological inputs PBT generators rarely synthesize |
| T1 | seconds | Narrow PBTs sharing `holon-pbt-core` (see below) | Daily-driver discovery; failures name the subsystem |
| T2 | minutes | `general_e2e_pbt` (headless full stack) | Integration bumper |
| T3 | slow | `gpui_ui_pbt`, `tui_ui_pbt`, `cross_frontend_pbt` (real UI) | UI/render correctness, cross-frontend convergence |
| T4 | offline replay | `turso-sql-replay` + `crates/holon/sql/regressions/*.sql` | Frozen regression gates for upstream bugs |

### Six narrow PBTs to add (T1)

These cover bug classes that today only surface after minutes in `general_e2e_pbt`.

1. **BlockCellRegistry routing PBT** — generate cell-write sequences (content/parent/tags/sort_key/marks) against an in-memory Loro + SQL pair, assert projection convergence + EventOrigin routing.
2. **SqlOperationProvider + event-bus PBT** — generate operation streams, assert SQL state + `Event::routing_doc_uri` round-trip + inbound-runtime gate behaviour. No Loro, no OrgFile.
3. **Block-tree org round-trip PBT** — extend `inline_marks_proptest` to full block trees; render → parse → assert structural identity.
4. **editor-view-model / MutableTree PBT** — generate keystroke + chord sequences against `headless_editor_mirror` + `InputState`; assert text and cursor invariants. Pure logic, microseconds per step.
5. **Render-DSL / view-model resolution PBT** — generate Block trees + queries, assert resolved ViewModel structure against a reference computation. No engine, no Loro, no Turso.
6. **Reactive-engine-only PBT** — strip `ReactiveEngineDriver` to engine + in-memory store. Catches dispatch/transition wiring bugs without the storage stack.

### Unit-test deletion policy

As T1 fills in, delete unit tests in `holon` and `holon-frontend` whose contract is fully covered by a narrow PBT. Keep unit tests only for:

- Pure parser boundary cases (Unicode, BOM, CRLF, malformed input) — these belong at T0.
- archlint-style structural rules.
- Fixed-input regression repros that are also documentation.

Default to delete on refactor; do not preserve a unit test just because it currently passes.

## Trade-offs

### Pros

- Faster signal per failure: a T1 PBT shrinks in seconds and names the subsystem.
- Single source of truth for "what an operation does" (ReferenceState) — refactor-safe.
- Combinatorial coverage beats hand-enumerated unit cases for the bug classes that actually occur (CDC races, Loro/SQL drift, chord-op sequencing).
- Generator and invariant reuse keeps the marginal cost of a new PBT low.

### What we'd miss / cons

- **Specific-input regression pinning.** When you fix a Turso `json_group_array` bug, you want a `.sql` file that runs in <1s on every CI and never flakes. PBTs are too noisy for that. T4 SQL replay tests fill this gap and are *complementary*, not substitutes — keep investing in `crates/holon/sql/regressions/`.
- **Pathological boundary inputs.** Generators rarely hit weird Unicode, BOM, CRLF, control chars, or malformed input unless explicitly taught to. T0 unit-level proptest with hand-crafted strategies covers this cheaply. "No unit tests" is not dogma — T0 is targeted and small.
- **Performance regressions.** PBTs check correctness, not latency or memory. A real benchmark suite (none today) is a separate workstream.
- **Visual snapshots.** `layout_insta` catches things that dump-equivalence PBTs don't — human eyeball on rendered output catches a class of regressions invariants can't express. Keep snapshots at T3.
- **Onboarding cost.** Adding a feature now means a transition + reference-state update + (maybe) an invariant. The shared toolkit reduces total LOC but raises the local-to-global ratio. Worth it, but real.
- **Debuggability tail.** A failing PBT pays a shrink + replay tax. The existing investment (`replay.jsonl`, `panic_diag`, `turso-sql-replay`) is load-bearing. The toolkit's value is bounded by how cheap shrink-to-repro stays — invest in it deliberately.
- **Type-level guarantees + archlint already do work** that some unit tests duplicate. Lean on them before writing a runtime test.

## Working principles

- **PBT model assertions are production hypotheses.** Every `apply_to_ref` line is a claim about prod. Pre-baking unverified ones turns the PBT into a rubber stamp. Workflow: strip the reference state to verified facts → run → read panics as "prod says X, your model said Y" → add the minimum aligned with verified prod.
- **Parse, don't validate.** Generators produce typed values; the SUT consumes typed values. Strings carrying domain meaning across boundaries are a smell.
- **Fail loud at the boundary.** Generators that silently coerce invalid inputs hide bugs the PBT was supposed to find.
- **Narrow before wide.** When a wide PBT shrinks to a small repro, the next step is often "this repro should be a narrow PBT" — promote it.
- **Delete on refactor.** Tests that pin internal seams are tax. The toolkit lets you replace them with one combinatorial test against the trait boundary.

## Pointers

- Toolkit crate: `crates/holon-pbt-core/`
- Harness modules (to migrate into the toolkit): `crates/holon-integration-tests/src/pbt/`
- UserDriver trait: `crates/holon-frontend/src/user_driver.rs`
- Driver impls: `frontends/{gpui,tui}/src/user_driver.rs`, `crates/holon-integration-tests/src/pbt/sut.rs` + `phased.rs`
- Invariants: `crates/holon-integration-tests/src/pbt/sut.rs::check_invariants_async`
- SQL regression replay: `crates/holon/sql/regressions/`, `turso-sql-replay` binary
- Architecture lint: `crates/holon-architecture-tests/tests/architecture_rules.rs` (a thin wrapper over `archlint`)
