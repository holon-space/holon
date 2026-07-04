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
| `crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs` | `ReactiveEngineDriver` | Full stack: Loro + Turso + OrgFile + frontend logic (no real UI) |
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
| --- | --- | --- | --- |
| T0 | ms | Boundary/parser proptest in each crate (Unicode, BOM, CRLF, malformed SQL, …) | Pin pathological inputs PBT generators rarely synthesize |
| T1 | seconds | Narrow PBTs sharing `holon-pbt-core` (see below) | Daily-driver discovery; failures name the subsystem |
| T2 | minutes | `general_e2e_composed_pbt` (headless full stack) | Integration bumper |
| T3 | slow | `gpui_ui_pbt`, `tui_ui_pbt`, `cross_frontend_pbt` (real UI) | UI/render correctness, cross-frontend convergence |
| T4 | offline replay | `turso-sql-replay` + `crates/holon-turso/sql/regressions/*.sql` | Frozen regression gates for upstream bugs |

### Six narrow PBTs to add (T1)

These cover bug classes that today only surface after minutes in `general_e2e_composed_pbt`.

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

- **Specific-input regression pinning.** When you fix a Turso `json_group_array` bug, you want a `.sql` file that runs in <1s on every CI and never flakes. PBTs are too noisy for that. T4 SQL replay tests fill this gap and are *complementary*, not substitutes — keep investing in `crates/holon-turso/sql/regressions/`.
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
- SQL regression replay: `crates/holon-turso/sql/regressions/`, `turso-sql-replay` binary
- Architecture lint: `crates/holon-architecture-tests/tests/architecture_rules.rs` (a thin wrapper over `archlint`)

## The wiring grid — drawn, not fixed (2026-07-07)

**Why.** A quality audit of 21 manual-bug escapes classified 12 as ENVIRONMENT: prod
assemblies the tests never ran (worst case: the dioxus-web worker wired
`EventInfraModule` alone → SILENT loss of every content write; also an empty op
registry and CRDT-config-only latency). Modularity with deactivatable subsystems is
net-quality-positive only while the keystone actually draws the wiring grid. It does,
since Roadmap Round 3c: `WideE2EMachine::init_state` draws `any_valid_wiring()` — a
wiring bug is now either drawn-and-tested, rejected loudly at composition time, or a
conscious omission from `wiring_axes()`.

**The typed grid.** `holon-pbt-core::wiring::Wiring` = three axes (`storage_adapters`,
`sync_adapters`, `actors`) as `BTreeSet`s; validity is `Wiring::validate()` (≥1 storage
adapter; `MCPServer` ⇒ storage; `ActionEngine` ⇒ a query-capable adapter). "Valid grid"
is a typed, enumerable notion (parse-don’t-validate), not folklore. Blessed CI manifests:
`Wiring::blessed_manifests()`.

**The draw.** `wiring_axes()` defaults to storage `{Loro, Org, Turso}`, sync `{}`,
actors `{MCPServer, ActionEngine}` (no `Actor::UI` — the windowed gpui harness is the
sibling; no `Markdown`/`GCal`/`GMail`). Turso is included with probability 0.20
(`QUERY_ADAPTER_INCLUSION_PROB`) so most cases stay on the cheap LoroMemory backend;
shrinking removes components, i.e. walks DOWN the lattice toward Loro-only.
`set_for_wiring(&Wiring) -> ComponentSet` normalizes a draw into the bootable headless
set; `cap_set_for_wiring` extracts the composed cap set; `aggregate_transitions`
auto-narrows the transition alphabet to it; `WideE2E::required_invariants` is the
per-draw non-vacuity floor. Each case prints `[wide-e2e wiring] drawn: ...` to stderr,
so a run log yields per-wiring case counts.

**Run controls.**

- `PROPTEST_CASES` (test default 16; `just pbt general <cases>`)
- `HOLON_PBT_FORCE_FULL=1` — pin every case to `full_headless` (deterministic exerciser
  for the frontend-only arms). Also the pin to use when replaying seeds/captures minted
  under `full_headless`.
