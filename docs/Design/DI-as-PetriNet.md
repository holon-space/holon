# Dependency injection as a Petri net

Status: open design question (Martin, 2026-10-08). Nothing here is decided or built.
Direction context: [Vision-PetriNetCoordination.md](../Architecture/Vision-PetriNetCoordination.md).

## Summary

- Holon's dependency injection (fluxdi with live edges) maps closely onto a Petri net.
- Services are places. Factories are transitions. Dependencies are read arcs.
- The states of a live cell (Pending, Partial, Ready, Failed) map onto the marking.
- "Boot as much as possible" is then the meaning of running the net, not a feature.
- The current choice is a middle path: fluxdi stays the executor, and its cell states are mirrored into the net as read-only environment places.
- A full "DI is a net" design is a later step. This note keeps the mapping, the trade-offs and Martin's answers so the discussion can continue.

## The mapping

| DI concept | Petri-net concept |
|---|---|
| A service type (or binding) | A place |
| An instance exists | A token on that place |
| A factory | A transition |
| A hard dependency | A read arc (the token is read, not consumed) |
| A live edge (`Live<T>`) | An arc that does not need the input to be Ready; the consumer observes the place |
| Pending | The place is empty, or a "building" place holds a token while the factory runs |
| Partial | A token whose value is partial (see "Partial inputs") |
| Ready | A token on the service's place |
| Failed(cause) | A token on a "failed" place, with the cause as its color |
| Restart / reconnect | A transition from "failed" back to "building"; generations are colored tokens |
| Scopes, per-document or per-user instances | Colored subnets |
| `shutdown_live` | A transition that empties the "building" places and ends production for good |

## Pull is an algorithm

Martin (2026-10-08): pull is not "demand tokens". Pull is an algorithm that decides which transitions fire, and in which order.

- Start at the place that must be filled.
- Find the transitions that can fill it, and what each one needs.
- Recurse into the missing inputs.
- A place that already holds a token is solved. That is the DI cache.
- Remember solved places (memoization). Detect a cycle when a place is already on the current path (fluxdi's ancestor path does this today).
- When several transitions can fill a place, rank them (ADR 0017).
- The output is a plan: a partial order of transitions.

The same algorithm serves boot ("fill: the window shows a session") and life planning ("fill: Chicken Teriyaki cooked tonight" finds that chicken must be bought). Martin earlier expected such questions to be answered by simulation. Pull is the efficient heuristic; simulation checks a pulled plan against uncertainty.

Spike result (2026-10-08, report `~/.claude/plans/pull-planner-spike.md`): pull works on Holon's real nets without engine changes. 1k transitions: 42 ms per pull. 10k: 8.6 s, because the spike copies whole sub-plans per place. The fix is to memoize only the choice per place and build the plan once.

## Engines inside one net

Martin asked whether one net can integrate specialized engines such as fluxdi. The answer is hierarchical nets with substitution transitions (as in CPN Tools). An engine sits behind a transition and offers three things:

1. Its interface places and their marking (for fluxdi: the cell states).
2. A planner query: "can you produce P, and what do you need?"
3. A demand: "produce P now". The engine pulls internally.

fluxdi, Turso IVM and Loro are the first candidates. The mirror of fluxdi cell states (boot-always ADR draft) is the first half of this contract.

## Fast transitions

Martin asked whether transitions can be cheap, even synchronous, and whether functions can be transitions with a fast path.

- **Pure transitions** (synchronous, deterministic, no side effects): a function is a transition whose inputs are its argument places and whose output is a result place. When all inputs are marked, call it inline and memoize. No journal, admission or cancellation. Chains can be fused, as IVM or Salsa do. Allowed on the render path.
- **Effectful transitions** (asynchronous or world-changing): journal, admission, cancellation and generations stay.

A cached synchronous fluxdi resolve is already a pure transition on the fast path.

## Bootstrap

Martin (2026-10-08): the boot net is hard-coded and then extends itself. A small kernel net in code brings up the store, the always-available core and the recovery path. Subnets declared in data (vault, capabilities) extend it. An extension is validated before activation. A failing extension disables only its own subnet.

## Partial inputs

Ruled (Martin, 2026-10-08): each input arc declares "complete only" (the default for hard DI dependencies) or "accepts partial". A result computed from a partial input is itself partial.

## Time

Direction (vision note §2.5): tokens carry validity intervals, transitions carry durations, resources (a shop, a person's calendar) are tokens. Pull with time is backward scheduling from a deadline.

## Trade-offs of "DI is the net"

Pros:

- One enablement model for services, data availability, document writability and user operations.
- "Why is this unavailable?" is answered by tracing the missing token back, also through services.
- Structural analysis finds cycles and dead parts before anything runs (the postponed static cycle check).
- Fix actions are ordinary transitions enabled by Failed markings.

Cons and open points:

- **Typing.** fluxdi hands out typed `Arc<T>` keyed by `TypeId`; the compiler checks the wiring. Net tokens are untyped or colored, so a typed bridge is needed, and some errors move from compile time to run time.
- **Speed.** Synchronous resolves happen on hot paths, including render. A resolve after boot must stay a cache read, not an engine call.
- **Execution semantics.** Factories are long, asynchronous and can fail. ADR 0032 must allow "firing" to take time (explicit building places) and to fail.
- **Scopes.** Child injectors become colored subnets. Martin: this is required anyway and is a driver, not a cost.
- **Bootstrap.** Martin: solved by the hard-coded kernel net.
- **Switching cost.** fluxdi's live edges are new and verified (increments 1-12 on `dev`). Replacing the executor now carries risk without a clear gain.

## Current choice and next questions

Current choice (not a decision): fluxdi stays the executor. Its cell states become read-only environment places in the net (ruled: environment places are allowed). The net decides what is enabled and explains what is not.

Questions to discuss next:

1. Should DI bindings be declared as net elements (places and transitions in data), with fluxdi as the engine behind them, or should the net only observe fluxdi?
2. Where is the boundary between "the kernel net in code" and "subnets in data" for DI?
3. Does a typed bridge (place per `TypeId`, token = `Arc<dyn Any>` checked at the boundary) keep enough compile-time safety?
4. Is the planner on the boot path cheap enough once the memoization fix is in (target: boot plan in well under 10 ms)?
5. How do generations and `shutdown_live` ("end, not pause") map onto net transitions?
