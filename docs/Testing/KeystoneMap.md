# Keystone map

Where each part of the keystone PBT
(`crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs`) is
registered, and what to touch to add one. Paths and symbols only: open a
symbol with `ast-outline show <file> <symbol>`.

Short names used below:

- `P` = `crates/holon-integration-tests/src/pbt`
- `C` = `crates/holon-pbt-core/src`

## How the parts fit

One case of the keystone:

1. The initial-state generator draws a wiring and builds the reference
   model for it.
2. The production builder boots a SUT for that wiring. Each component
   registers its SUT caps (capability traits) in a `CapMap`.
3. The transition generator draws transitions. It offers only the
   transitions whose caps the wiring provides.
4. Each transition changes the reference model (`apply_to_ref`) and the SUT
   (`apply_to_sut`).
5. After each step, the catalog runs every invariant whose `Needs` the SUT
   caps and the reference caps satisfy.

The worked examples all use one feature: the calendar day rollover
(`AdvanceDay`). Together they show a full feature: one SUT cap, one
reference cap, one transition and one invariant.

## Entry point

| File | Symbol | Role |
|---|---|---|
| `crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs` | `general_e2e_composed_pbt` | The test function: runner, known-red tally, end-of-run floors |
| `P/composed/wide_e2e.rs` | `WideE2E` | The slice: binds transition enum, machine and handle |
| `P/composed/wide_e2e.rs` | `WideE2EMachine` | The proptest `ReferenceStateMachine` |
| `P/composed/harness.rs` | `ComposedSlice` | The slice trait (`build`, `required_invariants`) |
| `P/composed/harness.rs` | `ComposedSut` | The `StateMachineTest` that drives the SUT and runs the catalog |

## Caps

A cap is a trait. A SUT cap (`Sut…`) is what the SUT can do or show. A
reference cap (`Ref…`) is what the reference model can answer.

| File | Symbol | Role |
|---|---|---|
| `C/capabilities.rs` | `SutClockAdvance` | Example SUT cap trait; most cap traits live in this file, frontend ones in `holon_frontend::pbt_caps` |
| `C/capabilities.rs` | `RefClock` | Example reference cap trait (read) |
| `C/capabilities.rs` | `RefClockMut` | Example reference cap trait (write, used by `apply_to_ref`) |
| `C/composition.rs` | `CapMap` | The cap registry of one SUT or one reference model |
| `C/composition.rs` | `CapProvider` | `register` puts a component's caps into a `CapMap` |
| `P/frontend_slice/components.rs` | `HeadlessFrontendComponent.advance_clock_days` | Example SUT cap impl on the frontend component |
| `P/frontend_slice/components.rs` | `HeadlessFrontendComponent.register_non_gesture` | Where the frontend component inserts most of its caps |
| `P/composed/builder.rs` | `compose_sut` | The production builder the keystone boots |
| `P/composed/builder.rs` | `compose_sut_seeded_impl` | Where per-wiring and env-gated caps are inserted |
| `P/composed/wide_e2e.rs` | `cap_ids_for_wiring` | Declared wiring → cap-set table; narrows the generator without a boot |
| `crates/holon-integration-tests/tests/composed_cap_set_table_matches_compose_sut.rs` | `assert_table_is_exact` | Fails when the table and `compose_sut` disagree |
| `P/transition_dispatch.rs` | `declare_e2e_transitions` | Its `apply_to_sut` impl carries the union bound of all SUT caps a transition uses |
| `P/ref_caps/clock.rs` | `advance_day` | Example reference cap impl on `ReferenceState` (one file per cap family in `P/ref_caps/`) |
| `P/ref_caps/mod.rs` | `register` | Where `ReferenceState` inserts its reference caps |

Worked example: add the SUT cap `SutClockAdvance`.

1. Declare `pub trait SutClockAdvance` in `C/capabilities.rs`.
2. Implement it on the component that owns the behaviour:
   `impl SutClockAdvance for HeadlessFrontendComponent` in
   `P/frontend_slice/components.rs`.
3. Insert it into the `CapMap`: `caps.insert(comp.clone() as Arc<dyn
   SutClockAdvance>)` in `compose_sut_seeded_impl` (here only under the env
   gate `HOLON_PBT_ADVANCE_DAY`). A cap that every frontend
   boot has goes into `register_non_gesture` instead.
4. Add it to `cap_ids_for_wiring` under the same condition as in step 3, else
   `assert_table_is_exact` fails.
