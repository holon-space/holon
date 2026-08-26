# net-proj Increments 1 + 2 — lane report

Workspace: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/netp-inc12`
Base: the integration tip carrying Inc 0 (`TransitionKey`). Base sentinel verified
(`grep -q TransitionKey crates/holon-net/src/bridge.rs` → hit).
Staleness re-check per plan §0: `jj diff --from 7105bfcf --summary` touches only
`crates/holon-net/*` (Inc 0's own files) — `traits.rs`, `operation_dispatcher.rs`
and `move_guard.rs` are untouched, so every §0 cite still holds.

Nothing committed. All changes are uncommitted working-copy state.

---

## 1. What was built

### Increment 1 — the `inv-net-totality` keystone invariant

**The invariant.** Every `(entity, op)` the run actually dispatched must have a
transition in the derived net. `Unanalyzable` passes — the net has said "cannot
say", which every analysis must surface. ABSENCE fails, because a missing
transition says nothing at all and no analysis can surface it.

| Piece | Path |
|---|---|
| body | `crates/holon-integration-tests/src/pbt/invariants/bodies/net_totality.rs` |
| wire | `crates/holon-integration-tests/src/pbt/composed/invariants/net_totality.rs` |
| catalog line | `crates/holon-integration-tests/src/pbt/composed/catalog.rs` |
| cap | `crates/holon-integration-tests/src/pbt/net_cap.rs` (`SutDerivedNet`) |
| fired-op source | `crates/holon-integration-tests/src/test_tracing.rs` (`SpanCollector::dispatched_operations`) |
| cap host | `crates/holon-integration-tests/src/pbt/frontend_slice/components.rs` |

**How "what fired" is observed — no new production seam.** The dispatcher
already opens a `dispatcher.execute_operation` span carrying
`operation.entity` / `operation.name`, on EVERY dispatch path, and the keystone
already runs a `SpanCollector` that the harness resets per transition. So the
fired set is read off spans the system already emits. Two consequences worth
stating: the span opens BEFORE routing, so a refused or failing operation counts
as fired (the net must describe what the system attempts); and paths that never
reach the dispatcher — the engine-synthetic `block` compounds — simply do not
appear, which is safe because the invariant is a subset check.

**One production change was required for the invariant to be TRUE rather than
approximately true.** `execute_operation_with_input` re-resolves the entity from
the `id` param's URI scheme when the named entity advertises no matching op
(`operation_dispatcher.rs:800-828`) — a caller naming a view (`focus_roots`) can
route to `block`. The span recorded only the DECLARED name, so the invariant
would have red-flagged legitimate scheme-resolved dispatches. Fixed by declaring
`operation.resolved_entity = tracing::field::Empty` on the span and recording it
once routing settles (`operation_dispatcher.rs:567`, `:835`). No behavior change;
`dispatched_operations()` prefers the resolved name and falls back to the
declared one for a dispatch that errored before routing settled.

**Both halves live on ONE cap** (`SutDerivedNet::{derived_net, fired_operations}`)
deliberately: the invariant's question needs both answers to come from the same
SUT, and a single cap makes the body drivable by a hand-built fake — which is
what turns the non-vacuity ablation into a unit test instead of a whole-slice
run.

### Increment 2 — the production sources

**2a — total descriptor catalog.** `BackendEngine::operation_catalog()`
(`crates/holon/src/api/backend_engine.rs`) = `dispatcher.operations()` ∪ the
engine-synthetic `block` compounds.

- `*::sync` / `*::full_sync` are excluded explicitly with a comment: their
  `entity_name` is `"*"`, a fan-out marker and not a relation, so they name no
  place to lower an arc onto. What they actually do is re-dispatch to each
  syncable provider, and those dispatches are described by that provider's own
  descriptors.
- Duplicates keep the FIRST occurrence. This is not silent vanishing: it is the
  routing rule dispatch itself follows (`execute_operation` takes the first
  provider advertising the pair), so the net describes the descriptor that would
  actually run. It is also forced — `STRUCTURAL_BLOCK_OP_DUP_ALLOWLIST`
  (`operation_dispatcher.rs:1179+`) documents that the structural block ops are
  knowingly double-advertised by `SqlBlockOperations` + `LoroBlockOperations`, so
  a strict `derive_net` would `Err(DuplicateTransition)` on every real wiring.
  Duplicates stay policed where they arise, by the registry-uniqueness assertion
  in `OperationDispatcher::operations`.

**2b — the watcher publishes its verdict.** `RuleSource` now carries a
`RuleAcceptance` instead of a bare `HolonRule`
(`crates/holon-net/src/compile.rs`):

| Variant | Meaning | Net shape |
|---|---|---|
| `Running(rule)` | parsed, owned by this watcher, firing | `active: true`, Analyzable |
| `Parked { rule, reason }` | parsed but not fired here — guard-only (advice reconciler owns it), or a subject with no reactive binding | `active: false`, Analyzable, full arcs |
| `Opaque { reason }` | never parsed — malformed body, or a paired block `action_watcher` owns | `active: false`, `Unanalyzable { Arcs, MarkingDelta }`, no arcs |

The hardcoded `matches!(rule.guard.subject, Subject::Clock)` mirror at
`compile.rs:140` is DELETED. `active` is now the watcher's own answer, which also
fixes the mirror's pre-existing bug: it ignored the paired-rule skip at
`holon_rule_watcher.rs:137`, where `action_watcher` owns the rule instead.

Registry: `crates/holon/src/api/accepted_rules.rs` (`AcceptedRuleHandle`), held on
`BackendEngine` beside `RuleStatusHandle` and written by the watcher's discovery
loop. **Lock scope:** every write is a single `self.0.write()…insert(…)`
statement whose guard is dropped at the end of that statement, and no write is
held across an `await`. The discovery loop reads from an
`mpsc`-backed `RowChangeStream`, so the registry lock is never taken while the
matview subscription is being serviced — the loop's critical section is
unchanged in length.

**2c — `BackendEngine::derived_net()`.** One method pulling 2a + 2b and calling
`derive_net`, recomputing per call and holding nothing. The code comment states
D31.a is a RULING and names why a cache would be wrong (providers register after
boot via `declare_type`; rule blocks are discovered reactively), so nobody adds
one "for symmetry".

---

## 2. The sub-fork, settled by measurement

**The apparent disagreement.** `crates/holon/src/di/registration.rs:434` passes
`false` to `block_synthetic_descriptors`; `crates/holon/src/api/operation_engine.rs:2814`
passes `self.template_source.is_some()`.

**Verdict: not a disagreement, and not a latent bug. No bugfunnel entry.**

The two sites answer different questions, and the shared parameter name
`include_template_picker` is what makes them look like they conflict:

- `registration.rs` builds `entity_operations` for the `ProfileResolver` — the
  profile MENU. Its own comment says so: "`instantiate_template` is NOT injected
  here (`include_template_picker = false`): it is surfaced via the template
  picker, not as a bare profile op."
- `operation_engine.rs` builds `available_operations` — the discovery list.

**The measurement.** The gate that decides what can actually FIRE is
`DispatchingOperationEngine::has_operation` (`operation_engine.rs:2834-2840`):
it admits `block::instantiate_template` exactly when `self.template_source.is_some()`.
That is the engine's value, not registration's literal `false`. So the net takes
the firing value.

**Encoded as a test, not left as a code reading.**
`the_net_admits_instantiate_template_exactly_when_dispatch_does` asserts the net
describes `instantiate_template` **iff** `has_operation` admits it — it agrees
with the gate rather than with either literal, so it stays green in a wiring with
or without a template source.

The new accessor is named for the question it answers:
`DispatchingOperationEngine::firable_block_synthetic_descriptors()`, with a doc
comment recording why the two call sites legitimately differ.

---

## 3. Per-tick derive cost (plan risk #4)

D31.a recomputes per call and the invariant runs per tick, so the derive sits on
the tick path. Measured and pinned by
`deriving_the_net_is_cheap_enough_for_the_tick_path`
(`crates/holon/src/api/backend_engine.rs`), which times 100 derives and prints
the per-call cost.

> **MEASURED: 219.497 µs per call** (`[net-proj] derived_net() cost: 219.497µs per call`,
> `target/lane-logs/inc2-holon-lib-test.log`, 100 derives after a warm-up call).

That is ~23x under the 5 ms assertion bound and far below any per-tick budget, so the
invariant stayed on the per-tick hook and the plan's fallback (move it to `finish` over
the union of fired ops) was not needed.

The assertion bound (5 ms) is deliberately far above the measured cost: it guards
against a derive that grows an I/O or a quadratic scan, not against scheduler
noise. The invariant stayed on the per-tick hook.

---

## 4. Cargo.lock

Two lines added, both `"holon-net",`:

- into `holon`'s dependency list — `BackendEngine::derived_net` returns a
  `holon_net::CompiledNet`, and `holon_rule_watcher` constructs
  `holon_net::RuleAcceptance`.
- into `holon-integration-tests`' dependency list — the `SutDerivedNet` cap and
  the invariant body name `holon_net` types.

`jj diff --git Cargo.lock` shows exactly those two `+` lines and nothing else: no
version bumps, no unrelated crate entries, no `cargo update` was run. Both are
the mechanical consequence of the two `Cargo.toml` dep additions. `holon-net`
depends only on `holon-api` / `holon-pattern` / `holon-rules`, so neither
addition creates a cycle.

---

## 5. Tests added

| Test | File | Pins |
|---|---|---|
| `an_unparseable_rule_block_is_inactive_and_unanalyzable` | `crates/holon-net/tests/transition_key.rs` | parse-failed rule → `active: false` + `Unanalyzable`, no arcs |
| `a_parked_rule_stays_analyzable_but_inactive` | same | a parked rule keeps its arcs; only `active` records that it does not fire |
| `the_wildcard_sync_descriptors_are_not_in_the_net` | `crates/holon/src/api/backend_engine.rs` | `*::sync` / `*::full_sync` absent BY NAME, with a non-empty-net guard so it cannot pass vacuously |
| `the_engine_synthetic_block_compounds_are_in_the_net` | same | `convert_block_to_page` / `merge_blocks` are dispatchable AND described |
| `the_net_admits_instantiate_template_exactly_when_dispatch_does` | same | the sub-fork measurement |
| `a_refused_rule_block_is_an_inactive_unanalyzable_transition` | same | registry → `derive_net` wiring end to end |
| `deriving_the_net_is_cheap_enough_for_the_tick_path` | same | plan risk #4 |
| `a_refusal_is_published_as_a_source_and_a_delete_forgets_it` | `crates/holon/src/api/accepted_rules.rs` | registry set/clear |
| `a_described_operation_passes_even_when_unanalyzable` | `.../bodies/net_totality.rs` | `Unanalyzable` is a declaration, not an absence |
| `dropping_one_descriptor_registration_reds` | same | NON-VACUITY ABLATION |
| `the_wildcard_fan_out_is_not_a_hole` | same | `*` is out of the net's domain, not a hole in it |

---

## 6. Gates

_pending — filled in below once run._

---

## 7. Deferred / flagged

- **The invariant's fired set is per-tick, not per-run.** The harness resets the
  span window at each `apply_transition`, so the check covers the ops of the tick
  just applied (plus the boot dispatches at the first `check_invariants`). The
  union-over-the-whole-run form the plan offers as a fallback was not needed,
  because the per-tick cost measured fine.
- **`Layer::StoreCrdt` is the invariant's attribution.** There is no `OpDispatch`
  layer in `holon_pbt_core::attribution::Layer`; the op-dispatch registry sits
  below the projection, so `StoreCrdt` is the closest existing rung. If the net
  work grows more invariants, a dedicated layer variant is worth its own change.
- **Rules reach the registry only through `holon_rule_watcher`.** Rules owned by
  `action_watcher` (the legacy query+action pairs) enter the net as `Opaque` with
  a reason naming that ownership. That is honest — this net does not model
  `action_watcher`'s firing decisions — but it means a paired rule's arcs are
  invisible to the analyses. Modelling them needs `action_watcher` to publish its
  own verdicts, which is out of this increment's scope.
