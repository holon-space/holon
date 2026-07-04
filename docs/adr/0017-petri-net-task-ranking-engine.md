# ADR 0017: Model personal task-ranking as a Petri net with Rhai-compiled guards

Status: Accepted (retroactive — documenting shipped architecture)
Date: 2026-07-06

> This ADR is written after the fact to record a decision the code already
> embodies: personal task prioritisation (WSJF) is computed by a small,
> general Petri-net execution engine (`holon-engine`) fed by a domain
> materialiser (`holon-petri`), with all user-authored scoring logic passing
> through a compiled-expression parse boundary (`holon-expr`). It documents
> *why* that shape was chosen over a plain scoring function, and captures the
> one substantive amendment made since (the `AttrInit` injection-safety split).

## Context

Holon needs to answer "what should I do next?" over the user's own task blocks.
The obvious implementation is a scoring function: read each task, compute a
number, sort descending. That is in fact the *output* we want — the shipped
engine ranks by **WSJF = Δobjective / duration**
(`crates/holon-engine/src/engine.rs:229-271`; documented in
`wiki/concepts/petri-net-wsjf.md`). But a flat scoring function cannot express
the constraints that make personal ranking correct:

1. **Dependency-gated availability.** A task blocked on another task (`>`
   sequential sibling, or an explicit `depends_on`) must not be *rankable at
   all* until its blocker is done — not merely scored lower. A scoring function
   has no notion of "not yet enabled".

2. **Completion as state that other tasks observe.** Finishing task A changes
   what B is worth and whether C is even available. This is a *marking change*,
   not a per-row recompute.

3. **Delegation as a waiting cycle.** A delegated task (`@[[Person]]:`) is not
   work *you* do — it produces a "waiting" obligation that only clears when the
   delegate reports back. That is a token that is *created* on delegation and
   *consumed* on completion — a two-phase lifecycle a scalar score can't hold.

4. **Value must be counterfactual.** "How much is doing this worth?" is only
   answerable as *objective(after) − objective(before)* — you have to simulate
   the firing to price it. That is Petri-net execution semantics, not a formula.

These are exactly the things Petri nets model natively: places hold tokens,
transitions are gated by input arcs (preconditions) and produce/consume tokens
(post/create/consume arcs), and a marking is the whole world-state. So rather
than bolt dependency-checking and delegation-tracking onto a scorer as special
cases, we model the whole problem as a net and let ranking fall out of it.

The engine is also reused as a standalone digital-twin simulator (the `/petri`
skill, `.claude/skills/petri-net/SKILL.md`, drives `holon-engine` YAML
scenarios), so it had to be a general net executor, not a task-specific scorer.

## Decision

### 1. A generic Petri-net engine (`holon-engine`) over domain-blind traits

`holon-engine` knows nothing about tasks. It is written entirely against four
traits (`crates/holon-engine/src/lib.rs:23-57`):

- `TokenState` — an identified token with typed attributes (`Value`).
- `TransitionDef` — inputs (`InputArc`), outputs (`OutputArc`), creates
  (`CreateArc`), and a `duration_minutes()`.
- `NetDef` — the set of transitions, the objective expression, constraints, and
  a discount rate.
- `Marking: Clone` — the token population plus a clock; cloneable because
  ranking simulates each candidate firing on a throwaway copy
  (`engine.rs:242`).

`Value` (`crates/holon-engine/src/value.rs`) is a small closed enum
(`Bool/Int/Float/String/Null`) — attributes are typed data, deliberately **not**
`rhai::Dynamic`, so token state stays inspectable and serialisable independent of
the expression engine.

The engine's responsibilities:
- **Enabling + binding** (`engine.rs:42-111`): find, per transition, a token
  assignment satisfying every input arc's preconditions. Binding uses
  **backtracking**, not greedy first-match, because a greedy pass starves later
  arcs of the same token type when an earlier arc grabs the only satisfying
  token (`bind_arcs`, `engine.rs:84-111`).
- **Firing** (`fire`, `engine.rs:114-226`): apply postconditions, create/consume
  tokens, advance the clock by the transition's duration.
- **Ranking** (`rank`, `engine.rs:229-271`): for each enabled binding, clone the
  marking, fire on the clone, evaluate the objective before/after, and sort by
  `Δobj / duration.max(0.001)`, tie-broken by lexicographic transition id for
  determinism.

### 2. Arcs carry *specs*, guards are *compiled Rhai* (`arc.rs` + `holon-expr`)

