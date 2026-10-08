# Vision: the Petri net as the coordination layer

*Part of [Architecture](../Architecture.md). Status: **direction, not yet
built**, except where a section names code that exists. Source: Martin's design
discussion of 2026-10-08. Use this note to steer changes near DI, boot,
reactivity, simulation and the Petri net (PN). It decides nothing; the ADRs
decide.*

## Summary

1. One Petri net coordinates Holon. Specialized engines do the work.
2. To fill a place, Holon **pulls**: it finds the transitions that can fill
   the place, finds what they need, and recurses (backward chaining).
3. A marked place is solved. A solved place is memoized. The ancestor path
   detects cycles. A ranking chooses between alternatives.
4. The output of a pull is a **plan**: a partial order of transitions.
5. Boot and "cook Chicken Teriyaki tonight" use the same algorithm.
6. Simulation does not search. It checks a pulled plan against uncertainty.
7. Each engine (fluxdi, Turso IVM, Loro) is a **substitution transition** with
   a three-part contract: interface places, a planner query, a demand.
8. **Pure** transitions run inline and memoized. **Effectful** transitions go
   through the dispatcher with journal, admission and cancellation.
9. A small kernel net in code boots Holon. Data declares all other subnets.
   A failed subnet disables only itself.
10. Colored subnets (per scope, per document, per user) are a requirement.

## 1. The coordination-layer idea

Holon has one logical net (ADR 0032 §1, `docs/adr/0032-petri-net-execution-semantics.md:47-102`).
Today the net covers operations and rules. In the target, the net also
covers how services, data and documents become available.

The net does not replace the engines. Each engine keeps its fast,
specialized execution:

| Engine | Specialty | Stays inside the engine |
|---|---|---|
| fluxdi | service construction | factory chains, cache, scopes |
| Turso IVM | relational data | incremental matview maintenance |
| Loro | documents | CRDT merge, text edits |

The net sees each engine through a small interface (section 5). The net
answers the questions that cross engines: "what must happen before the window
shows a session?", "why is this operation unavailable?", "what does the user
have to do?".

Hierarchical nets with **substitution transitions** (as in CPN Tools) give the
structure. A substitution transition looks like one transition on the parent
page. A subnet on its own page implements it. The subnet can be a PN page or an
engine.

## 2. The pull algorithm

### 2.1 Idea

Pull is an algorithm. It is not a demand token that flows through the net.
To fill a place, look at the transitions that can produce a token in it.
For each one, look at its input places. Recurse. This is backward chaining
(goal regression). Build systems, Prolog and STRIPS/HTN planners use the same
method.

Four rules make it efficient and safe:

- **Marked = solved.** A place that holds a token needs no work. The fridge
  holds chicken; a fluxdi cache holds the singleton. Same rule.
- **Memoize.** A place solved once in this pull is solved for all consumers.
  The DI cache is this memo.
- **Ancestor path.** A place that appears again on its own path is a cycle.
  Report it with the path. fluxdi does this per resolve with `ResolveGuard`
  (fluxdi `24b6eeb`, `fluxdi/src/resolve_guard.rs:66`) and across tasks with
  the wait-for graph (`~/.claude/plans/fluxdi-live-edges.md` §7).
- **Rank alternatives.** When several transitions can fill a place, rank them.
  ADR 0017's ranking engine supplies the order
  (`crates/holon-engine/src/engine.rs:254`, `Engine::rank`).

### 2.2 Pseudocode

```text
pull(goal, path, memo) -> Result<Plan, Unfillable>
  if marked(goal) with a usable token:   return Plan::empty()     # solved
  if memo has goal:                      return memo[goal]
  if goal in path:                       return Err(Cycle(path + goal))
  producers = transitions with an output arc to goal
              + engines whose can_produce(goal) answers Some(needs)
  if producers is empty:
      return Err(Unfillable{goal, why: environment_or_no_producer(goal)})
  results = []
  for t in rank(producers, marking):                    # ADR 0017 order
      if refused(t) by a guard that the marking decides: continue
      subplans = [pull(p, path + goal, memo) for p in inputs(t)]
      if all subplans are Ok:
          results.push(merge(subplans).then(t))         # t after its inputs
      else:
          remember the failures                         # for the disclosure
  memo[goal] = best(results) or Err(Unfillable{goal, failures})
  return memo[goal]
```

