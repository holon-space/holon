# ADR 0024: Unified action execution — Petri-net surface, block substrate, dual-evaluated guards

**Status:** Accepted (ratified by Martin 2026-07-10, after two inline-review
rounds — marking styles, firing-key definition, lease override, precise CPN
terms, unified output model). Implementation plan:
`docs/Proposals/action-unification-implementation-plan.md`; execution began
2026-07-10 with the Phase-2 risk-elimination spike + Phase-1 work packages.
**Deciders:** Martin (+ discussion session "holon: Actions vs Petri-Net")
**Relates to:** [ADR 0017](0017-petri-net-task-ranking-engine.md) (the Petri-net
engine this ADR re-scopes into semantics + simulator; its flat-net v0.3 model —
see `.claude/skills/petri-net/SKILL.md` — supplies the default marking style),
[ADR 0022](0022-runtime-definable-advice-rules.md) (rules-as-vault-blocks,
synthesized matviews, suppression anti-join — the closest existing relative of
the unified rule engine),
[ADR 0023](0023-two-stage-relevance-app-layer-reranker.md) (retrieval/rerank
split the deliberative layer mirrors), `docs/Architecture/Model.md` (five
layers, invariants 1–12 — this ADR adds no new authoritative store and routes
every effect through the intent boundary).
Vault altitude view: `holon-pkm/Projects/Holon/Engine Foundations.org`, headline
`:ID: deliberative-layer-vision`. UX companion:
`docs/Proposals/action-ux.md`.

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
3. **Petri-net engine** (`crates/holon-engine`, ADR 0017): a general net
   executor over domain-blind traits, used by the `holon-petri` materializer
   (WSJF ranking) and the `/petri` skill as a standalone digital-twin
   simulator. Its `state.yaml`/`history.yaml` files are **simulation scenario
   artifacts** (initial state + replay for what-if runs), not app state; the
   engine cannot execute holon operations and is not wired into the app.

