# ADR 0032 — Petri-net execution semantics: one net, derived projection, dispatcher-enforced firing

**Status:** Accepted (ratified by Martin, 2026-08-25)
**Date:** 2026-08-24
**Deciders:** Martin (design discussion "PN execution architecture", 2026-08-24)
**Relates to:**
[ADR 0024](0024-unified-action-execution.md) — the Petri-net *surface*: token
vocabulary, marking styles, the effects-are-token-operations rule, the
dual-evaluated guard AST. This ADR settles the *execution* half that 0024 left
open.
[ADR 0031](0031-native-transition-catalog-and-macro-reification.md) — operation
descriptors as reified data, and the declared-guard seam this ADR's net guard
sits beside.
[ADR 0030](0030-birth-atomicity-authority-and-mirror-contract.md) — one
authority store per birth; firing never widens it.
[ADR 0028](0028-sharing-policy-overlay.md) — container-scoped sharing, which
the occurrence journal's scope follows.
`docs/Architecture/Model.md` — five layers, four mode axes, invariants 1–12.

## Problem

ADR 0024 named Petri nets as Holon's single action language and defined what a
token, a place, and a firing *mean*. It did not say how firing is executed,
where the net exists as an artifact, or what a firing may be refused for. Four
questions were left to be answered by whichever code happened to need them
first:

1. **What is the marking, exactly?** Change-data-capture events look like
   tokens (they arrive, they trigger, they are consumed), and treating them as
   tokens is the obvious implementation shortcut.
2. **Does the net exist anywhere?** Today the net is implicit — spread across
   rule blocks, operation descriptors, and per-rule watchers. Nothing can be
   drawn, simulated, or checked for conflicts.
3. **What refuses a firing?** Authorization is enforced; structural legality of
   the resulting marking is not. Placement rules live wherever a provider
   remembered to check them.
4. **What does undo mean for a firing?** Holon's undo engine records field
   fingerprints and replays inverses, with no relationship to the transitions
   that produced them.

## Decision

### 1. One logical Petri net, and the marking derives from durable state only

Holon has exactly **one** logical net. It is not a family of nets per feature.

**The marking derives from durable store state, and nothing else.** Two claims,
kept deliberately separate:

- **What a token is:** a token is *not* a row. Tokens are per **(entity,
  aspect)** (§4), and a token exists because durable rows exist with the
  attributes they have. For a simple block aspect that is one row's columns;
  for a **hierarchical entity** — a team, a digital twin with sub-twins — an
  aspect may derive from many rows or a whole subtree.
- **What carries marking:** every token, simple or composite, must **reduce to
  durable state**. Nothing else carries marking. In particular, CDC changes are
  **ephemeral evaluation triggers**, never tokens.

The second claim is load-bearing rather than philosophical: a resync re-emits
every event with no marking change, so a net whose tokens were events would
fire a phantom occurrence for every row it re-observes. Under the
durable-state reading, resync is a no-op by construction — the marking it
reports is the marking that was already there.