Two token states need a rule:

- **`Failed(cause)`** is a token. It is not "empty". The normal producer of
  this generation has failed. The pull then looks for producers of the **next
  generation** of the place. Those are the recovery transitions (ADR 0035
  remedies, for example `recovery.recreate_database`).
- **An intent transition** (a user or an agent must request it, ADR 0032 §1
  table) cannot fire by itself. The pull keeps it in the plan as a
  **frontier** step. The frontier is what the system offers the user.

An **environment place** has no producer in the net (ADR 0032 §1,
`:70-82`). The clock, "shop open" and a file on disk are examples. The pull
cannot fill it. It can only wait for it or report it.

### 2.3 Worked example: boot

Goal: `window shows a session`.

```text
window shows a session
 └─ bind_window                      needs: FrontendSession
     └─ FrontendSession factory      needs: BackendEngine
         └─ BackendEngine factory    needs: schema roots, OperationDispatcher
             ├─ schema roots         needs: Turso open
             │   └─ Turso open       needs: db path (core: marked)
             └─ OperationDispatcher  needs: LiveSet<dyn OperationProvider>
```

The chain is real today. `main.rs` resolves `FrontendSession`
(`frontends/gpui/src/main.rs:98`). Its factory awaits `BackendEngine`
(`crates/holon-app/src/wiring.rs:504-515`). That factory awaits the schema
roots and the `OperationDispatcher`
(`crates/holon/src/di/registration.rs:786-798`). Today fluxdi does this pull
implicitly, without a plan as output.

Now let `Turso open` hold `Failed(SchemaStepFailed)`. The pull continues:

1. `Turso open (gen 1)` is `Failed`. Look for producers of `gen 2`.
2. Producers: `recovery.retry_boot` and `recovery.recreate_database`. Both are
   intent transitions. A condition token `BootComponentFailed{database}`
   enables both (boot-always draft §5, `~/.claude/plans/adr-draft-boot-always.md:127-139`).
3. Rank: retry first (cheap, no data loss), recreate second.
4. Plan: `[retry_boot (frontier) | recreate_database (frontier)] → Turso open
   gen 2 → schema roots → dispatcher → BackendEngine → FrontendSession →
   bind_window`.

The plan's frontier is the remedy set that the recovery screen shows. The
window does not wait for the plan. The kernel marks `window shows defaults` at
t0 (section 7). The session supersedes it when the plan completes.

### 2.4 Worked example: Chicken Teriyaki

Goal: `Teriyaki cooked (tonight)`. The same `pull` runs.

```text
Teriyaki cooked (tonight)
 └─ cook_teriyaki (recipe)  needs: chicken, soy sauce, mirin, rice, 40 min of me tonight
     ├─ soy sauce, mirin, rice       marked in the pantry → solved
     ├─ 40 min of me tonight         environment (calendar) → marked
     └─ chicken                      empty
         ├─ thaw_from_freezer        needs: frozen chicken (empty) → Unfillable
         └─ buy_chicken (intent)     needs: shop open, me at shop
             └─ shop open            environment (clock, open until 20:00)
```

Plan: `buy_chicken (frontier, before 20:00) → cook_teriyaki`. The frontier
step becomes a task block through the dispatcher (an effect, ADR 0024
"Effects are token operations"). A shopping-list connection can add the item
(ADR 0034). The recipe transition can come from a `.cook` file: its
ingredients are the input arcs.

Not built: recipe transitions, pantry places, a calendar place, and the pull
itself.

### 2.5 Time

Direction, not built. Martin's idea: a time slot of a resource (a shop, a
person) is a token. The duration of a step is a property of its transition,
or an input token that the transition consumes.

Petri-net theory has five well-known ways to add time:

| Model | Where time is | Typical use |
|---|---|---|
| Time Petri nets (Merlin and Farber) | each transition has a firing interval [min, max], counted from when it becomes enabled | timeouts, protocols |
| Timed Petri nets (Ramchandani) | a transition takes a fixed duration to fire | throughput, cycle time |
| Timed-arc Petri nets (Hanisch; Bolognesi et al.) | each token has an age; an input arc accepts only an age interval | freshness: "chicken is good for 3 days" |
| Timed colored Petri nets (Jensen, CPN Tools) | each token carries a timestamp; a transition delay adds to it; one global clock | scheduling with typed tokens |
| Stochastic Petri nets, GSPN (Molloy; Ajmone Marsan et al.) | delays are random | performance and risk |