- `HOLON_PBT_WIRING_AXES="storage;sync;actors"` — scope the drawn universe (fail-loud on
  a typo), e.g. `"Loro;;"` for all-Loro-only runs.

**Interleaving (the scheduler-seed + kind-mask axis).** Unarmed, the keystone awaits
every write and settles all three projections between transitions, so no two writes are
ever in flight and a task-ordering bug is ungeneratable. Arming a transition kind makes
THAT kind run through the fire-and-forget dispatch door production GPUI uses, with a
seeded pump instead of the immediate await. Arm one kind at a time so every red names a
`(kind, seed)` pair; the per-transition settle still runs before the tick's invariants,
so the oracle is exactly as strict as it is unarmed.

- `HOLON_PBT_SCHED_KINDS` — comma-separated `E2ETransition` variant names, or `all`.
  **Unset ⇒ empty mask ⇒ the harness runs its pre-existing code path, unchanged.** An
  unknown name panics rather than silently arming nothing.
- `HOLON_PBT_SCHED_SEED` — `u64` scheduler seed (default `0`). Mixed with the kind and
  the transition's index, so two ticks of one kind draw different pump budgets.
- `HOLON_PBT_SCHED_STEPS` — max pump steps per masked transition (default `8`).

Each masked transition prints `[interleave] <Kind>: seed=… steps=… intents=N
peak_in_flight=M`. A transition that dispatched ≥ 2 intents but never had more than one
in flight FAILS LOUD: a masked run that did not overlap proves nothing, so it must not
read as green. Kinds that dispatch a single intent per transition can never overlap
(the window is intra-transition) and are observed, never asserted on.

**The seed widens the interleaving; it does not replay it.** The armed door
(`dispatch_intent_through_armed_door`) hands the intent to
`ReactiveEngine::dispatch_intent`, which spawns onto the ambient tokio
multi-thread runtime (`reactive.rs:3633`), not through Increment 1's injectable
`Spawner` seam (`holon_api::spawner::Spawner`). The pump's `steps` yields are
therefore raced against real OS thread scheduling: the `(kind, seed)` pair
reproducibly selects the *pump budget* (verified: identical seed ⇒ identical
`seed=…` and `steps=…` on every `[interleave]` line, though how many such lines
a run reaches varies), but NOT the resulting
interleaving or which invariant an armed red trips — repeated runs of the same
`(kind, seed)` have been observed to fail on different blocks and different
oracles. Treat an armed red as one triaged sample of a widened window, not as
a reproducible case: don't expect `HOLON_PBT_SCHED_SEED` alone to reproduce a
specific finding, and capture the actual failing log alongside the finding
when triaging it (see BugFunnel). Routing the armed door through the
`Spawner` seam with a deterministic (single-thread, seed-ordered) executor
would narrow this gap; it is deferred, not built, and is a candidate for
Increment 3. It would not close it: tasks spawned inside `loro`, inside the
vendored `turso`, or at any `tokio::spawn` not yet routed through the seam
still run on tokio's own scheduler.

An armed run is an OBSERVATION run, not a gate. Its reds are triage input:
`bug-gap-triage` them into `docs/Testing/BugFunnel.md` and decide reference-model vs SUT
before funding a fix.

**Product surface ↔ grid points** (reduced surface: GPUI desktop+mobile, dioxus-web,
MCP; tui/flutter/waterui are archived and deliberately NOT in the axes):

| Shipped assembly | Grid point |
| --- | --- |
| GPUI desktop default (Turso authority, CRDT off) | `full_headless`-like draws: `{Loro?, Org, Turso}` + ViewModel |
| GPUI mobile (crdt.enabled ⇒ Loro authority + Turso) | `{Loro, Turso}` draws; substrate pinned by `keystone_boots_ios_crdt_loro_authority_substrate` |
| dioxus-web worker (SqlOnly Turso, no Loro) | `{Turso}`-without-Loro draws (≈ `Wiring::sql_only()`) |
| Headless MCP | draws with `Actor::MCPServer` |

