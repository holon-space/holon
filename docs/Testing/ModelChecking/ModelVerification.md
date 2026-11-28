# Model verification: Petri nets, TLA+ and tests

This document explains how model checking, the Petri-net executor, the
functional core and the tests fit together. It states the target, what each
part proves, and which gaps the tests still close.

Status: the method is in use for hand-written specs (`Writeback.tla`,
`SettleWake.tla`, `HomeBySettle.tla`). Two parts are hypotheses that spikes
test now:
- A spec generated from a net file: `WorkRuntimeSpike.md`.
- A hand-written spec of the executor's reader isolation (its spike report lands with that spec).

## Worked example: one keystroke, three views

You type `a` in a block. The block shows in the editor, in a backlink panel
and in a search result. You want this to be true:

- **Safety.** A view never shows text that you did not type, and a view never
  goes back to an older version.
- **Liveness.** When you stop typing, every view eventually shows the last
  keystroke. While you type, views may differ.

In TLA+ the liveness half reads:

```tla
Converges == <>[]UserIdle => <>[](\A v \in Views : view[v] = buffer)
```

"Eventually" needs the condition `UserIdle`. Without it, a user who never
stops typing makes the property fail for a trivial reason: no view ever shows
the newest key.

Liveness also needs fairness: a transition that stays enabled eventually
fires. The engine must keep this promise. A test of the engine checks it once.

## Terms

| Term | Meaning |
|---|---|
| Engine | The general firing rules of the Petri-net executor (consume, read, inhibit, produce, priority). |
| Net instance | One net, for example `crates/holon-net/nets/work-runtime.yaml`. It is the coordination design. |
| Spec | A TLA+ description of the states and the steps of a system. |
| Property | A statement that every behaviour of the spec must satisfy: an invariant (safety) or a liveness property. |
| Witness | An invariant that must FAIL. It shows that a state is reachable, so that a property does not hold only because nothing happens. |
| Contract | The precondition and the postcondition of one transition body, over its arc places. |
| Environment | What the outside world may do: effect outcomes, probe facts, the clock, the user. |

## A Petri net and a TLA+ spec are the same kind of thing

Both describe a state-transition system.

| Petri net | TLA+ spec |
|---|---|
| place | variable |
| marking | state |
| transition with its enabling rule | action: enabling condition and next state |
| input arcs (what a transition reads) | the variables the action reads |
| one firing | one step |

A net that is data (a net file) can become a spec by generation. The spec then
follows the net, but only while the gate regenerates it from the net file and a
conformance PBT checks the executor against it. Without both, a generated spec
goes stale like a hand-written one.

For normal code this is not possible. A hand-written spec repeats the logic of
the code (`HomeBySettle.tla` repeats the supervision of `home_by`), and the
copy can drift. The check of the `Models:` anchors and the conformance PBTs
keep a hand-written spec close to its code. They do not make the drift
impossible.

## What the net does not describe

1. **Transition bodies.** The net says which tokens move, not which value an
   output token has. A property about content ("the keystroke shows up") needs
   the contract of each body involved.
   - Contracts compose with the generated net spec, because each contract
     reads and writes only the places on its arcs.
   - This holds only when the arcs list every read and write. The engine
     enforces it: reading an undeclared place is a refusal
     (`docs/Architecture/PetriNetLayers.md`, section "L0 petri-net").
2. **The environment.** Effects, probes, the clock and the user are written
   as "what the outside may do". The model checker assumes these statements.
   It cannot check them.
3. **Data size.** TLC needs a finite state space: for example 2 blocks,
   3 keystrokes and 2 views. Most bugs show at small sizes. That is a
   hypothesis, not a proof.

So in the target state a person writes:
- the properties of the net instance;
- the contracts of the transition bodies;
- the environment.

The generator writes the behaviour.

## The functional core is where code and spec meet

A conformance PBT needs a pure function: state and input in, next state out.
The functional core gives this.

- **One TLA+ action = one core function.** The conformance PBT calls the
  function and checks each step against the matching action. tla-checker
  evaluates the action in process. Each recorded call is a search over a few
  unobserved steps, so a case costs more than one evaluation.