The fit for Holon:

- **Timed colored tokens.** A token carries a validity interval. Examples:
  "shop open" is valid until 20:00; a free slot in a person's calendar;
  chicken with an expiry date. Holon's tokens are already colored (typed
  rows), so a validity interval is one more attribute.
- **A duration on each transition.** Cook = 40 min. `holon-engine` already
  moves its clock forward by the transition's duration when it fires
  (`crates/holon-engine/src/engine.rs:222-240`).
- **Resources as tokens.** A person's time slot is a token. The transition
  that uses it consumes it, so two plans cannot use the same slot.
- **Pull with time is backward scheduling.** Start at the goal's deadline.
  Go back along the plan and subtract each duration. The result is the
  latest start time of each step. MRP calls this lead-time offsetting; the
  critical-path method calls it the backward pass. A step whose latest start
  is already past, or whose input token is not valid at that time, makes
  the plan fail.
- **Random delays belong to simulation.** The pull uses one fixed duration
  per step. Simulation samples durations and reports the risk that the plan
  misses its deadline (section 3).

Chicken Teriyaki with time:

```text
goal: dinner at 19:00
cook_teriyaki   40 min              → start by 18:20
buy_chicken     30 min incl. travel → done by 18:20 → start by 17:50
  shop open     valid until 20:00   → fits; the shop does not limit this plan
  my slot       17:50–19:00 free    → consumed by buy_chicken and cook_teriyaki
```

The dinner time limits the plan, not the shop. If the calendar has a
meeting until 18:00, the pull fails with that reason, or a different plan
wins (buy at lunch).

## 3. Pull versus simulation

Simulation can also find such a plan, by trial and error. It runs forward:
it fires enabled transitions and searches the futures. The search
space grows with every enabled transition, including the irrelevant ones.

Pull runs backward from the goal. It visits only the places that the goal
needs. It is a far more efficient heuristic for "what must happen for X".

Simulation keeps a different job: **check a pulled plan against
uncertainty.** Durations vary. The shop closes at 20:00. A meeting runs late.
The simulator fires the plan on a cloned marking with sampled durations and
reports risk ("buy chicken at lunch, not after work"). It can also compare the
two or three alternative plans that the pull produced. See
[Simulation.md](Simulation.md) for the uses of simulation.
`Engine::rank` today simulates one firing per enabled transition on a clone
(`crates/holon-engine/src/engine.rs:254`); that is a one-step forward check.

The net itself runs on the live path: the planner on boot, pure transitions
inline (section 4). Simulation does not: it works on a copy and never writes
the real state. A plan commits by firing real transitions through the
dispatcher (ADR 0024 P2, `docs/adr/0024-unified-action-execution.md:98-105`).
[ADR 0031](../adr/0031-native-transition-catalog-and-macro-reification.md) guard 1
now states this direction.

## 4. Pure and effectful transitions

A transition is one of two kinds. The catalog declares the kind (ADR 0031).

**Pure** — synchronous, deterministic, no side effect. A function is a
pure transition: its argument places are the inputs, its result place is the
output. Fast path:

- Fire inline when all inputs are marked.
- Memoize by the input token values.
- Fuse chains, as IVM and incremental-computation systems (Salsa) do.
- No journal, no admission, no cancellation, no generation.
- Allowed on the render path.

**Effectful** — asynchronous, or it changes the world. It takes the full
path: dispatcher gates (`docs/adr/0032-petri-net-execution-semantics.md:183-194`),
occurrence journal, admission, cancellation, generations.

Do not give a pure transition the effectful path "to be safe". Do not put an
effectful transition on the fast path "to be fast". The kind is part of the
contract.

## 5. Engines as substitution transitions

Each engine exposes three things:

| Part | Meaning |
|---|---|
| (a) interface places + marking | the places the engine fills, with their token state |
| (b) planner query | `can_produce(P) -> Option<Needs>`: "can you produce P, and what do you need?" |
| (c) demand | `produce(P)`: "produce P now". The engine pulls internally. |