**Places are entity types and attribute predicates, not Turso relations.**
Places = the entity kinds and the predicates over their attributes ("blocks
tagged `Page`", "integrations in state `enabled`"). Mapping places onto Turso
relations is on ADR 0024's rejection list, and the reason survives here: guards
must evaluate with Turso absent (Model.md's storage-backend and file-adapter
mode axes both admit configurations with no projection). Matview subscriptions
are the reactive evaluator's **compiled read-arc indexes** — an implementation
detail of one evaluation mode, exactly as ADR 0024's P1b describes.

**Transitions are the operation catalog**, in two families:

| Family | Fired by | Enabledness |
|---|---|---|
| Rule transitions | the net itself, automatically | marking-guarded; fires whenever a binding is enabled |
| Intent transitions | a user, an MCP agent, or a rule | marking-guarded, plus an explicit firing request |

**A firing is one `execute_operation` dispatch.** There is no second execution
path. Rules fire by dispatching, which is why §6's precedence rule is
enforceable at all.

### 2. The net is a derived projection, never a second authority

The net exists as an artifact: a **projection compiled from rule blocks and
operation descriptors**. It is derived, read-only, and rebuildable from its
sources at any time. ADR 0024's P1a forbids a second authoritative store, and
that applies to the net's own definition as much as to its marking — editing
the projection is not a way to change the net.

Having the artifact buys analysis that the implicit net cannot offer. The
honest list:

- **Read/write-set conflict detection** — which transitions contend for the
  same places.
- **Cycle detection** over the transition graph (A produces what B consumes,
  B produces what A consumes). This is an **over-approximation**: it reports
  every real cycle plus some that guards make unreachable. It runs at
  rule-save time and fails loud.
- **What-if simulation** in `holon-engine`, which already has a net executor
  and needs only a compiled net to run against.
- **Whole-net visualization** — the first time the automation surface can be
  seen rather than inferred.

The projection's **arc language is the projection increment's design surface**,
not fixed here. One extension is anticipated from review: **correlated
multi-arc bindings** — a transition takes both a block and its parent as
inputs, with variables unified across the arcs — which is the CPN-orthodox
join and what expresses relational guards (a parent-hop predicate like
"parent is not a Page"). It is also cheap where it runs: read arcs compile to
matview indexes, and joins are what the SQL evaluator is natively good at.

What the projection explicitly does **not** buy: **colored-Petri-net model
checking**. Holon's colors are unbounded domains and its guards include
inhibitor arcs, which together put reachability beyond decidability. Claiming
otherwise would set an expectation the analysis cannot meet.

### 3. Execution stays distributed; enforcement centralizes at the dispatcher

Execution keeps its current shape: **one watcher per transition**, spawned
independently, supervised let-it-die. This ADR introduces no central
scheduler — but that is a statement about today, not carved in stone: if
cross-rule priority, fairness, or quiescence detection ever demand one, the
net guard's **arbitrate** slot (deferred item 5) is where central scheduling
grows, consulting the same derived projection — a future ADR's decision,
neither made nor precluded here.

What centralizes is **firing-time enforcement**. `OperationDispatcher` gains a
third seam, the **net guard**, consulted for every dispatched operation:

```
authorization (BoundaryEnforcer)  →  declared guards (GuardWorld)  →  net guard  →  provider
```

The net guard reads the derived projection and the durable marking, and
returns one of: **confirm**, **refuse** (loud `Err`, never a silent drop), or
**arbitrate** (the reserved slot for the fairness policies deferred in item 5).
It is the one place that knows what the resulting marking would be, which is
why placement legality belongs to it and not to individual providers.

In code it is the third **gate**, following the two existing gates' shape — an
`Option<Arc<dyn …>>` field with a dedicated `enforce_*` method at their call
site (`operation_dispatcher.rs:911-915`) — not a new interceptor abstraction
(Concerns §1). **Ruled (D12.a, 2026-08-24): the net guard stays a distinct
gate.** It overlaps `GuardWorld` conceptually — both are enabledness — so both
seams carry a comment stating when and how they unify: *when* the derived
projection exists and the two primitives of deferred item 6 make the
declared-guard predicates expressible as net arcs, *how* by generalizing
`GuardWorld` to marking-aware whole-delta evaluation and folding the net guard
into it (Concerns §2).

**First policy: the placement-and-capability move guard.** A move is refused
when either half fails:

- **Machinery containment** — a machinery block may not be moved out of the
  structure that owns it.
- **Destination capability** — the destination's home must declare that it can
  store an entity of the moved kind. This needs a *supported entity kinds*
  clause in capability profiles; the existing `hosted_kinds` axis
  (`Hierarchical` / `FreeStanding`, `crates/holon-capability/src/axes.rs:305`)
  describes a shape, not a kind vocabulary, so it is a neighbour rather than
  the field itself.

**Two paths bypass the dispatcher by declaration, not by omission:**

- **Text edits** travel through the Loro CRDT. Text concurrency is the merge
  algebra's job, and its job is to *merge*, never to exclude. A net guard on
  character edits would be asserting exclusion where the design guarantees
  convergence.
- **Org ingest** travels through the three-way diff into the consolidator.
  Ingest is the **environment**: marking that appears from the world, not a
  transition being arbitrated. A file that changed on disk has already
  happened; refusing it would only desynchronize the replica from its own
  files. **Sync is environment the same way**: a peer's merged-in changes
  arrive as marking appearing from the world — the local net never models the
  peer, only the arrival of its consequences (the digital-twin stance). Two
  rules keep that black box honest: every transition's declared claim
  discipline (§5) states how its firings fare under merge adjudication
  (optimistic = overridable, leased = protected within its lease domain), and
  a merge verdict that overrides a local firing surfaces as a **visible
  environment event in the journal** — a lost claim is something a
  compensation rule can fire on, never silent marking drift.

**Authorization-relevant predicates read the store, never the derived
projection.** The projection lags its sources — cold start, IVM catch-up — and
a subject the projection cannot classify tempts a guard toward "confirm",
because the pressure toward fail-open is latency and latency is permanent. A
predicate whose refusal protects capability therefore evaluates against
durable state, and where the store cannot answer, it refuses. A
convenience policy may still choose the projection and fail open; an
authorization policy may not.

### 4. Tokens are aspect-granular

One entity carries several **colored tokens, one per aspect**. Aspects are not
interchangeable, and a transition declares, per aspect, whether it **consumes**,
**reads**, or **produces**:

| Aspect | Semantics |
|---|---|
| Structural / placement | Consumable. The tree's one-parent invariant is exactly this token's linearity. |
| Text | CRDT-shared. Never exclusively held; concurrent holders are the normal case. |
| Existence / visibility | Never consumed. Read and produced only. |

**Aspects reach single-field granularity.** A block's `color` and `assignee`
are distinct tokens, so transitions on different fields never contend. The
shipped witness is the marking delta's `Envelope{varies_by}`: which token
`set_field` consumes is decided by its `field` parameter.

**Hierarchical entities carry their own tokens per level.** A composite entity
(a team, a digital twin with sub-twins) and its sub-entities each hold
distinct aspect tokens; consuming the composite's structural token does not
implicitly consume its sub-entities'. A transition that needs exclusive hold
of a subtree names the tokens it consumes — containment is expressed by arcs,
never by token identity.

**Exclusion is expressed by consumption, not by inhibitor arcs.** An inhibitor
arc says "fire only while no token is here", which is an extension to classical
Petri nets that weakens reachability analysis (ADR 0024 discloses this for its
own use of inhibitors in guards). Consume-semantics gets the same exclusion for
free: a consumed token is absent, and absence is plain enabledness. Inhibitor
arcs remain available in guards; they are not the mechanism for exclusion.

### 5. Claim disciplines: optimistic and leased consumption

**Arc type decides who contends.** Only consuming arcs contend — a transition
that reads never blocks anything, and a long read that needs a stable view
takes a snapshot, not a token. Every transition takes time (`async` makes that
explicit), and duration by itself implies nothing. What must be declared,
beside the marking delta on the descriptor, is the **claim discipline** of the
transition's consuming arcs — *when* its consumption takes effect.

**Optimistic consumption is the default and the local-first position.** The
consume takes effect at commit; concurrent consumers all fire; the merge
algebra adjudicates after the fact — the tree's one-parent invariant for
structure, last-writer-wins for plain values, deterministic identity for
idempotence. Nothing coordinates, nothing blocks, an offline replica fires
freely.

**Leased consumption is the opt-in coordination discipline**, and the only one
that gives exclusion across processes or replicas:

- **begin-T** consumes the input tokens into a durable **in-flight** state —
  ADR 0024's `in-transition` attribute shape, which exists for exactly this
  residue.
- **end-T** completes it, in one of three arms: **commit** (produce the
  outputs), **abort** (fire the derived inverse of the partial delta, §7), or
  **expire** (the lease ran out). All three are transitions; all three appear
  in the occurrence journal.
- The in-flight state is **marking like any other** (§1): durable rows, so
  crash recovery and resync need no special case, and "consumed and
  unavailable" is plain enabledness — the token simply is not there.

**Mechanically, the in-flight state is a row in a dedicated claims
collection** — there is no token row to remove, because tokens were never
stored things (§1). `begin-T` fires through the dispatcher like any operation
and writes a claim row `{entity_uri, aspect, occurrence_id, holder,
expires_at}` with **deterministic identity** (keyed by entity and aspect, so
two racing begins target the same row). The collection is deliberately
**separate from the entities it claims**, for three reasons: it works for
arbitrary entity kinds — blocks, integrations, twins — through the one
universal entity-URI reference; it is **holon-native regardless of the claimed
entity's home** (a claim on a Todoist-homed task must not ask Todoist to store
our lease); and it never round-trips through org files, because a lease is
runtime coordination state, not authored content. It holds *claim* marking
specifically — the rest of the marking stays what §1 says it is, the entity
rows themselves. Enabledness is then an ordinary place-predicate over two
entities: the token "exists" iff the entity does and no unexpired claim row
covers the aspect. The existing machinery covers each hard case for free:
crash — the holder stops heartbeating, `expires_at` passes, the guard treats
an expired claim as absent *lazily* (no janitor needed for correctness) and an
explicit `expire` transition fires when first observed; resync — claim rows
re-emit as-is, no phantoms; cross-replica — the deterministic row identity
makes the claim write **the** CRDT-winner operation, and the losing replica's
overridden claim surfaces as the §3 environment event. A claim row inherits
the claimed entity's container for sharing scope; a deleted entity leaves its
claims inert (expiry retires them). Renewals update `expires_at` and are not
occurrences; begin, end, abort, and expire are. The occurrence journal records
`begin`, so in-flight work is also queryable as open occurrences — but
enabledness authority is the claims collection, never the journal (journal
sync is optional per container; enabledness must work locally regardless).

