# ADR 0031 — Holon-native transition catalog, reified by macro

Date: 2026-08-09. Status: accepted (Martin, ratified 2026-08-08 as decision D7 of the
agent-op/revert design; this ADR is the in-repo record of that ratification).
Companion to ADR 0024 (unified action execution — the PN transition vocabulary and the
dual-evaluated Pattern AST) and ADR 0030 (birth atomicity — whose Enforcement clause
names this machinery).

## Problem

A user-intent operation's knowledge is scattered. The trait method carries the
signature; the provider body carries the real semantics; an opaque precondition closure
carries the guard; hand-written descriptor literals carry the UI and boundary metadata;
and anything that must be *simulated* carries a second, hand-written declaration in a
`holon-engine` YAML net. Nothing ties them together, so a declaration that diverges from
what the provider actually does is misinformation with authority.

Two consequences are already paid for:

- **Guards are not data.** `OperationDescriptor::precondition` was
  `Option<Arc<Box<dyn Fn(&HashMap<String, Box<dyn Any>>) -> Result<bool, String>>>>`.
  A `dyn Fn` over `dyn Any` is not serializable, not inspectable, not simulatable, and
  not comparable — the type carried a manual `PartialEq` that skipped the field, a
  `Debug` impl that printed `"<closure>"`, and a serde skip. A closure cannot be loaded
  by a second consumer, so the differential oracle below is unreachable while it exists.
- **Shared predicates are single-sourced by value but hand-wired by call site.**
  `holon_core::block_op_catalog::page_under_non_page_prohibited` is documented as "the
  ONE shared predicate for every reparenting/re-tagging chokepoint", yet each chokepoint
  calls it by hand. One forgotten call site is a silent hole.

## Decision

**Adopt option (a): a Holon-native transition catalog, derived from the op definitions
by macro.** Arcs and guards are declared in Holon's own vocabulary, against the real op
pipeline, next to the trait method they describe. `holon-engine` remains what it is
today — the ranking / what-if simulator over the task layer — **fed FROM the native
catalog rather than being the catalog**.

Concretely: the declaration surface is the existing `#[operations_trait]` attribute
family (`#[affects]`, `#[triggered_by]`, `#[menu_exposure]`, `#[boundary_behavior]`,
`#[target_scope]`, `#[require]`), extended with guard and arc declarations. The macro's
output is a plain, serializable value on `OperationDescriptor`. One declaration, every
consumer derived.

### Why option (b) was refused

Option (b) was: extend `holon-engine`'s flat net to carry inhibitor arcs, relational
attribute access and dynamic arc sets, and let that net be the catalog. It was refused
because all three additions are **structural** to the engine's `TokenState` / flat
attribute model, so (b) pays a large engine rewrite to obtain a declaration language
that is still further from the real op pipeline than Holon's own vocabulary is. The
three gaps the de-risking experiment hit are cheap in the native direction and expensive
in the engine direction: an inhibitor is a predicate over live state; ancestor-chain
reads are an accessor over the block tree; per-firing arc lists are data-dependent
arity. The engine's job — ranking and what-if over the task layer — does not improve by
becoming the system of record for op semantics.

A third shape (a declarative `ops.toml` + `build.rs` codegen) was considered and refused
for a different reason: it re-opens the exact drift this ADR exists to close. The
declaration would live in a different file from the method it describes, so signature
changes and declaration changes become separately reviewable, and a renamed method
becomes a runtime lookup miss instead of a compile error.

### The three standing unification guards

These are binding on every increment (they predate this ADR; it restates them so they
are enforceable in-repo):

1. **No PN runtime in the live dispatch path.** Guard evaluation is a compiled
   predicate; accept stays semantic replay through the normal dispatcher. The engine
   stays a simulator.
2. **The catalog is derived from op definitions, never hand-maintained in parallel.** A
   hand-maintained parallel catalog that can drift is the kill criterion for the whole
   design.
3. **Adoption is incremental.** Only ops that appear in scenarios or in an
   exhaustiveness set get declarations. A day-one total catalog is explicitly refused.

### The HARD dual-consumer requirement

**The catalog substrate must be loadable by BOTH the in-memory engine and the real
dispatcher.** One catalog, two consumers — otherwise the differential oracle compares a
declaration against itself and proves nothing. This is what forces the declaration to be
serializable data rather than a closure, and it is the reason the closure guard is
deleted rather than wrapped.

### Declarable-as-EXCLUDED

Effects below the declaration boundary are **declared as excluded, never omitted**. The
de-risking experiment surfaced exactly one undeclared write — `sort_key`, the
consolidator's order monopoly (Model.md invariant 2; invariant 3 already bans intents
from carrying order keys). "Not declared" and "deliberately below the boundary" must be
different states in the type system, so silence is a red and an explicit
`Excluded { reason }` is the only way to be quiet about a field.