The PN sees interface places as **read-only environment places**. The engine
writes them; no PN transition does. This keeps ADR 0032 §1 intact
(`:70-82`): a place that a transition writes owes durable state, and a fluxdi
cell is not durable state.

| Engine | (a) interface places | (b) planner query | (c) demand |
|---|---|---|---|
| fluxdi | one live cell per type or set slot: `Pending` / `Partial` / `Ready` / `Failed`, with a generation | a registered provider for `T` and its declared dependencies | `resolve_live::<T>()` |
| Turso IVM | one matview or query result per (query, params), with completeness | the query compiles to a matview over known relations; needs: its schema roots | create or subscribe the matview (start on first observer) |
| Loro | one document per id: loaded / absent / failed | the document exists in the store; needs: the store | load or check out the document |

Text edits stay inside Loro and bypass the dispatcher
(`docs/adr/0032-petri-net-execution-semantics.md:219-224`). The net observes
their results as marking.

**First contract, designed:** mirror the fluxdi live-cell states into the
marking as read-only environment places (`source(vault)`,
`provider(entity)`, `home(doc)`, `condition(kind, subject)`; boot-always draft
§7, `~/.claude/plans/adr-draft-boot-always.md:152-188`).

## 6. Colored subnets

Colored subnets are a requirement, not a cost. Holon needs one subnet
instance per color: per scope, per document, per user, per integration. CPN
Tools calls these page instances. Examples that exist today in other forms:

- one fluxdi set slot per integration
  (`IntegrationSupervisor`, `crates/holon-app/src/mcp_integrations.rs:348`);
- one Loro document per document id;
- one matview per (query, context params).

Colors are unbounded domains. Do not promise model checking
(`docs/adr/0032-petri-net-execution-semantics.md:144-147`).

## 7. The kernel net and self-extension

A small **kernel net**, hard-coded in Rust, boots Holon. It holds:

- the always-available core: paths, `ConditionBus`, boot ledger, log ring,
  last panic record (boot-always draft §2);
- the transition `load defaults`, which marks `window shows defaults` at t0
  (draft §3);
- the store, the session slot with generations, and the recovery transitions.

The kernel then **extends itself** with subnets declared in data: vault rule
blocks, capability profiles, connection sidecars, integration configs. One
meta-transition does this (ADR 0032 §2 anticipates such transitions,
`:149-171`; deferred item 7, `:505-508`):

1. Load the subnet declaration.
2. Validate it before activation: compile, then run conflict and cycle
   analysis (`crates/holon-net/src/analysis.rs:62`, `:111`).
3. Activate it, or mark its places `Failed(cause)` and raise a condition.

A failed extension disables only its own subnet. This is the same shape as
defaults-first on the data side: the built-in value stays until the data
supersedes it.

## 8. How today's pieces map onto the target

| Today | Role in the target |
|---|---|
| fluxdi live cells (`Pending`/`Partial`/`Ready`/`Failed`, generations; fluxdi note §4, §15 inc 4) | interface-place marking of the service engine |
| fluxdi `ResolveGuard` (per-task resolve set) | ancestor-path cycle check of a pull |
| fluxdi wait-for graph (fluxdi note §7) | cycle check across concurrent demands |
| fluxdi cache and in-flight cells (fluxdi note §15 inc 2) | memo; "marked = solved" |
| ADR 0017 `Engine::rank` (`crates/holon-engine/src/engine.rs:254`) | ranking of alternative producers; core of plan checking |
| ADR 0032 §1 read-only places (`:70-82`) | interface places of engines |
| ADR 0032 §2 derived projection, meta-transitions (`:104-171`) | the net as artifact; self-extension |
| ADR 0032 §3 dispatcher gates (`:183-194`) | the effectful path |
| ADR 0032 deferred item 6: relational hop, inhibitor (`:496-504`) | also needed by the planner to read guards |
| `holon_net::evaluate` and `Offer` (`crates/holon-net/src/enabledness.rs:39`, `:62`) | forward enabledness; pull is its backward dual |
| `holon_net::Marking` (`crates/holon-net/src/marking.rs:16`) | read side of the planner's marking |
| ADR 0035 conditions with declared remedies | recovery transitions on the plan frontier |
| boot-always env places (draft §7) | first engine contract |
| ADR 0031 one catalog, two consumers | transition source for planner, dispatcher and simulator |