**Invalid assemblies fail loud, they are not tested.** Composition-time rejections:
`Wiring::validate()` / `ComponentSet` validity (test-side), and in PRODUCTION startup
`OperationDispatcher::assert_content_write_capability()` — a `block` pipeline wired
without its CRUD ops (the `EventInfraModule`-alone trap) crashes `BackendEngine`
construction with a message naming the missing ops and the fix
(`crates/holon/src/api/operation_dispatcher.rs`; tests `content_write_guard_*`).

**Lattice operations (bottom-up runs + delta-debug).** The valid-wiring set is an
explicit partial order (subset lattice), queryable via
`ComponentSet::valid_children` / `valid_parents_within`; `bisect_downward` /
`bisect_upward` (`holon-pbt-core::bisect`, ADR 0009 §3) walk it greedily. The wiring is
externally suppliable, not only drawn: `reproduces_under(set, transitions)`
(`pbt/bisect_driver.rs`) replays a captured sequence under ANY supplied `ComponentSet`,
and cross-set replay uses `ReplayMode::SkipGated` — a transition gated out by the
narrower wiring becomes a flagged `StepOutcome::SkippedByGating` no-op (reference state
NOT advanced), never silently different semantics. These three properties are design
commitments: a future ladder-runner (start from the minimal wiring covering a dev
session’s diff, grow rung by rung; on failure delta-debug down to a minimal
(wiring, sequence) pair) composes out of them with no representation change.
`HOLON_PBT_PIN_WIRING="storage;sync;actors"` pins the keystone's generation to ONE
exact manifest (fail-loud on typo/invalid; mutually exclusive with FORCE_FULL) — the
env-level face of the function-arg seam (`wide_e2e_ref_for(&Wiring)`).

**Draw distribution (default axes, measured).** The validity filter reweights the
draw: raw Turso inclusion is 0.20, but `Wiring::validate` rejects empty-storage and
ActionEngine-without-Turso draws — both Turso-free — so the accepted share is higher.
Measured over 40 000 draws of `any_valid_wiring()`: P(Turso | valid) = 0.388,
P(Org | valid) = 0.545, P(ActionEngine | valid) = 0.164, P(MCPServer | valid) = 0.426.
Expected Turso (full BackendEngine + frontend) cases in a 16-case run ≈ 6.2. The drawn
universe has 22 valid raw grid points, collapsing to 20 distinct booted `ComponentSet`s
under `set_for_wiring` normalization.

**Why the Turso share is a floor, not a taste.** The whole feed-driven org write-back
path — `BlockFeed` → the `group_by`/`home_by` doc resolver → the `FileSyncController`
delta drain — exists ONLY under a query-capable adapter, and `SutOrgRead` (hence
`inv-blocks-match-ref/org`) is registered only on the frontend arm. A Turso-free draw
therefore tests write-back NOWHERE, which is why the earlier "the bias does not need
reweighting" reading was wrong: it asked whether a RUN sees Turso at all, not what
fraction of CASES can exercise write-back. `MIN_QUERY_ADAPTER_DRAW_SHARE` (1/3) pins
that fraction and `query_adapter_draw_share_meets_writeback_floor` enforces it. After
the Option-C Inc 2 cutover the holder is the only write-back path, so this floor is
load-bearing (holder design §9.5 / §10.2.7).

**Replay-mode caveat.** `stepper::run_sequence` is the ONLY cap-aware replayer
(`ReplayMode::SkipGated` gates on `required_wiring().satisfied_by() &&
caps_available(required_caps())`). `fixtures::replay_steps` and proptest's stock
persisted-regression replay are Strict/same-set by construction — replaying a recorded
sequence under a SUBSET wiring must go through `run_sequence`/`reproduces_under`, not
the fixture path.

**Seeds.** No persisted regression file exists yet for
`general_e2e_composed_pbt` (`crates/holon-integration-tests/proptest-regressions/`).
Historical captures/seeds minted under `full_headless` replay meaningfully via
`HOLON_PBT_FORCE_FULL=1` (pin) or `reproduces_under` with an explicit set.