This follows the house pattern already applied three times on `OperationDescriptor`
(`menu_exposure`, `boundary_behavior`, `target_scope`): a non-defaultable field with a
fail-closed variant plus an exhaustiveness certificate — never an `Option<T>` that
defaults to absent.

### The equality oracle is the standing truthfulness gate

Macro derivation satisfies unification guard (2) by construction for signatures and
preconditions. It cannot satisfy it for effects below the declaration boundary. So the
standing gate on catalog truthfulness is the **mutation-proven marking-equality oracle**
— for each declared op, simulated marking ≡ real post-op state, with an intentionally
mutated declaration required to red. Prose review is not the gate. The corollary is
operational: **do not expand the exhaustiveness set faster than the oracle covers it.**

### Scope of "transition"

Transitions are ADR 0024 PN transitions: semantic operations above the **intent
boundary**. Keystrokes, CRDT merge, fractional-index minting, projection, CDC, org
write-back and Sync/Ingest environment moves are below it and are never catalogued.
`split_block` the *op* is in; `TypeChars` the *gesture* is not.

## Guard language

Guards are the ADR 0024 dual-evaluated **Pattern AST** — not Rhai, not free-form SQL.
That AST is not future work: it exists in prod as `holon_api::pattern`, with a guard
string parser (`Guard::parse`), an in-memory `evaluate()` over an `InMemoryWorld`, a
`to_sql()` compiler against a projection-owned `SchemaAbstraction`, and a
mutation-proven in-memory ≡ SQL agreement PBT (`holon-advice/tests/pattern_agreement.rs`).
Its first consumer is already the ADR 0024 user rule block surface
(`holon-advice/src/holon_rule.rs`, the `when:` guard string).

The catalog therefore does **not** introduce a guard AST. It adopts that one. Two
consequences follow and are binding:

- **No third predicate-ish type.** `holon_api::pattern`'s own doc records the committed
  convergence (`Pattern::Scalar(Predicate)`); adding a separate op-precondition AST
  beside it would re-create the duplication this ADR exists to remove.
- **The parser must be reachable from the macro.** `holon-api` depends on
  `holon-macros`, so `holon-macros` cannot depend on `holon-api`. Parsing a guard at
  macro-expansion time (below) requires the Pattern module to sit in a leaf crate that
  both depend on. That relocation is part of the increment that lands the retargeted
  `#[require]`, not a separate cleanup.

`to_sql()` is already built, so the catalog inherits SQL compilation rather than
deferring it. The subject set is NOT extended for op parameters: guards are
relational-only (P6=A, ruled below).

`holon-engine`'s own guards stay Rhai. User-authored YAML nets keep Rhai until a
separate ruling says otherwise; the catalog's guards are the Pattern AST, and the
engine loader translates. Unifying the two guard languages is NOT part of this ADR.

### Ruled: guard syntax is a string literal (P2)