Input arcs hold a `PrecondSpec` per attribute (`crates/holon-engine/src/arc.rs:53-63`):
`Placeholder("$who")` binds a value, `Comparison{op, rhs}` compares against a
compiled Rhai expression, `Exact(s)` is a literal equality. The comparison
operators are a closed `CmpOp` enum and the spec is **parsed once at net load**
(`PrecondSpec::from_str`, `arc.rs:68-108`) — malformed operators fail loudly
there (`arc.rs` tests `malformed_operators_fail_loudly`), never at fire time.

The objective, constraints, and any `=`-prefixed user expression are
`CompiledExpr` (`crates/holon-expr/src/lib.rs`). This is the load-bearing
**parse-don't-validate boundary**:

- A `CompiledExpr` holds `{ source: String, ast: rhai::AST }`
  (`holon-expr/src/lib.rs:15-18`). It **serialises as the source string** and
  **deserialises by compiling** — `Deserialize` calls `Self::compile` and
  `map_err(serde::de::Error::custom)` (`lib.rs:46-51`), so an objective that
  doesn't compile is rejected the moment a net is loaded from YAML, not the
  moment it is first evaluated. `compile` also strips a leading `=`
  (org-file convention) so stored expressions and inline ones share one type
  (`lib.rs:54-65`).

`holon-expr` exists as its own crate precisely so this compiled-expression type
is the *shared vocabulary* between `holon-api` entity definitions and the
engine's guard evaluator (module doc, `holon-expr/src/lib.rs:1-5`) without either
side depending on the other.

The `RhaiEvaluator` (`crates/holon-engine/src/guard.rs`) is the only place Rhai
runs: `check_precond` (`guard.rs:92-156`) matches a token against an arc,
`eval_postcond` (`guard.rs:180-206`) computes new attribute values, and the
objective/constraint evaluation lives in `objective::evaluate`
(`crates/holon-engine/src/objective.rs`).

### 3. The task domain lives in `holon-petri`, which *materialises* blocks into a net

`holon-petri` (`crates/holon-petri/src/lib.rs`) is the domain adapter. Its job is
`materialize`/`materialize_at` (`lib.rs:828-920`): turn a `&[Block]` into a
`(TaskNet, TaskMarking)`, then `rank_tasks` (`lib.rs:1300-1361`) runs the engine
and returns `RankResult`. The mapping is:

- **Tokens** = the self-person, referenced entities (`[[wiki links]]`),
  completion markers, delegation "waiting" obligations, knowledge from questions
  (`build_*_tokens`, `lib.rs:962-1086`). Token types are strings like `person`,
  `document`, `completion`, `waiting`, `knowledge`.
- **Transitions** = tasks. `build_task_transitions` (`lib.rs:1090-1268`) emits
  **one** transition for a self-executed task and **two** for a delegated one:
  a `{id}_delegate` sub-transition that *creates* a `waiting` token, plus the
  main transition that requires (and `consume`s) that `waiting` token — the
  Petri encoding of "you can't mark it done until the delegate reports back."
  Sequential deps and `depends_on` become input arcs on `completion` tokens
  (`lib.rs:1195-1212`), so a blocked task is simply *not enabled* until its
  blocker's completion token exists.
- **The objective** is generated Rhai summing per-task weights over completion
  tokens (`build_objective_expr`, `lib.rs:1270-1288`), compiled through
  `CompiledExpr`.
- **Prototypal scoring**: a `prototype_for` block supplies defaults and
  `=`-computed weight fields (`priority_weight`, `position_weight`,
  `task_weight`, …). `PrototypeValue` (`lib.rs:278-315`) is `Literal(f64)` or
  `Computed(CompiledExpr)`, parsed at the boundary; `resolve_prototype`
  (`lib.rs:376-436`) merges instance-over-prototype, topo-sorts computed fields
  by dependency, and evaluates them.

Because these are *external, stored* inputs (org-drawer strings, block content),
`holon-petri` parses them fail-loud: `PetriError` (`lib.rs:47-96`) is returned —
never `panic!` — so the live `rank_tasks` MCP tool surfaces bad data instead of
aborting the process. `numeric_prop`/`integer_prop` (`lib.rs:543-572`) reject
non-numeric drawer values rather than defaulting them.

### 4. Amendment: `AttrInit::{Expr, Literal}` — injection-safety for created tokens