**A leased transition's claim is a lease tied to session liveness**, renewed
by a heartbeat while the holding process lives — *not* by user activity. Tying
renewal to liveness is what makes crash detection reliable: a dead process
stops heartbeating within a bounded window, whereas a thinking user is
indistinguishable from a dead one.

**Why the discipline is declared rather than derived: `color` and `assignee`
have identical arcs.** Two racing writes to a block's color should merge —
last wins, no dialog. Two racing claims of a task's assignee must not merge
silently — losing a claim has to be loud. Same aspect shape, same consuming
arc, opposite correct semantics; only a declaration separates them.

**The discipline belongs to the transition, not the field.** The one-shot
`set_field` stays optimistic — an agent setting a color races and merges.
Opening a color *picker* begins a different, leased transition: begin-pick
consumes that field's token, browsing happens in flight, end-pick commits the
chosen value or aborts — and a one-shot set arriving mid-session finds the
token consumed and is refused by plain enabledness. Guard rail: leases are for
explicit *sessions* with human or external-effect duration, never for every
hover and affordance — and a surface without a lease degrades to
merge-plus-undo, not to data loss, so adoption is incremental.

**Editing is the first leased transition — and its claim is narrower than the
whole structural aspect.** begin-edit claims the edited block's **identity**,
not its placement: operations whose declared delta destroys or reshapes that
identity — delete, split, merge — contend with the live edit and are refused
or confirmed; operations that only relocate it — a move or reorder by a
collaborator or an agent — do not contend at all, because an edit anchors to
the block's ID, not its position, and the block may travel freely while the
user types. (Review narrowing: a collaborative move mid-edit is normal, not a
conflict.) end-edit commits.