`holon_net::evaluate` has no production caller yet; only tests call it.

## 9. Nudges for any change near these areas

- [ ] Does the change add a hand-written gate or ready-signal? Prefer a live
      cell or an environment place that the net can read.
- [ ] Does it hide a dependency inside a factory body? Declare it, so a
      planner query can see it.
- [ ] Does a new failure state look like "empty"? Make it a typed `Failed`
      token with a cause and a generation.
- [ ] Does it add a fix for a failure? Make it an operation that a condition
      token enables (ADR 0035), so it appears on a plan frontier.
- [ ] Is a new transition pure or effectful? Declare the kind; give it only
      that kind's path.
- [ ] Does a PN transition write an engine's place directly? Do not; demand
      it from the engine.
- [ ] Does a new subnet (config, sidecar, rule) fail as a whole app? Make it
      validate first and fail alone.
- [ ] Does a new per-thing instance (per document, user, integration) get
      bespoke plumbing? Shape it as a colored instance.
- [ ] Does a new search run forward by trial? Consider a pull first; use
      simulation to check the result.

## 10. Open questions

1. Does a demand to an engine count as a write to a PN place under ADR 0032
   §1? This note assumes no: the engine writes, the net only requests.
2. Pure transition outputs are recomputable. Do they need an ADR 0032 §1
   amendment as "derived places" that owe no durable state?
3. Where does the planner run: in the kernel (Rust), in `holon-engine`, or
   both on one catalog?
4. Is a plan disposable deliberation state (ADR 0024 P2) or vault content?
   Its frontier steps become tasks.
5. Which objective ranks producers in a pull: WSJF (ADR 0017), plan cost, or
   risk from simulation?
6. How does the pull reason about environment places that change with time
   ("shop open until 20:00")? Direction in section 2.5: timed colored tokens
   with a validity interval, a duration per transition, resources as tokens,
   and backward scheduling from the deadline. Still open: how a validity
   interval is stored, how the clock relation (ADR 0024 P5) feeds the pull,
   and which duration the pull uses (expected or pessimistic).
7. How does the pull treat a guard the marking cannot decide
   (`Offer::Unknown`)? Optimistic for planning, fail closed for protective
   arcs (ADR 0032 §3, `:258-266`)?
8. A token can hold a value that is not complete yet. Example: the vault
   index is a fluxdi live cell in state `Partial`; the writable files are
   indexed, the recipes are still loading. The transition "show search
   results" needs the vault index as input. Does it fire now with the
   partial index, or does it wait for the complete one? Proposal: each input
   arc declares it. "Complete only" is the default and fits hard DI
   dependencies. "Accepts partial" fits display. Completeness then flows to
   the outputs: a result computed from a partial input is itself marked
   partial.
9. How does a plan stay current when the marking changes? Incremental
   re-planning (the memo as an IVM view) is not designed.
10. *Answered (Martin, 2026-10-08).* Boot by pull puts a planner on the boot
    path. Direction: the net takes more work, also on the live path
    (section 3). [Simulation.md](Simulation.md) states this direction. ADR
    0031 guard 1 now states this direction.

## Related documents

[Model.md](Model.md) ·
[Simulation.md](Simulation.md) ·
[Engine.md](Engine.md) ·
[Reactivity.md](Reactivity.md) ·
[../Vision/PetriNet.md](../Vision/PetriNet.md) ·
[ADR 0017](../adr/0017-petri-net-task-ranking-engine.md) ·
[ADR 0024](../adr/0024-unified-action-execution.md) ·
[ADR 0031](../adr/0031-native-transition-catalog-and-macro-reification.md) ·
[ADR 0032](../adr/0032-petri-net-execution-semantics.md) ·
[ADR 0035](../adr/0035-conditions-are-one-vocabulary-with-declared-remedies.md) ·
fluxdi live edges: `~/.claude/plans/fluxdi-live-edges.md` ·
boot always (draft): `~/.claude/plans/adr-draft-boot-always.md` ·
recovery layout §8: `~/.claude/plans/holon-recovery-layout-design.md`