5. If a transition calls it, add `+ ::holon_pbt_core::capabilities::SutClockAdvance`
   to the union bound in `declare_e2e_transitions`.

A reference cap follows the same steps on the reference side: trait in
`C/capabilities.rs`, impl on `ReferenceState` in `P/ref_caps/<family>.rs`,
one `caps.insert(self.clone() as Arc<dyn RefClock>)` line in `register` in
`P/ref_caps/mod.rs`.

## Transitions

One file per transition in `P/transitions/`. The macro
`crate::declare_e2e_transitions!` in `P/transitions/mod.rs` builds the enum
`E2ETransition` from one line per variant.

| File | Symbol | Role |
|---|---|---|
| `P/transitions/advance_day.rs` | `AdvanceDay` | Example transition: data, generator, ref apply, SUT apply |
| `P/transitions/mod.rs` | `advance_day` | Example `mod` line for the transition file |
| `P/transition_dispatch.rs` | `declare_e2e_transitions` | Generates the enum, its dispatch and `aggregate_transitions` |
| `C/lib.rs` | `TransitionRef` | `preconditions` and `apply_to_ref` |
| `C/cap_transition.rs` | `cap_transition` | Macro that writes `apply_to_sut` against the named SUT cap, plus the SQL budget |
| `P/transitions/mod.rs` | `every_variant_has_a_dedicated_file` | Fails when a variant has no file |
| `P/transitions/mod.rs` | `every_file_is_registered_as_a_module` | Fails when a file has no `mod` line |
| `P/transitions/mod.rs` | `required_caps_match_transition_impl_bounds` | Pins `required_caps` against the impl bounds, for the transitions it lists |
| `P/transitions/mod.rs` | `every_transition_is_drawable_in_a_shipped_composition` | Fails when no shipped wiring can draw a transition |

Worked example: add the transition `AdvanceDay`.

1. Create `P/transitions/advance_day.rs` with `pub struct AdvanceDay`. Put the
   `@pbt` header tags at the top (they feed `docs/Testing/pbt-inventory.md`).
2. In that file, implement:
   - `TransitionFactory` (`weighted_generator`, see Generators),
   - `TransitionRef` (`preconditions`, `apply_to_ref` through `RefClockMut`),
   - `crate::cap_transition! { AdvanceDay: holon_pbt_core::capabilities::SutClockAdvance, … }`
     for `apply_to_sut`.
3. In `P/transitions/mod.rs`: add `mod advance_day;`,
   `pub use advance_day::AdvanceDay;` and the line
   `AdvanceDay(AdvanceDay),` in the `crate::declare_e2e_transitions!` call.
4. Run the unit tests of `P/transitions/mod.rs`: they fail on a missing file,
   a missing `mod` line, or a transition that no shipped wiring can draw.

## Invariants

An invariant body is a type that implements `Invariant<R, S>`. It is bound
on the caps it reads.

| File | Symbol | Role |
|---|---|---|
| `C/invariant.rs` | `Invariant` | The body trait (`id`, `check`) |
| `P/invariants/bodies/journal_one_per_day.rs` | `InvJournalOnePerDay` | Example body |
| `P/invariants/bodies/mod.rs` | `journal_one_per_day` | Example `mod` line for the body file |

Worked example: add the invariant `inv-journal-one-per-day`.

1. Create `P/invariants/bodies/journal_one_per_day.rs` with
   `pub struct InvJournalOnePerDay` and `pub const ID: InvariantId`.
2. Implement `Invariant<R, S>` with `R: RefClock, S: SutSqlProjection`.
   Return `InvariantResult::Skipped(<reason>)` when the state gives nothing
   to judge.
3. Add `pub mod journal_one_per_day;` to `P/invariants/bodies/mod.rs`.
4. Give it a catalog row (next section). Without one, no slice runs it.

## Catalog rows

A catalog row bridges a body into an object-safe `CapInvariant` and states
its `Needs` as data. The keystone runs every row whose `Needs` the drawn
wiring satisfies.