**A wrongly-expired lease is cheap by design.** The cost is "re-acquire": text
was merging all along (§4), and structural operations during the gap are
*refused*, not lost — the user retries. False positives are therefore tolerable,
which is what lets the expiry window be short.

**Lease release and expiry are themselves transitions**, so both appear in the
occurrence journal. A lease that vanished is a visible event, not an absence.

Cross-replica claims resolve per mode: in Full mode by a CRDT-winner operation
or by the lease itself; in SqlOnly mode by a store transaction.

**Consuming reads and consuming claims are not in the first version.** They
arrive with **integrations** — beside editing and field-sessions, the third
leased instance: multi-step transitions with external effects, where a claim
is what makes promotion to an external system (Todoist and its kin)
exactly-once — merge can reconcile a store, it cannot un-send an effect. This is also where the humans-and-agents unification
becomes concrete: users, rules, and MCP agents are all **transition executors
drawing from one alphabet**, differing in who holds the claim, not in what
they may fire.

### 6. Precedence and the firing axiom

**Intent firings outrank rule firings on shared tokens.** A rule waiting on a
token yields to a user or agent that wants it.

**Rules cannot shortcut the dispatcher.** This is structurally true today, not
a policy to be enforced later: rule firing routes through `execute_operation`
with `OpOrigin::Rule` (`crates/holon/src/api/holon_rule_watcher.rs:421-427`),
so the same seams that judge a user's intent judge a rule's.

