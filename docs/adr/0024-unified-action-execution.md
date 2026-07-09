# ADR 0024: Unified action execution — Petri-net surface, block substrate, dual-evaluated guards

**Status:** Proposed (2026-07-09). Distilled from a first-principles session with
Martin ("holon: Actions vs Petri-Net"); the principles below were developed and
provisionally agreed in that discussion. The leaning "one rule language = Petri
nets" is a **declared target, not yet ratified as implementation commitment**;
the Phase-1 work package is decision-invariant either way.
**Deciders:** Martin (+ discussion session)
**Relates to:** [ADR 0017](0017-petri-net-task-ranking-engine.md) (the Petri-net
engine this ADR re-scopes into semantics + simulator),
[ADR 0022](0022-runtime-definable-advice-rules.md) (rules-as-vault-blocks,
synthesized matviews, suppression anti-join — the closest existing relative of the
unified rule engine), [ADR 0023](0023-two-stage-relevance-app-layer-reranker.md)
(retrieval/rerank split the deliberative layer mirrors),
`docs/Architecture/Model.md` (five layers, invariants 1–12 — this ADR adds no new
authoritative store and routes every effect through the intent boundary).
Vault altitude view: `holon-pkm/Projects/Holon/Engine Foundations.org`, headline
`:ID: deliberative-layer-vision`.

## Context

Holon has **three** independent "when a condition holds, run an effect" systems,
built on different substrates and unaware of each other:

1. **Query-actions** (`crates/holon/src/api/action_watcher.rs`): a trigger query
   block + an `action` DSL block in the vault; fired reactively via matview CDC,
   or as a boot-time one-shot for tableless triggers (`date('now')`). Dedup is
   nothing but `INSERT OR IGNORE`; temporal triggers never re-fire.
2. **Advice rules** (ADR 0022): vault-block-defined rules compiled to one
   synthesized matview each, with a suppression anti-join; effect = display
   placement.
3. **Petri-net engine** (`crates/holon-engine`, ADR 0017): transitions with Rhai
   guards over token markings; event-sourced `state.yaml`/`history.yaml`; today a
   standalone CLI + `holon-petri` materializer (WSJF ranking) — it cannot execute
   holon operations and is not wired into the app.