- **The imperative shell = the scheduler.** It decides which action runs when.
  TLA+ writes this as a free choice between the enabled actions, and TLC tries
  every order. A random PBT samples some orders.
  - A deterministic scheduling step in the test harness (a `pump`) makes the
    shell's choice explicit, so that a PBT can replay a TLC trace.
- **One transaction = one action.** A read-modify-write is one pure plan
  inside the engine transaction. Each split of a transaction adds a point
  where other steps can interleave. The reason for a split therefore includes
  the TLC run that shows the split is safe.

## Layers compose by refinement

The executor has layers: the general Petri net, the workflow net (effects,
timers, retry), and the Holon net. A higher layer's spec is an abstraction of
the lower one. TLC checks the refinement mapping between them.

Witness invariants are the TLA+ form of Petri-net liveness: every transition
can fire at least once.

## What the model checker proves, and what the tests close

The model checker proves:

> design + transition contracts + environment assumptions ⇒ properties, within the bound.

Each part of the left side is a gap between the model and the running code.
The tests close the gaps:

| Gap | What checks it |
|---|---|
| The engine fires as the semantics say | Engine PBT against a reference semantics, once |
| The generator reads the net file correctly | A mutated net file makes a property red |
| A transition body meets its contract | A PBT per transition; pure; tla-checker evaluates the contract |
| No hidden channels | The footprint refusal, and architecture tests |
| The outside world matches the environment | Integration tests against real Turso, Loro and the file system; the keystone PBT; dogfood |
| Sizes above the bound | Random PBT at larger sizes, with the spec as oracle |
| Latency, memory, pixels | Performance tests, GPUI PBT, dogfood |
| The property is what the user wants | Witnesses, review, dogfood |

Trace validation ("the special logs") records the firings of real runs and
checks that each one is a step of the spec. It finds hidden channels and wrong
environment assumptions. It does not cover interleavings: TLC does.

Expected effect: race bugs move to design time. The remaining bugs collect at
the gaps in the table, so tests can aim at them.

## How this makes the test suite faster

1. **The race search leaves the slow tests.** The keystone PBT hunts
   interleavings with long random runs under load, which is slow and flaky.
   TLC covers all interleavings within the bound, in seconds for small bounds
   and in many minutes for larger ones under load. The keystone then checks
   wiring and the gaps, with fewer and shorter cases.
2. **Contract PBTs are pure.** A case takes microseconds to milliseconds, with
   no database and no async runtime.
3. **Selection by contract.** If a change in a body keeps its contract, its
   contract PBT runs, and the model-check result stays valid. The wide test
   steps can go to the landing gate. This needs strong contract tests.
4. **No search for known counterexamples.** A TLC counterexample becomes a
   fixed example in the PBT.

The cost: TLC time grows with the state space. Specs need bounds, and they are
split along their parts.

## Crate interfaces as specs

A crate interface can be a Rust type plus a TLA+ spec. The crate proves the
spec under its assumptions and promises it to its callers. This is
assume-guarantee composition, and TLA was designed for it.

What it gives: high confidence in the coordination logic. What it does not
give: a correct system.

- TLC checks within a bound. A full proof (TLAPS) costs much more.
- Code meets its spec only as far as the tests check it. Proving code against
  a spec is possible, but at a very high cost.
- Safety composes easily. Liveness needs care: if crate A progresses only when
  B progresses, and B only when A progresses, the assumptions are circular and
  the composition is not sound.
- Rust types give the shape of data, not the order of calls. The order lives
  in the spec.
- Performance, resource limits, library bugs and UI stay outside unless they
  are modelled. A crash can be modelled as a transition (the Poisoned place).
- A wrong property gives a verified wrong system.

Recommendation: where possible, a crate interface is the same thing as a
transition contract. Then one kind of artifact specifies both, and both
compose by one rule (`docs/Architecture/Vision-PetriNetCoordination.md`,
section 5, "Engines as substitution transitions").