**The firing axiom, stated explicitly because implementations drift from it:**

> The marking decides. Occurrences trigger. Deterministic identity dedupes.

A blocked rule **re-evaluates when the token is released**. It never queues a
firing to replay later — a queued firing carries a stale marking, and replaying
it fires against a world that has moved on.

### 7. Undo is the inverse of a declared marking delta

**The firing log is an occurrence journal made of durable, container-scoped
store entities** with deterministic firing IDs. Container scoping is not
incidental: it aligns the journal with the sharing model (ADR 0028), and it is
what lets Martin later choose which logs are shared with whom and with which
device. Syncing the journal is therefore optional per container, and
**cross-device undo must not be precluded** by anything decided here.

**Undo of occurrence *k* fires a derived inverse transition** built from *k*'s
declared marking delta. It is enabled **only while that inverse delta still
is** — undoing a creation requires the created token to still be there. When it
is not, the refusal is **loud**; undo never approximates.

**External-effect transitions have no derived inverse.** No marking delta
un-sends a message. Such a transition offers either an **authored
compensation** or an honest "cannot be undone" — the Saga split, taken
deliberately.

**Undoability is statically queryable from the projection**: a transition whose
write set touches an external place is non-undoable *by inspection*, before it
ever fires.

**Undo stays meta-level.** Derived inverses never join the user-facing
transition alphabet — they are not rules a user can bind, name, or trigger
directly.

The existing undo engine (`crates/holon-core/src/undo.rs`) remains the
mechanism, re-founded on declared deltas rather than on field fingerprints
captured at write time. Migration concern, recorded rather than resolved: its
history is a per-replica snapshot persisted through `UndoStore` — durable
across restarts, but replica-scoped and unsynced, and its grouping cursors are
explicitly transient. That scope has to become container-scoped for
cross-device undo to be reachable.

### 8. Terminology: two vocabularies, one bridge

Petri-net vocabulary — **Transition, Place, Arc, Marking, Occurrence,
Binding** — is used in the **new projection and analysis layer only**. The
`Operation*` names stay exactly as they are in the dispatch layer. No file
blends the two.

The bridge:

| Dispatch layer | Net layer |
|---|---|
| `OperationDescriptor` | Transition |
| An operation instance (entity + params) | Binding / firing request |
| A successful `execute_operation` | Occurrence |
| A `GuardWorld` check | Enabledness |

**One literal type bridge pins this table in code**, so the correspondence is
compiler-checked rather than documentary. A wholesale rename
(`OperationDescriptor` → `Transition` and its family) is revisited only after
the projection exists and the two vocabularies have been lived with.

## Deferred — open, not decided

1. **Full-mode claim protocol.** Details wait on measuring whether the Turso
   fork's SELECT-then-INSERT is serializable under concurrent claimers.
   Measurement first.
2. **Journal sync and privacy semantics** — which containers' occurrence
   journals travel to which devices and people.
3. **Compensation authoring UX** — how a user or rule author writes the
   inverse an external transition cannot derive.
4. **`OperationDescriptor` → `Transition` rename.**
5. **Quiescence detection and cross-rule fairness.** The net guard reserves
   the arbiter slot (§3); the policies that would occupy it are not chosen.