Three worked-example bugs from the journal auto-create rule (2026-07-09 dogfood)
exposed the missing semantics: action-rule blocks confused for renderable query
sources; `date('now')` hacked as a boot one-shot; "one journal per day" leaning
on accidental effect idempotence. Meanwhile the vault archive ("Query-Triggered
Actions") had already drafted — but never built — action scopes
(Local / Once / Owner-only) and an execution-log dedup.

The open question: one action machine, or several? And on what substrate?

## Terminology (used consistently below)

Colored-Petri-net terms, aligned with the engine's flat-net model:

- **Token** — an identified value with typed attributes. In Holon a token *is a
  block* (the Digital Twin).
- **Place** — where tokens of one color live. In the engine's flat-net model
  places are *implicit*: one per `token_type` (tokens never change type).
- **Input arc** — matches (and optionally **consumes**) a token, with per-arc
  **binding preconditions** (arc expressions).
- **Read arc** — tests without consuming (guard context, e.g. the clock).
- **Inhibitor arc** — a negative condition ("no such token exists").
- **Transition guard** — an optional boolean over the *whole binding* (formally
  in CPN the guard sits on the transition; arc expressions sit on arcs).
- **Output arc** — emits a token or updates a bound token's attributes
  (postconditions).
- **Firing** — consuming/emitting per the arcs; a **simple action = 1
  transition + M input arcs with binding preconditions (+ optional transition
  guard) + N output arcs + at most one external side effect.**
- **Binding / firing key** — the assignment of tokens/values to the
  transition's input and read arcs that enables a firing, canonically
  serialized (journal rule: `{today: 2026-07-10}`).

## First principles (the decision drivers)

**P1a — One authoritative state: the block tree; markings are *derived*, not
stored.** No engine may own a second authoritative store. Tokens are blocks, so
in the flat-net model the marking is exactly "which blocks exist with which
attributes" — already replicated, merged, visible, and queryable; there is
nothing extra to persist. The one residue is a token currently held by an
in-flight *timed* transition, encoded as an `in-transition` attribute/edge on
the block itself. The "CRDT for the Petri net" is then simply the existing
block CRDT. (`state.yaml` remains what it is today: a simulator scenario file,
never architecture.)

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
a simple rule grows. A simple action is a *degenerate net* — one transition per
the terminology above — authored via sugar as terse as today's query/action
block pair. Growing it means adding arcs and transitions, not porting to
another system.

**P4 — The effect taxonomy governs firing discipline, not the engine.**
- *Internal / idempotent effects* (block CRUD): convergent **by construction**
  via deterministic effect IDs, minted **per emitted token**:
  `UUIDv5(HOLON_RULE_NS, rule-id ‖ firing-key ‖ output-slot ‖ [element-index])`.
  The firing key is the transition's binding (see Terminology); the output-slot
  discriminator makes multi-output transitions collision-free (a today-page
  *template* emitting several blocks mints several distinct ids); the
  element-index covers variadic expansion within one slot. IDs are UUID-shaped
  (name-based) so users never suspect they are meaningful or editable. Two
  replicas firing the same rule for the same binding produce the same blocks;
  the merge collapses them. At-most-once-per-key becomes a naming discipline,
  not an execution-semantics problem.
- *External / once-only effects* (send email, call API): no CRDT can un-send two
  emails. Exactly-once world effects are impossible in a leaderless
  eventually-consistent system; they require **asymmetry** — an ownership/lease.
  In net vocabulary the lease is naturally *a token* (the send transition
  consumes/holds the executor token). Partitions can still mint dual owners;
  leases need TTL + reconciliation, stated honestly rather than hidden.
  **Manual override is a requirement, not a violation:** a user on the road
  with only their phone must be able to take the lease from the computer that
  holds it ("take over on this device"). The takeover is itself vault data (a
  new lease epoch); if the old holder had in-flight effects during a partition,
  duplicates are possible — disclosed, and surfaced in the automation journal
  for reconciliation. When the user explicitly overrides, availability beats
  exclusivity.
- An execution log is **not** a cross-replica dedup mechanism (each replica's
  log races the others'); it is demoted to bookkeeping for Once/Owner-only
  scopes.

**P5 — Time is data.** A clock relation (a `today` row advanced by a scheduler;
finer grains later) makes temporal guards ordinary reactive guards. This deletes
the `is_tableless` boot-one-shot branch, fixes day-rollover, and moots Turso's
rejection of non-deterministic views. The relation is an *evaluator detail of
the reactive path* — a boot-reseeded cache of the OS clock, never authoritative;
the standalone evaluator desugars the same builtin to a direct `Clock::now()`
call. Both replicas advancing their clocks independently is harmless because
internal effects are convergent (P4).

**P6 — Program is data, but not display content.** Rules/nets are vault blocks
(replicated, runtime-definable — ADR 0022 precedent) carrying a marking that
exempts them from content rendering. The journal-rule render bug is exactly this
marking missing for action blocks.

**P7 — Two marking styles; workflow state must not disturb content placement.**
- *Attribute-marking (the default; = the engine's flat-net model):* the entity
  block **is** the token; lifecycle lives in attributes/edges (`status`,
  `in-transition`, typed edges). Tokens never move in the tree, so a person
  block stays under "work" no matter which workflows it participates in.
  Convergence is per-field merge (per-property LWW / CRDT); firing effects are
  convergent when postconditions are pure functions of the binding
  (deterministic per firing key).
- *Position-marking (only where position IS the semantics):* e.g. Kanban
  boards — column = place = parent block, card = token, movement/consumption =
  `move_block`. There the tree CRDT's one-parent invariant is exactly the
  linearity primitive: concurrent grabs of one token resolve to a single winner
  deterministically, by machinery already trusted.
- *Reference-tokens (escape hatch):* when one entity participates in several
  nets with conflicting marking needs, mint a small token block per
  (net-instance, entity) holding a *reference* to the Digital-Twin block — one
  content block per entity, any number of workflow markers.
In every style, the *marking* converges by construction; the *effect* side of a
losing replica is governed by P4.

**P8 — Firing history is block history plus provenance.** Block history already
exists per substrate with a fidelity ladder mirroring the merge ladder: Loro op
history ≻ jj/git commit history ≻ none (a Turso audit table can only ever be a
cache — Layer 3 is ephemeral by contract). What history alone cannot answer is
*why*: ops executed by the engine are stamped with provenance
(`fired-by: <transition-id>` — the same slot future device/author attribution
uses; "serializable ops with provenance" is already a kept-warm invariant,
Model.md §Offline). Then:
- *forensic/debug history* — "why does this block have this state?" → filter
  the substrate's history by provenance down to the exact firing (e.g. Loro op
  history shows `set_field status=delegated, fired-by: rule:delegate-work` with
  its causal context; in an org-only vault the same question degrades to
  jj/git commit granularity). Best-effort, mode-dependent, disclosed.
- *user-facing automation journal* — the provenance-stamped effects themselves,
  uniform across modes, rendered by a **query**, not stored as a log: the
  Automations page is `effects grouped by rule and day` ("Daily journal —
  created '2026-07-10' at 00:03 ⚙").
Simulation traces are engine-local scratch, never history.

## Decision

**Target: one rule system.** Colored Petri nets — in the terminology above —
are the single user-facing action language; query-actions and advice rules are
re-understood as degenerate/adjacent forms of it. The existing `holon-engine`
is re-scoped from "rival state owner" to **semantics + in-memory evaluator +
simulator**.

| Concept | Substrate |
|---|---|
| Net / transition / rule definition | vault blocks (program-marked, P6; `holon_rule` source language) |
| Place | implicit per token type (attribute-marking, default) — or a parent block where position is the semantics (boards) |
| Token (= Digital Twin) | the entity block itself; reference-token blocks for multi-net participation |
| Marking | derived from block existence + attributes (`in-transition` for mid-flight timed transitions); never a private store |
| Guard / enabledness | `Pattern` AST (below), dual-evaluated (P1b) |
| Firing (reactive) | matview `Change::Created` → effect via `execute_operation` |
| Firing (standalone) | in-memory evaluator in `holon-engine` |
| At-most-once (internal) | deterministic per-output-token IDs (P4) |
| At-most-once (external) | lease token + TTL + reconciliation + user override (P4) |
| History | block history + `fired-by` provenance; journal = a query (P8) |
| Deliberation | simulator forks in-memory worlds from serialized blocks |

### Effects are token operations

For intra-Holon effects, the effect DSL is **not** the authoring surface —
**transitions declare marking deltas, and the created/consumed/updated blocks
are themselves the tokens**:

- `create` = **output arc** (the emitted token *is* the new block);
- `delete` = **consuming input arc**;
- `update` = bound-token attribute postconditions — a colored-token value
  update with **identity preserved**, compiled to `update`/`set_field` intents,
  **never** delete+create (references, CRDT history, and provenance must
  survive);
- `move` (position-marking only) = consume-from-place-A + emit-into-place-B,
  compiled to `move_block`;
- guard context = **read arcs**; negative conditions
  (`not block_exists("Journals/{today}")`) = **inhibitor arcs** — the
  suppression/at-most-once anti-join seen from the net side. A create-rule is
  typically inhibited by *its own output*, so it self-disables after firing:
  at-most-once-per-key becomes plain enabledness semantics. (Disclosed:
  inhibitor arcs extend classic PNs — Turing-completeness, weaker reachability
  analysis — relevant to the deliberative layer's static reasoning.)

Why this beats side-effect-style `block.create(...)` transitions:
1. **Simulability for free** — the marking delta *is* the declaration; the
   simulator needs no separate effect models for intra-Holon rules.
2. **Analyzability** — "at most one journal per day" is a checkable invariant,
   not a hoped-for runtime property.
3. **Invertibility** — undo = reversed arcs (the kept-warm inverse-ops
   invariant).
4. **Convergence** — deterministic per-output IDs (P4) attach to output arcs.

Disclosed extensions/residue: Holon's sibling sets are ordered (output arcs
carry an `after` hint; classic places are multisets); typed edges are tokens in
relation-places; output-token value construction needs a small expression
language (the residual Rhai/expr slot — confined to postconditions, never in
matching). The `block.*` operation DSL remains as the **compilation target** at
the intent boundary, not a user surface. External side-effects follow the P4
lease taxonomy.

### Guard surface vs compilation

Builtins like `{today}` / `{clock.today}` are **environment references,
interpolated** — not pattern variables — so the author writes
`when: not block_exists("Journals/{today}")` with no explicit binding. The
compiler desugars each builtin reference into a read arc on the corresponding
clock/environment relation (reactive path) or a direct `Clock` call (standalone
path). The Datalog range-restriction check (every pattern variable bound by a
positive atom before appearing under negation) remains as an internal
well-formedness rule; it surfaces only for user-introduced pattern variables,
with a plain-language error, never for builtins.

Rule blocks get a `holon_rule` source language (family of
`holon_advice_rule_yaml`, superseding the bare `action` language) — this is
what the P6 program-marking keys off. Rule bodies are **valid YAML**, with
guard expressions as strings parsed by the Pattern parser.

**Guard language: a dual-evaluated Pattern AST, not Rhai, not free-form
PRQL/SQL.** The precedent exists in the codebase in two halves:
`holon_api::Predicate` (`crates/holon-api/src/predicate.rs` — prod,
serializable, in-memory `evaluate()`, scalar-only) and the PBT query AST
(`crates/holon-integration-tests/src/pbt/query_ast.rs` — relational vocabulary
`PropEq`/`Membership`/`EdgeExists{negated, inner}` with BOTH in-memory
evaluation and `compile_to_sql`). Unify: promote the relational vocabulary into
a prod `Pattern` type (predicates + variable *bindings* — a guard must say
which token/row matched, not just true/false) with `evaluate()` and `to_sql()`.
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
role, if any, is computed value construction inside *postconditions* — never in
the matching path.

### Advice is emission into display placement (one output model)

A deeper unification than "two effect kinds" (ratified in discussion,
2026-07-10): **advice is also an `emit` of blocks — they just are not
persisted.** Holon already has the exact concept that distinguishes the two:
ADR 0015's **canonical vs display placement**. So the view-vs-event split is
not a keyword on the input side; it is the **place kind of the emission**:

- emission into a **canonical place** (`place: journals`) is **ratcheted** —
  the block persists once fired (event semantics);
- emission into a **display place** (`place: display(under: x)`) is
  **maintained** — rows retract automatically when the binding disappears
  (view semantics; the IVM maintains it).

Every rule "fires per matching binding" — that is plain PN enabledness, and the
reactive evaluator implements it literally (one matview row per binding;
`Change::Created` = a new binding appeared). A separate `when:` vs `for:`
keyword pair was considered and **rejected**: it duplicated on the input side a
distinction that belongs to the output. One parser-enforced invariant replaces
it: **consuming input arcs ⇒ ratcheted outputs only** — consumption is a state
change, and a "view that consumes" is incoherent. Views are read-arc-only,
which *derives* "advice has no firing/consumption semantics" instead of
asserting it.

**Canonical form and sugar.** The canonical rule form is arc-array structured —
deliberately close to the existing engine `net.yaml` (ADR 0017 / petri-net
skill): `input:` arcs with `bind` / `type` / `when` (binding preconditions) /
`consume` / `absent` (inhibitor), `output:` arcs with emission expressions.
Per P3 the simple case keeps a terse **sugar form** (top-level `when:` +
`emit:`) *defined by its desugaring* into the canonical form: one non-consuming
input arc per free entity reference, `not <exists-pattern>` → an inhibitor arc
(`absent: true`), a single output arc.

The related-lessons rule (today's `AdviceRule` with `anchor: has_tag` +
`tag_overlap_recency`, re-expressed; canonical form):

```yaml
#+begin_src holon_rule
name: related-lessons
input:
- bind: x
  type: block            # person/book/... are subtypes via a type attribute
  when: has_tag("project")
  consume: false
output:
- all_of: advice_candidates(x, tag_overlap_recency: {source: has_tag("lesson")}, k: 3, n: 20)
  place: display(under: x)     # maintained → retracts with the binding
#+end_src
```

The journal rule (canonical form, then its sugar):

```yaml
#+begin_src holon_rule
name: daily-journal
input:
- bind: day
  type: clock            # read arc on the environment relation
  consume: false
- absent: true           # inhibitor arc: no journal for today yet
  type: block
  when: parent_is("journals") and name == day.today
output:
- emit:
    place: journals            # canonical → ratcheted
    name: "{day.today}"
#+end_src
```

```yaml
#+begin_src holon_rule
name: daily-journal
when: not block_exists("Journals/{today}")
emit:
  place: journals
  name: "{today}"
#+end_src
```

How the one-language claim holds up:

- **Shared:** envelope (`name`, `active`), discovery/lifecycle (block-defined,
  program-marked, reconciled to synthesized matviews), the match language (arc
  `when:` clauses parse to the Pattern AST, dual-evaluated), and now the
  **output model** — everything emits; place kind decides
  maintained-vs-ratcheted.
- **`all_of:` is a multiset arc expression** (CPN-standard: arcs may carry
  collection-valued expressions — also the general answer to multi-token
  emission). `advice_candidates(...)` is its first built-in set-valued
  function; ADR 0022's closed `ScoringTemplate` vocabulary survives intact
  *inside the function's arguments*. The advice special-casing shrinks to this
  one UDF.
- **Residual asymmetry — anti-joins.** Display emission gets the suppression
  anti-join *implicitly* (dismissal is user intent, system-woven); canonical
  emission gets at-most-once *explicitly* (the author's inhibitor arc, plus
  deterministic emission ids). Same mechanism, different provenance —
  deliberate, since silently deduping a canonical emission would hide firing
  semantics the author must own.
- **Migration note:** advice rules' `source_language` is `'ln'` in code today
  (`holon-advice/src/discovery.rs`); actions use `'action'`. Both converge on
  `holon_rule` in Phase 3; until then the two formats coexist with their
  current languages.

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
- **Position-marking as the default:** reparenting content blocks for workflow
  state would make entities vanish from their knowledge context (a delegated
  person disappearing from "work"); attribute-marking is the default, position
  only where movement is the meaning (P7).

## Consequences & risks

- **Write-path pressure:** attribute-marking fires are field writes (cheap);
  position-marking fires are `move_block`s through consolidator → projection
  (→ org writeback). Assumption stated here: PN firings are human-timescale
  (planning workflows), not hot loops. Mitigations if violated: net-instance
  state in a document the org adapter does not materialize (replica
  participation is per-replica), plus the incremental-projection track.
- **Marking-style choice** needs a per-place-type rule of thumb (default
  attribute; position for boards; reference-tokens for multi-net) — the UX doc
  carries the user-facing side.
- **Compilation is real work** (net → matviews + intent wiring). Staged so the
  degenerate one-transition case (≈ today's query-actions) lands first;
  `action_watcher` is eventually re-understood as the compiled output of a
  one-transition net and deleted as a separate machine.
- **Simulator fidelity:** for vault-internal effects the compiler that makes
  nets executable also makes them simulable (the effect's marking-delta is
  derivable); external effects simulate lease/token movement only.

## Phases

Detailed Phases 1–2 plan (work packages, tests, rulings):
`docs/Proposals/action-unification-implementation-plan.md`.

**Phase 1 — decision-invariant fixes (valid under every outcome above):**
1. *Time-as-data:* clock relation + scheduler; delete the `is_tableless`
   one-shot branch of `action_watcher`.
2. *Deterministic effect IDs:* action `block.create` mints name-based
   UUID-shaped IDs per emitted token (subsumes the `create: missing 'id'` fix,
   PR #33 lineage).
3. *Program marking:* action/trigger blocks excluded from content rendering
   (fixes the journal-rule render bug; BugFunnel entries mapped in the plan).

**Phase 2 — guard language:** promote the dual-evaluated `Pattern` AST
(unify `holon_api::Predicate` + PBT `query_ast` vocabulary), with the
in-memory ≡ SQL agreement property as a prod invariant.

**Phase 3 — rules on the unified substrate:** one rule definition format +
discovery/lifecycle (generalizing ADR 0022) with one output model — emission
into canonical (ratcheted) or display (maintained) placement, advice =
`all_of: advice_candidates(...)` into display placement; provenance stamping
(`fired-by`) on engine-executed ops; automation
journal as a query.

**Phase 4 — nets:** attribute-marking as the default marking style (markings
derived from block attributes; `in-transition` for timed transitions);
position-marking for boards; compile transitions to matviews (reactive) and to
the in-memory evaluator (standalone); re-scope `holon-engine` accordingly;
lease tokens with user override for Once/Owner-only scopes.

**Phase 5 — deliberation:** simulator over serialized net subtrees; ranking /
what-if as advice; the AlphaGo-shaped heuristic-guided search is the long-term
differentiator (vault: `deliberative-layer-vision`).