Guard expressions are written as **string literals parsed by the Pattern parser at
macro-expansion time**, exactly as ADR 0024 already rules for user rule blocks ("rule
bodies are valid YAML, with guard expressions as strings parsed by the Pattern
parser"). A parse error is therefore a **compile error**, and built-in ops and user rule
blocks share ONE grammar instead of forking into two.

The known hazard is that rustfmt's `format_strings` mangles long string literals
containing escaped quotes, and a corrupted guard is worse than a broken build because it
may still parse and mean something else. Mitigation: guards compose by named
sub-pattern, and the parser **hard-errors on any guard literal over 80 characters**,
naming the composition escape hatch in the message. That is a lint, not a convention.

### Ruled: exhaustiveness scope is minimal and oracle-paced (P3)

The fail-closed exhaustiveness set contains only the ops named in the increment plan and
grows **one op at a time, together with the oracle coverage for that op, in the same
revision**. Ops outside the set legitimately carry the fail-closed variant. This is
unification guard (3) made operational and it is the mitigation for catalog
untruthfulness: the set may never outrun its oracle.

### Ruled: op-precondition guards are RELATIONAL-ONLY (P6=A)

`holon_pattern::pattern::Subject` is `Block | Clock`, and a guard evaluates by iterating
that relation and returning bindings — "enabled" means at least one binding. An op
precondition such as "this parameter is non-empty" iterates nothing and binds nothing.
The open question was whether the catalog's guards gain a parameter subject.

**They do not.** A guard is a predicate over the state the op touches; **parameter
validity belongs in the typed params and NEVER in a guard subject.** Two reasons:

- **The oracle stays total.** A parameter subject has no relation to iterate, so it has
  no meaning under `to_sql()`. It would have to be carved out of the mutation-proven
  in-memory ≡ SQL agreement PBT, and a grammar with a hole is a grammar whose oracle
  proves less than it appears to. Every leaf reachable from a guard string is dual
  evaluated, with no exceptions.
- **Typed params already own it.** `OperationParam` carries a `TypeHint`, and the
  parse-don't-validate rule makes the parameter's own type the place where "non-empty" /
  "in 1..=5" is enforced. Restating it in a guard would be a second, weaker,
  drift-prone copy of a constraint the type system can hold.

The consequence is concrete: the pre-existing `#[require]` fixtures were all
parameter-shaped (`!id.is_empty()`, `priority >= 1`) and are therefore illegal under
this ruling. They are rewritten as relational predicates
(`crates/holon-macros-test/src/lib.rs`).

The increment that lands the retarget adds exactly one leaf to serve the first real
relational need: `Pattern::Parent(Box<Pattern>)` — "the subject block HAS a parent and
that parent satisfies the inner pattern". It is existential, not implicative, so a
parentless block never matches and `parent(not has_tag("Page"))` means "has a
non-page parent" — precisely the shape `page_under_non_page_prohibited` needs, where a
root page is legal. It compiles to a 2-valued `EXISTS` correlated on `parent_id`, so it
stays sound under `Not`, and it is covered by the agreement PBT like every other leaf.

### Deferred-open: facet authority (P4)

Whether facet-level authority declarations (ADR 0030 D5 — "who owns this facet") fold
into this catalog is **deferred and still open**. ADR 0030 records it as an open
question and it is an architecture-level call, not a lane decision. Until it is ruled,
repair-by-re-derivation for file mirrors stays blocked exactly as ADR 0030 states.

## Enforcement

- `OperationDescriptor`'s guard field is non-defaultable and carries an explicit
  "declares no precondition" variant: that is a stated fact, not an absence.
- The guard is ordinary serializable data: it round-trips through serde, participates in
  `PartialEq` and prints in `Debug`. A serde round-trip test on the descriptor with its
  guard intact is the certificate that the dual-consumer requirement stays reachable.
- **A declared guard reads the CURRENT state; the chokepoints read the PROPOSED one.
  Inc 3 must not confuse them.** A declared `parent(...)` guard evaluates the subject
  block's EXISTING `parent_id` — it detects a block that IS already in a violating
  topology. The three hand-wired chokepoints
  (`holon_core::traits` ~:2263, `sql_operation_provider` ~:3810,
  `loro_block_operations` ~:204) ask a different question: they evaluate the
  PROSPECTIVE parent of a move that has not happened yet. So Inc 3's dispatcher gate
  MUST evaluate guards against the **post-image of the proposed op** (or bind the
  prospective parent explicitly as the guard's subject), and must NEVER naively swap
  the declared guard into those call sites — under the current topology the guard is
  trivially false for the very move the chokepoint exists to refuse. What IS already
  proven is the equivalence of the two predicates over a given topology:
  `parent_guard_reproduces_the_chokepoint_truth_table`
  (`crates/holon-pattern/src/pattern.rs`) reproduces all six
  `(child_is_page, parent_is_page)` cases of `page_under_non_page_prohibited`,
  including the parentless case. That is the semantic bridge; supplying the right
  state to evaluate against is Inc 3's job.
- Guard evaluation is a single generated gate at the one routing point, not codegen
  inside each provider. Totality is delivered by the provider's write transaction and
  its rollback (ADR 0030's own position: "total or refused" is delivered by the
  transaction, not by literal guard-before-write), so the guard is free to run before
  the provider — which also gives mode honesty for free: one evaluation, both write
  authorities.
- An advertised op with no catalog entry, and a catalog entry no provider advertises,
  are **boot refusals**, not warnings. That is the `block_op_catalog_parity` test
  promoted to a runtime invariant, and it is the regression guard for bug class I1
  (descriptors drifting between the two write authorities).
- What this deliberately does NOT do: it does not prove a provider body opens a
  transaction, nor that no write escapes it. That obligation stays with ADR 0030's
  fault-injection PBT. A typestate firing token would make a forgotten guard a *compile*
  error rather than a *boot* error; it is recorded as the escalation path, not adopted,
  because it means threading a parameter through both providers' entire write surface.

## Consequences

- `PreconditionChecker` and every producer of it are deleted. Old and new do not
  coexist.
- ADR 0030's "until D7 lands" clauses become pointers to this ADR and to the increment
  that satisfies them.
- The keystone PBT is a third consumer and the host of the truthfulness oracle. Deriving
  the keystone's *enabledness* from the catalog is legitimate; deriving its *effects*
  from the catalog would trivialize the oracle and is refused.

## Out of scope

- Anything below the intent boundary; a PN runtime in live dispatch; replacing the
  temporal-LIFO user undo stack; a day-one total catalog.
- Extending `holon-engine`'s flat net (refused option (b)); unifying its Rhai guards
  with the Pattern AST.
- User-authored rule blocks' authoring surface (they share the grammar, not the track).
- An op-name newtype/enum replacing the free-string op-name sites across frontends and
  keymaps — genuinely valuable, genuinely orthogonal.
- The scenario store / staging and external-twin overlays. This catalog unblocks them;
  it does not build them.