6. **The declared-guard → arc-language merge gate.** Two primitives the arc
   language deliberately lacks decide whether declared guards can ever compile
   to arcs: a **correlated place predicate (relational hop)** — a place
   definition that references a second entity's attributes, correlated per
   firing (the shape of `parent(...)` guards) — and an **inhibitor over an
   uncorrelated place** — emptiness of a place as an enabledness condition
   (the shape of negated `block_exists`). The unification in §3 is on the
   table only when both are expressible as arcs; while either is missing,
   declared guards stay a guard-side language.

## Consequences

**First increment — three pieces, each independently useful:**

1. **Declared marking deltas on operation descriptors.** Data only: every
   transition states what it consumes, reads, and produces. Nothing consumes
   the declarations yet, and ADR 0031's equality oracle is what keeps them
   truthful.
2. **Projection derivation as a pure read artifact.** Compile rule blocks and
   descriptors into the net; run conflict and cycle detection over it. No
   execution path changes.
3. **The net-guard seam**, carrying the placement-and-capability move policy
   as its first and only tenant.

Undo re-founding (§7) and the editing lease (§5) are later increments. Every
one of them is a behavior change, so every one goes through the red-first
property-based-test discipline of the `holon-feature` skill: the keystone or
GPUI test fails *because* the behavior is missing, then passes.

**Costs accepted:**

- A third dispatcher seam is a third thing every composition site must wire,
  and a fourth thing every dispatched operation pays for. The two existing
  seams already crash loudly when unwired at a production site; the net guard
  follows that precedent.
- The derived projection is a build step that can go stale relative to its
  sources. It is rebuildable, so staleness is a correctness bug in the
  derivation, not a data-loss risk.
- Aspect-granular tokens make a transition's declaration longer than "reads
  X, writes Y". That verbosity is what makes the analysis in §2 possible.

## Concerns raised during drafting

These are inconsistencies noticed while recording the design. They are
surfaced, not resolved.

1. **"Third interceptor" is off by one.** The dispatcher already has two
   pre-provider gates — `BoundaryEnforcer` (authorization, ADR 0028) and
   `GuardWorld` (ADR 0031 declared guards) — called in that order at
   `crates/holon/src/api/operation_dispatcher.rs:911` and `:915`. The net
   guard is the **third gate**, making four stages including the provider.
   Neither existing gate is called an "interceptor" in the code; both are
   `Option<Arc<dyn …>>` fields with dedicated `enforce_*` methods, and the net
   guard should follow that shape rather than introduce an interceptor
   abstraction.

2. **The declared-guard seam overlaps the net guard's job.** ADR 0031's
   `GuardWorld` already evaluates subject-bound relational predicates against
   the current world and refuses before the provider runs — which is
   "enabledness" under §8's own bridge table. Whether the net guard is a
   distinct seam or a *generalization* of `GuardWorld` (marking-aware, whole-
   delta rather than subject-bound) is worth settling before both exist.

3. **Undo's current storage is not what the design assumed.** The migration
   concern was framed as "history in process-local state". Measured: `UndoStack`
   is persisted per replica database through the `UndoStore` trait
   (`crates/holon-core/src/undo.rs:39-44`), so it survives restarts. The real
   gap is *scope* — replica-wide and unsynced, versus the container-scoped,
   optionally-shared journal §7 requires.

4. **Capability profiles have a neighbouring axis, not the needed one.**
   `FidelityAxes::hosted_kinds` carries `Hierarchical` / `FreeStanding` — the
   shape of a hosted thing, not a vocabulary of entity kinds. Whether the
   destination-capability check extends this axis or adds a new one is open.

5. **Model.md invariant numbering.** The design discussion referenced
   "invariant 3" for the intent boundary. In the current Model.md, invariant 3
   is `after_sibling` in intent, and the intent-boundary rule is carried by
   the Layer-1/Layer-5 rules plus invariant 8 (structural ops are commit
   points). Invariant 8 is the one that bears on §5: a begin-edit that
   consumes the structural token must flush pending editor state first, since
   it *is* a structural op.