| File | Symbol | Role |
|---|---|---|
| `P/composed/invariants/journal_one_per_day.rs` | `wire` | Example row: body + `RunMode` + `Needs` + `Attribution` |
| `P/composed/invariants.rs` | `journal_one_per_day` | Example `mod` line for the row file |
| `P/composed/catalog.rs` | `central_invariants` | The list of `wire()` lines |
| `P/composed/catalog.rs` | `composed_invariant_catalog` | Folds the central list with the companion crates' lists |
| `crates/holon-loro-testing/src/lib.rs` | `pbt_contribution` | Example companion-crate contribution |
| `P/composed/correspondences.rs` | `org_blocks` | Example correspondence family (several rows from one `wire()`) |
| `C/composition.rs` | `BridgedInvariant` | The bridge a row wraps the body in |
| `C/composition.rs` | `Needs` | `sut_present`, `sut_absent`, `ref_present` |
| `C/composition.rs` | `run_selected` | Selects rows by `Needs` and runs them |
| `P/composed/wide_e2e.rs` | `WideE2E.required_invariants` | Floor: every selected row must run on the draw |

Worked example: add the catalog row for `inv-journal-one-per-day`.

1. Create `P/composed/invariants/journal_one_per_day.rs` with `pub fn wire()`.
   It returns `BridgedInvariant::new(InvJournalOnePerDay, RunMode::Strict,
   Needs { … }, Attribution::at(Layer::Projection, file!()))`.
2. List in `Needs` every cap the body reads, plus each cap that limits the row
   to the wirings where the check means something. Here `SutClockAdvance` keeps
   the row off Turso slices that never fire the journal rule.
3. Add `pub mod journal_one_per_day;` to `P/composed/invariants.rs`.
4. Add `invariants::journal_one_per_day::wire(),` to `central_invariants`.
5. Run the `catalog_suite` test of holon-integration-tests.

## Generators

| File | Symbol | Role |
|---|---|---|
| `C/wiring.rs` | `wiring_axes` | The axes the wiring is drawn over |
| `C/wiring.rs` | `any_valid_wiring` | The wiring strategy (shrinks toward Loro-only) |
| `P/composed/wide_e2e.rs` | `WideE2EMachine.init_state` | Initial-state generator: wiring + reference model |
| `P/composed/wide_e2e.rs` | `WideE2EMachine.transitions` | Transition generator; calls `aggregate_transitions` |
| `C/lib.rs` | `TransitionFactory` | Per-transition `weighted_generator` and `required_caps` |
| `P/transitions/advance_day.rs` | `AdvanceDay.weighted_generator` | Example per-transition generator |
| `P/transition_dispatch.rs` | `variant_weight_multiplier` | Weight override per variant (`HOLON_PBT_WEIGHTS`) |
| `P/stepper.rs` | `transition_applicable` | Alphabet gate: drops transitions the shrunk wiring cannot drive |
| `P/generators.rs` | `content_strategy` | Example content generator of the keystone (more shared strategies: `C/content_generators.rs`) |

`aggregate_transitions` and `transition_arms` have no file of their own:
`declare_e2e_transitions` generates them. A variant is offered when the drawn
wiring satisfies its `required_wiring`, the cap set has its `required_caps`,
and its `weighted_generator` returns a weight and a strategy.

Worked example: the generator of `AdvanceDay`.

1. In `AdvanceDay.weighted_generator`, return `Fail(<Reason>)` while the
   transition cannot run (`Reason::AppNotStarted`).
2. Else return `(weight, strategy)`: the weight against the other variants,
   and a strategy that draws a rollover more often than a same-day re-tick.

## Reference model

| File | Symbol | Role |
|---|---|---|
| `P/reference_state.rs` | `ReferenceState` | The model; one field per sub-state |
| `P/clock_state.rs` | `ClockState` | Example sub-state (field `clock`) |
| `P/composed/wide_e2e.rs` | `wide_e2e_ref_for` | Builds the model for a drawn wiring |
| `P/composed/wide_e2e.rs` | `WideE2EMachine.apply` | Applies a transition to the model |
| `P/ref_caps/clock.rs` | `advance_day` | Example model change behind a reference cap |
| `P/ref_caps/clock.rs` | `visited_days` | Example model read behind a reference cap |

Worked example: model the day rollover.

1. Add the sub-state `ClockState` in `P/clock_state.rs` and the field
   `clock: ClockState` on `ReferenceState`; initialise it where
   `ReferenceState` is constructed.
2. Expose it only through reference caps: `RefClock` (read) and
   `RefClockMut` (write) in `P/ref_caps/clock.rs`, registered in `register`
   in `P/ref_caps/mod.rs`.
3. Model every effect that production has on the state the invariants read.
   Here `advance_day` also seeds the journal day-block that the production
   journal rule creates, so the block-tree invariants agree with the SUT.