Originally a create-arc attribute value was always a Rhai expression string
evaluated at fire time. That is safe for net-authored YAML, but `holon-petri`
*programmatically* builds create arcs whose attribute values include
**user-derived text** — a delegate's name, a task's block id. Splicing a name
containing `"` or `\` into a generated Rhai string literal produced invalid Rhai
(e.g. `"Al"ice\Bob"`) that only failed at fire time, so `rank_tasks` returned
`Err` on perfectly legal names (regression captured by the test
`rank_tasks_tolerates_quotes_and_backslashes_in_names`,
`holon-petri/src/lib.rs:1439-1449`).

The fix makes the create-arc attribute a two-variant enum
(`AttrInit`, `crates/holon-engine/src/arc.rs:187-192`):

- `AttrInit::Expr(String)` — a Rhai expression, evaluated against the bound-token
  scope (the historical behaviour; the untagged serde form YAML nets still
  deserialise to).
- `AttrInit::Literal(Value)` — a **pre-typed value passed straight through as
  data**, never assembled into or parsed as Rhai source (`fire`,
  `engine.rs:182-190`).

The contract (documented at `arc.rs:178-186`): *programmatic net builders MUST
use `Literal` for any user-derived text.* `holon-petri` now carries delegate
names and source-task ids as `AttrInit::Literal(Value::String(...))`
(`lib.rs:1128-1142`, `1216-1231`). This is parse-don't-validate applied to code
generation: user text becomes typed token *data* and can never re-enter the Rhai
parser, so it cannot break out of a literal or inject an expression. Where a
value genuinely must live inside a compiled expression (the objective), it goes
through `rhai_string_literal` (`lib.rs:1008-1023`) which quotes and escapes; and
token ids derived from `EntityUri`s are pushed through `rhai_ident_fragment`
(`lib.rs:997-1001`), with `materialize_at` asserting the mapping stays injective.

## Consequences

- **Ranking is explainable and counterfactual.** Every number is
  `objective(after firing) − objective(before)`, so "why is this first?" is
  answerable by inspecting the simulated marking — not by trusting an opaque
  weight blend. Dependencies and delegation are *structural*, not fudge factors.
- **One engine, two front doors.** The same `holon-engine` powers the live
  `rank_tasks` MCP tool *and* the standalone `/petri` YAML simulator. The domain
  knowledge is quarantined in `holon-petri`; the engine stays task-agnostic.
- **The parse boundary catches bad scoring at load, not at rank time.** A
  malformed objective or prototype expression fails when the block/net is read
  (`CompiledExpr::deserialize`, `PrecondSpec::from_str`), with the offending
  source in the message — not as a silent zero later.
- **Backtracking binding is O(worse) than greedy.** Correctness (not starving
  later arcs) was chosen over the simpler greedy bind; nets with many
  same-typed tokens per transition pay for it. Acceptable at personal-task
  scale; a smell if nets grow large.
- **Cost: Rhai is a real dependency and a real footgun.** The scoring language
  is dynamically typed; `int` vs `float` literal mismatches and undefined-var
  access are live hazards (documented, `wiki/concepts/petri-net-wsjf.md:114-121`).
  Generated Rhai must force float formatting and guard `is_def_var`. Compiling
  per expression also costs; `CompiledExpr` keeps the AST to avoid recompiling
  on every evaluation.
- **`materialize` is a wide, sequential pipeline** (`lib.rs:828-920`, ~90 lines
  of ordered steps). It is the natural seam for future bugs — token-id collision
  (`FragmentCollision`), prototype resolution order, sequential-dep grouping —
  and is where new task semantics land. Kept honest by fail-loud `PetriError`s
  and the entity-URI round-trip test (`lib.rs:1377-1403`).

## Alternatives considered

- **A plain WSJF scoring function.** Rejected: cannot express not-yet-enabled
  (dependency gating) or the delegation waiting-cycle without re-inventing token
  bookkeeping as ad-hoc special cases — i.e. re-inventing a worse Petri net.
- **`rhai::Dynamic` as the token attribute type.** Rejected: couples token state
  to the expression engine and loses clean serialisation. `Value` is a small
  typed enum with an explicit `to_rhai_dynamic`/`from` bridge
  (`value.rs:36-74`) used only at the guard boundary.
- **Storing expressions as raw strings, compiling on use.** Rejected: pushes
  compile failures to evaluation time and re-compiles repeatedly. `CompiledExpr`
  compiles once at the deserialize boundary and keeps the AST.
- **Keeping create-arc values as expression strings everywhere.** Rejected after
  the injection regression — see the `AttrInit` amendment. Expression strings
  remain for YAML-authored nets; programmatic builders must use `Literal`.
