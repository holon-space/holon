# Simulation

*Part of [Architecture](../Architecture.md). Status: design note. The
"Built today" section describes code. The other sections give the direction.*
Direction: [Vision-PetriNetCoordination.md](Vision-PetriNetCoordination.md)
(direction, not a decision). A change to simulation should move toward
checking a plan that a pull made, not toward searching for a plan by trial.

## Terms

- A **place** holds tokens of one kind. Example: the open tasks.
- A **token** is one value in a place. In Holon a token is usually a block or
  an entity row (ADR 0024, Terminology).
- The **marking** is the set of all tokens in all places at one time.
- A **transition** is a step that takes tokens from its input places and puts
  tokens into its output places. When it does this, it **fires**.
- The **dispatcher** runs operations against the real state. It is the only
  path for a real change (ADR 0024 P2, ADR 0032 §3).
- **Simulation** fires transitions on a copy of the marking. The real state
  does not change.
- A **pull** starts at a goal place and works backward to find the
  transitions that can fill it. Its output is a **plan**: transitions in a
  partial order ([vision note §2](Vision-PetriNetCoordination.md#2-the-pull-algorithm)).

## Three uses

Simulation has three uses. They need different tools, so name the use first.

| Use | Candidates | Who scores them | Where the candidates live |
|---|---|---|---|
| **Search** | many (10⁴ and more) | a compiled expression, cheap per step | in memory (`holon-engine`) |
| **Alternatives** | 3 to 10 | an LLM or a human | real blocks in their own subtrees |
| **Preview** | 1 | a human | a copy plus a list of firings; accept = fire them for real |

Four rules follow from this table.

- **Search does not change block text.** Search needs a fixed set of moves and
  a cheap score. Free text has neither: the moves have no limit, and only an
  LLM can score text. An LLM allows about 100 evaluations, not thousands.
- **Search keeps its steps in memory.** Turso has one writer. Many parallel
  runs that write each step to Turso wait on one lock. Turso stores only the
  run's identity (seed, parameters, catalog version) and the winning result.
- **Check the winner twice.** Search runs on the cheap in-memory marking.
  Before Holon shows the winner, it runs the winner once more on the full
  model (a Loro copy for blocks) and computes the score again. If the two
  scores differ, that is probably a bug.
- **Alternatives are normal content.** Research drafts and candidate plans
  are kept, linked and searched, so they are real blocks. An agent writes them
  under its own subtree, for example `research/<topic>/<agent>`. `OpOrigin`
  records who wrote them (`crates/holon-api/src/operation_engine.rs:37`).
  Only Preview keeps its changes away from the real state until a human
  accepts them.

## Built today

- `holon-engine` is a small Petri-net engine. `Engine::enabled` finds the
  transitions that can fire (`crates/holon-engine/src/engine.rs:49`).
  `Engine::fire` fires one and moves the marking's clock forward by the
  transition's duration (`engine.rs:122`, clock at `:222-240`).
- `Engine::rank` fires each enabled transition once, on a copy of the
  marking. It sorts them by change in score per minute (WSJF, ADR 0017)
  (`engine.rs:254`). The score is an expression from the net file
  (`ObjectiveDef`, `crates/holon-engine/src/yaml/net.rs:31`).
- `holon-petri` turns task blocks into a net and a `TaskMarking`
  (`crates/holon-petri/src/lib.rs:258`, `materialize` at `:1040`).
  `rank_tasks` ranks them (`lib.rs:1514`). The MCP tool `rank_tasks` calls it
  (`frontends/mcp/src/tools.rs:3103`). This is a one-step look ahead, not a
  search.
- The `holon-engine` command line has `simulate` (fire the best transition,
  N times) and `whatif` (fire one transition on a copy)
  (`crates/holon-engine/src/main.rs:89`, `:113`). These work on YAML nets, not
  on the vault.
- The trust gate can turn an operation into a proposal instead of a real
  change (`TrustDecision::Propose`, `crates/holon-profiles/src/trust.rs:71`).
  This is a first part of Preview.
- ADR 0031 derives one transition catalog from the operation definitions. A
  test checks that a declared transition writes only the places it declares
  (`crates/holon-integration-tests/tests/catalog_suite/arc_marking_equality.rs`).

Not built: the pull, a check of a multi-step plan, random durations, the
preview store (a copy plus a list of firings), and copies of external systems
(Todoist, calendar) for simulation.

## Direction: the net coordinates, simulation checks the plan

The Petri net gets more work. It becomes Holon's coordination layer, also on
the live path. It decides what must happen before a window shows a session,
why an operation is not available, and what the user must do next. The
engines (fluxdi, Turso IVM, Loro) still do the work inside
([vision note §1, §5](Vision-PetriNetCoordination.md#1-the-coordination-layer-idea)).
One model for boot, operations and plans is simpler to understand. It also
lets data extend Holon: a vault block can declare a new subnet
([vision note §7](Vision-PetriNetCoordination.md#7-the-kernel-net-and-self-extension)).

In this direction, simulation does not search for plans. A pull finds the
plan; it goes backward from the goal and visits only the places the goal
needs. Simulation then **checks** that plan against what is not certain:

- **Durations.** Cooking can take 30 or 50 minutes.
- **Availability.** The shop closes at 20:00. A meeting can run late.
- **Risk.** How often does the plan miss its deadline?

The simulator fires the plan many times on a copy of the marking, with
random durations. It reports the risk ("buy the chicken at lunch, not after
work"). It can also compare the two or three plans that the pull ranked.
Search stays for problems that have no clear goal place, for example the
best schedule for all open tasks.

Three rules stay:

- Simulation never writes the real state. It works on a copy.
- A plan becomes real only when its transitions fire through the dispatcher
  (ADR 0024 P2). Holon never copies simulator state back.
- The planner, the dispatcher and the simulator read one catalog
  (ADR 0031). With two catalogs, the check above compares a declaration with
  itself and proves nothing.

One rule changes. ADR 0031 (guard 1, `docs/adr/0031-native-transition-catalog-and-macro-reification.md:74`)
says: "No PN runtime in the live dispatch path." In the direction, the net
does run on the live path: the planner on boot, and pure transitions
inline. Effectful transitions still go through the dispatcher
([vision note §4](Vision-PetriNetCoordination.md#4-pure-and-effectful-transitions)).
ADR 0031 guard 1 is still the binding text until an ADR changes it.

## Examples by use

Find the use of a new idea before you design for it.

**Plan check (a pull makes the plan, simulation checks it):**
- "Cook Chicken Teriyaki tonight": is there enough time to buy the chicken
  before the shop closes?
- Boot: which remedy does the recovery screen offer first?
- "What if I take on this project": how likely is it that the deadlines
  hold? An LLM estimates effort and durations once and stores them as
  attributes. The compiled engine then runs the many what-ifs. The LLM is
  never inside the loop.

**Search (a compiled score, many candidates):**
- Plan the `SCHEDULED` dates of all open tasks under deadlines, capacity
  and dependencies.
- Load of connected systems (Todoist tasks, calendar density), scored before
  Holon sends anything.

**Alternatives (few candidates, an LLM or a human decides):**
- Deep research: several agents work in their own subtrees. Pick the best
  result or merge them.
- Two or three ways to reorganize the open work. The user picks one. The
  others are archived, not reverted.
- Draft variants of a section as sibling subtrees.

**Preview (one candidate, review, then accept):**
- An agent's changes, shown on a Loro copy before Holon trusts its writes.
- A held external change, shown on the copy of the external system ("task X
  will be done in Todoist"). Accept fires it. Reject never calls the API.
- Bulk changes: archive everything done before a date, rename a tag
  everywhere.

Add new examples with their use.