Three worked-example bugs from the journal auto-create rule (2026-07-09 dogfood)
exposed the missing semantics: action-rule blocks confused for renderable query
sources; `date('now')` hacked as a boot one-shot; "one journal per day" leaning on
accidental effect idempotence. Meanwhile the vault archive ("Query-Triggered
Actions") had already drafted — but never built — action scopes
(Local / Once / Owner-only) and an execution-log dedup.

The open question: one action machine, or several? And on what substrate?

## First principles (the decision drivers)

**P1a — One authoritative state: the block tree.** No engine may own a second
authoritative store. Private state (`state.yaml` as the home of markings) does
not replicate, does not merge, is invisible to the vault and to queries. Nets,
rules, tokens, leases: all vault data (blocks/edges). The "CRDT for the Petri
net" is then simply the existing block CRDT.

**P1b — One semantics, pluggable evaluators.** Guard/enabledness semantics are
defined once, declaratively, over blocks. Evaluation strategy is per-context:
when the app pipeline runs, token-blocks are projected into Turso anyway
(projection is total), so incremental evaluation via matview + CDC is free; in a
standalone context (CLI, embedded, Turso absent) the same guards evaluate
in-memory over deserialized blocks. Two *evaluators* are fine; two *semantics*
are forbidden.

**P2 — Reaction and deliberation are layers, not rivals.** Reactive firing is
always-on, incremental, current-state-only. Deliberation (simulate / what-if /
rank) forks hypothetical worlds from derived snapshots and may become arbitrarily
intelligent — the product vision is AlphaGo-shaped: a learned heuristic guiding
search over possible futures. Deliberation state is derived and disposable; both
layers act on the vault **only** through the operation-intent boundary
(Model.md invariant 3/4). Committing a plan = firing real transitions, never
copying simulator state back.

**P3 — One user-facing rule language, graduated complexity.** The user must
never face a "query-action or Petri net?" decision, nor a refactoring cliff when
a simple rule grows. A simple action is a *degenerate net* — one transition
(guard + effect), authored via sugar as terse as today's query/action block
pair. Growing it means adding places/transitions around it, not porting it.

**P4 — The effect taxonomy governs firing discipline, not the engine.**
- *Internal / idempotent effects* (block CRUD): convergent **by construction**
  via deterministic effect IDs — UUID-shaped (name-based, e.g. UUIDv5 of
  rule-id + firing key) so users never suspect they are meaningful or editable.
  Two replicas firing the same rule for the same key produce the same block; the
  merge collapses them. At-most-once-per-key becomes a naming discipline, not an
  execution-semantics problem.
- *External / once-only effects* (send email, call API): no CRDT can un-send two
  emails. Exactly-once world effects are impossible in a leaderless
  eventually-consistent system; they require **asymmetry** — an ownership/lease.
  In net vocabulary the lease is naturally *a token in a place* (the send
  transition consumes the executor token). Partitions can still mint dual
  owners; leases need TTL + reconciliation, stated honestly rather than hidden.
- An execution log is **not** a cross-replica dedup mechanism (each replica's
  log races the others'); it is demoted to bookkeeping for Once/Owner-only
  scopes.

**P5 — Time is data.** A clock relation (a `today` row advanced by a scheduler;
finer grains later) makes temporal guards ordinary reactive guards. This deletes
the `is_tableless` boot-one-shot branch, fixes day-rollover, and moots Turso's
rejection of non-deterministic views. Both replicas advancing the clock
independently is harmless because internal effects are convergent (P4).

**P6 — Program is data, but not display content.** Rules/nets are vault blocks
(replicated, runtime-definable — ADR 0022 precedent) carrying a marking that
exempts them from content rendering. The journal-rule render bug is exactly this
marking missing for action blocks.

**P7 — Token consumption is linearity made explicit — and it is a replicated
write.** With place = parent block and token = child block, "a token is in
exactly one place" is precisely the tree CRDT's one-parent invariant: concurrent
consumption = concurrent `move_block`s, resolved to a single winner
deterministically by machinery already trusted. The *marking* converges by
construction; the *effect* side of the losing replica is governed by P4.

**P8 — Firing history is block history plus provenance.** Block history already
exists per substrate with a fidelity ladder mirroring the merge ladder: Loro op
history ≻ jj/git commit history ≻ none (a Turso audit table can only ever be a
cache — Layer 3 is ephemeral by contract). What history alone cannot answer is
*why*: ops executed by the engine are stamped with provenance
(`fired-by: <transition-id>` — the same slot future device/author attribution
uses; "serializable ops with provenance" is already a kept-warm invariant,
Model.md §Offline). Then:
- *forensic/debug history* = substrate history filtered by provenance,
  best-effort, mode-dependent, disclosed;
- *user-facing automation journal* = the provenance-stamped effects themselves —
  uniform across modes — rendered by a **query**, not stored as a log.
Simulation traces are engine-local scratch, never history.

## Decision

**Target: one rule system.** Petri nets (colored — tokens carry data) are the
single user-facing action language; query-actions and advice rules are
re-understood as degenerate/adjacent forms of it, per the mapping below. The
existing `holon-engine` is re-scoped from "rival state owner" to
**semantics + in-memory evaluator + simulator**.

| Concept | Substrate |
|---|---|
| Net / transition / rule definition | vault blocks (program-marked, P6) |
| Place | parent block (or edge-typed relation for light markings) |
| Token (= Digital Twin) | child block; moves via `move_block` (P7) |
| Guard / enabledness | `Pattern`/`Predicate` AST (below), dual-evaluated (P1b) |
| Firing (reactive) | matview `Change::Created` → effect via `execute_operation` |
| Firing (standalone) | in-memory evaluator in `holon-engine` |
| At-most-once (internal) | deterministic effect IDs (P4) |
| At-most-once (external) | lease token + TTL + reconciliation (P4) |
| History | block history + `fired-by` provenance; journal = a query (P8) |
| Deliberation | simulator forks in-memory worlds from the serialized net subtree |

**Guard language: a dual-evaluated Pattern AST, not Rhai, not free-form
PRQL/SQL.** The precedent exists in the codebase in two halves:
`holon_api::Predicate` (`crates/holon-api/src/predicate.rs` — prod, serializable,
in-memory `evaluate()`, scalar-only) and the PBT query AST
(`crates/holon-integration-tests/src/pbt/query_ast.rs` — relational vocabulary
`PropEq`/`Membership`/`EdgeExists{negated, inner}` with BOTH in-memory evaluation
and `compile_to_sql`). Unify: promote the relational vocabulary into a prod
`Pattern` type (predicates + variable *bindings* — a guard must say which
token/row matched, not just true/false) with `evaluate()` and `to_sql()`.
Rationale:
- the **agreement oracle already exists** — the PBT query machinery was built to
  check in-memory ≡ SQL; promoting the AST carries the oracle along as a pinned
  prod invariant;
- the AST is **designed to the IVM-supported subset** — unsupportable guards fail
  at parse, not at matview-DDL time (parse-don't-validate; the `date('now')`
  failure class disappears). `NOT EXISTS`/anti-join — the construct consumption
  and suppression need — is proven maintainable in the Turso fork (ADR 0022);
- guards stay **legible** — serializable for rules-as-blocks, inspectable by the
  planner, queryable by advice, executable by the simulator. Rhai strings are
  opaque blobs by comparison.
Caveat: the PBT AST's `to_sql` hardcodes current schema shapes
(`json_extract` on properties, `block_tags`); promotion lifts the *design*, and
the compiler must target the projection's schema abstraction. Rhai's residual
role, if any, is computed value construction inside *effects* — never in the
matching path.

**Advice: views, not events.** Advice is a continuous view (retracting when the
pattern stops holding); an action firing is an edge-triggered irreversible
event. Both compile onto the same substrate and share the rule *definition
format and lifecycle*, but advice does not get firing/consumption semantics
forced onto it. "One rule language" = one authoring model with two effect
kinds: `advise` (view) and `operate` (transition).

## What this rejects

- **B — shared effect API only, two matchers kept:** accepts permanent drift of
  two matching languages, two persistence models, two trigger semantics, and the
  user-facing "which engine?" cliff (P3 violation).
- **Petri-canonical on a private substrate:** the engine as authoritative state
  owner violates P1a (no replication/merge/visibility) and Layer 4 ("convergent
  state, not an event log").
- **Places = Turso relations directly:** ties action execution to Turso being
  up; token-as-block keeps the model mode-independent (Turso is just the free
  incremental evaluator where it exists).
- **Execution-log-as-dedup:** races across replicas; see P4.

## Consequences & risks

- **Write-path pressure:** every token move traverses consolidator → projection
  (→ org writeback). Assumption stated here: PN firings are human-timescale
  (planning workflows), not hot loops. Mitigations if violated: net-instance
  state in a document the org adapter does not materialize (replica
  participation is per-replica), plus the incremental-projection track.
- **Token granularity** needs a rule of thumb per place-type: full child block
  vs edge/property marking (mirrors existing per-field fidelity degradation).
- **Compilation is real work** (net → matviews + intent wiring). Staged so the
  degenerate one-transition case (≈ today's query-actions) lands first;
  `action_watcher` is eventually re-understood as the compiled output of a
  one-transition net and deleted as a separate machine.
- **Simulator fidelity:** for vault-internal effects the compiler that makes
  nets executable also makes them simulable (the effect's marking-delta is
  derivable); external effects simulate lease/token movement only.

## Amendment (2026-07-09, same session): effects are token operations

Ratified direction from Martin's demo review: for intra-Holon effects, the
effect DSL is **not** the authoring surface — **transitions declare marking
deltas, and the created/consumed blocks are themselves the tokens**:

- `create` = **output arc** (the emitted token *is* the new block);
- `delete` = **input arc** (consume);
- `update` = consume + emit with **identity preserved** — a colored-token value
  update, compiled to `update`/`set_field` intents, **never** delete+create
  (references, CRDT history, and provenance must survive);
- `move` = consume-from-place-A + emit-into-place-B, compiled to `move_block`;
- guard context = **read arcs** (test without consuming);
- negative conditions (`not journal(date = today)`) = **inhibitor arcs** — the
  suppression/at-most-once anti-join seen from the net side. A create-rule is
  typically inhibited by *its own output*, so it self-disables after firing:
  at-most-once-per-key becomes plain enabledness semantics, not a bolted-on
  firing discipline. (Disclosed: inhibitor arcs extend classic PNs —
  Turing-completeness, weaker reachability analysis — relevant to the
  deliberative layer's static reasoning.)

Guard surface vs compilation (revised after Martin's review): builtins like
`{today}` / `{clock.today}` are **environment references, interpolated** — not
pattern variables — so the author writes `when: not
block_exists("Journals/{today}")` with no explicit binding. The compiler
desugars each builtin reference into a read arc on the corresponding
clock/environment relation (this is what makes the rule re-fire on rollover
and keeps the compiled matview deterministic, per P5). The Datalog
range-restriction check (every pattern variable bound by a positive atom
before appearing under negation) remains as an internal well-formedness rule;
it surfaces only for user-introduced pattern variables, with a plain-language
error, never for builtins.

Why this beats side-effect-style `block.create(...)` transitions:
1. **Simulability for free** — the marking delta *is* the declaration; the
   "simulator fidelity" consequence above dissolves for all intra-Holon rules.
2. **Analyzability** — "at most one journal per day" is a checkable place
   invariant, not a hoped-for runtime property.
3. **Invertibility** — undo = reversed arcs (the kept-warm inverse-ops
   invariant).
4. **Convergence** — deterministic emission IDs (P4) attach to output arcs.

Disclosed extensions/residue: places are ordered in Holon (output arcs carry an
`after` hint; classic places are multisets); "place" generalizes to relation so
typed edges are tokens too; output-token value construction needs a small
expression language (the residual Rhai/expr slot — confined to color functions,
never in matching). The `block.*` operation DSL remains as the **compilation
target** at the intent boundary, not a user surface. External side-effects are
untouched by this amendment (P4 lease taxonomy).

Rule blocks get a `holon_rule` source language (family of
`holon_advice_rule_yaml`, superseding the bare `action` language) — this is
what the P6 program-marking keys off. Rule bodies are **valid YAML**, with
guard expressions as strings parsed by the Pattern parser.

## Phases

**Phase 1 — decision-invariant fixes (valid under every outcome above):**
1. *Time-as-data:* clock relation + scheduler; delete the `is_tableless`
   one-shot branch of `action_watcher`.
2. *Deterministic effect IDs:* action `block.create` mints name-based
   UUID-shaped IDs from rule-id + firing key (subsumes the `create: missing
   'id'` fix, PR #33 lineage).
3. *Program marking:* action/trigger blocks excluded from content rendering
   (fixes the journal-rule render bug; BugFunnel entry per `bug-gap-triage`).

**Phase 2 — guard language:** promote the dual-evaluated `Pattern` AST
(unify `holon_api::Predicate` + PBT `query_ast` vocabulary), with the
in-memory ≡ SQL agreement property as a prod invariant.

**Phase 3 — rules on the unified substrate:** one rule definition format +
discovery/lifecycle (generalizing ADR 0022) with effect kinds `advise` |
`operate`; provenance stamping (`fired-by`) on engine-executed ops; automation
journal as a query.

**Phase 4 — nets:** token-as-block markings (place = parent, token = child,
consumption = `move_block`); compile transitions to matviews (reactive) and to
the in-memory evaluator (standalone); re-scope `holon-engine` accordingly;
lease tokens for Once/Owner-only scopes.

**Phase 5 — deliberation:** simulator over serialized net subtrees; ranking /
what-if as advice; the AlphaGo-shaped heuristic-guided search is the long-term
differentiator (vault: `deliberative-layer-vision`).
