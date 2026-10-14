# ADR 0032 — Petri-net execution semantics: one net, derived projection, dispatcher-enforced firing

**Status:** Accepted (ratified by Martin, 2026-08-25). Amended in place 2026-10-13:
net semantics revisions 1 and 1.1 (§9; changes to §1, §3, §5, §7), #171.
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
**Direction:** [Vision-PetriNetCoordination.md](../Architecture/Vision-PetriNetCoordination.md)
(direction, not a decision). A change to execution should move toward pull
(backward chaining that outputs a plan), engine interface places as read-only
environment places, and a kernel net that extends itself by validated subnets.

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

### 1. One logical Petri net, and written marking derives from durable state

Holon has exactly **one** logical net. It is not a family of nets per feature.

**Marking that a transition writes derives from durable store state, and
nothing else.** Two claims, kept deliberately separate:

- **What a token is:** a token is *not* a row. Tokens are per **(entity,
  aspect)** (§4), and a token exists because durable rows exist with the
  attributes they have. For a simple block aspect that is one row's columns;
  for a **hierarchical entity** — a team, a digital twin with sub-twins — an
  aspect may derive from many rows or a whole subtree.
- **What carries marking:** every token a transition produces, consumes, or
  relocates must **reduce to durable state**. Nothing else carries such
  marking. In particular, CDC changes are **ephemeral evaluation triggers**,
  never tokens.

The second claim is load-bearing rather than philosophical, and its reason
also states its scope: a resync re-emits every event with no marking change, so
a net whose tokens were events would fire a phantom occurrence for every row it
re-observes. Under the durable-state reading, resync is a no-op by construction
— the marking it reports is the marking that was already there.

**That hazard only bites places a transition writes**, which is what bounds the
requirement. A **read-only place** — one no transition produces, consumes, or
relocates — has no event to re-emit and therefore no phantom to fire. Such a
place need only be **observable**: it may be backed by any source the net can
watch, a reactive var as legitimately as a durable row. Capability profiles are
the motivating case — configuration, authored outside the net, read by guards
(the destination-capability check in §3) and written by no transition.

The exception is narrow and stays policed: **the moment a transition writes a
data-class place, that place owes durable state.** Process-class and
device-class places (§9.4) are not synced and have one writer, so a resync
cannot re-emit them as phantom events; they owe nothing durable beyond their
class. Read-only is a property of the whole
transition catalog rather than a promise one declaration makes about itself, so
giving a config-backed place its first writer is a change that must move the
place onto durable state first, not a declaration that can be added in passing.

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

**Delivery is not authority.** The net may be *pulled* — recomputed on demand
and held nowhere — or *held as a reactive var*, recomputed whenever its sources
change. Both are the same pure function of the same sources, so neither can be
stale and neither acquires authority; the choice is one of delivery, not of
semantics. A held net is emphatically not a cache kept for symmetry with the
other derived artifacts: a cache persists a snapshot and then owes an
invalidation story, whereas the net re-derives on the source change itself and
so has no staleness window to manage. The line a delivery shape must not cross
is retaining state the current sources no longer imply.

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

**Anticipated: transitions that change the net's own structure.** None of this
is built, and none of it is decided here; the shape is recorded because it is
the shape that keeps such a change consistent with this section, and that is
worth stating before someone needs it. Introducing or retiring a datatype —
enabling an MCP client, disabling an integration — introduces or retires the
places that datatype carries. Modelling that as a **meta-transition**, a
transition *inside* the net whose effect happens to be on the net's own
structure, does not make the net an authority. The opposite: a firing **is** the
authority mechanism, so expressing structural change as a firing puts it under
the same dispatcher, the same three gates, and the same occurrence journal as
every other operation, instead of beside them. The net still holds no authority
of its own — net-structure change simply stops being an out-of-band edit and
becomes a first-class modelled operation.

Such a transition acts on the **change delta** and does the housekeeping that
delta implies. The motivating case is §5's: a transition removed while one of
its firings is in flight leaves a claim that no live transition will ever end.
§5 retires that claim by expiry, which is correct but only as prompt as the
lease is long. A meta-transition observing the removal can release the claim
immediately, as declared housekeeping on the delta, and record the release in
the occurrence journal like any other firing. Expiry remains the backstop that
makes correctness independent of anyone noticing; the meta-transition is the
fast path, not a replacement for it.

### 3. Execution stays distributed; enforcement centralizes at the dispatcher

Execution keeps its current shape: **one watcher per transition**, spawned
independently, supervised let-it-die. This ADR introduces no central
scheduler — but that is a statement about today, not carved in stone: if
cross-rule priority, fairness, or quiescence detection ever demand one, the
net guard's **arbitrate** slot (deferred item 5) is where central scheduling
grows, consulting the same derived projection — a future ADR's decision,
neither made nor precluded here.

The target is one executor for all token classes (§9.4). The semantics it
runs are §9; they are net constructions, not scheduler special cases.

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

**Typing into a creation slot is a text edit whose first character also creates
the node.** The slot has no node until then; the first character's write IS the
create, performed in-process through the cell registry (`split_block`'s
precedent), with the SQL row following through the outbound projector. It is
listed here because it looks like a create and is governed by the text-edit
bypass above: one writer, causal order by construction, nothing for the
dispatcher to arbitrate. The net observes the resulting node as marking, exactly
as it observes every other text edit.

The reason it is not modelled as a marking precondition instead: an existence
precondition satisfied in-process by the same task that needs it is not an
ordering problem, and modelling it as one costs a durable row per gesture while
leaving the CRDT leg — which bypasses the dispatcher by this very section —
uncovered. Where no cell route takes the create (no registry installed, or the
registry declines), the create is dispatched instead; that leg is not only a
test axis, GPUI production runs it too (D113.a). On both legs the write tier
(Model.md invariant 14) is asked once, before the id is minted: bypassing the
dispatcher does not bypass its tier decision.

**Authorization-relevant predicates read the store, never the derived
projection.** The projection lags its sources — cold start, IVM catch-up — and
a subject the projection cannot classify tempts a guard toward "confirm",
because the pressure toward fail-open is latency and latency is permanent. A
predicate whose refusal protects capability therefore evaluates against
authoritative state — the store, or the configuration backing a read-only place
(§1) — and where that source cannot answer, it refuses. A
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
consume takes effect at commit; concurrent consumers all fire; nothing
coordinates, nothing blocks, an offline replica fires freely. Optimism decides
only that no consumer is excluded. It does not change what a data-class
consumption records: every consumption of a data-class token adds an entry to
the token's claims set (REV11-consume.a, below). The transition's declared
`contest` decides what the concurrent entries mean after the merge:
`idempotent` (the derived `owner` is the deterministic minimum and nobody is
asked) or `conflict` (both devices show `contested` and a user decides). The
merge algebra adjudicates the content after the fact — the tree's one-parent
invariant for structure, last-writer-wins for plain values, deterministic
identity for idempotence.

**Leased consumption is the opt-in coordination discipline**, and the only one
that gives exclusion across processes or replicas:

- **begin-T** moves the input tokens into the in-flight place `<T>.Running`
  (§9.2) — ADR 0024's `in-transition` attribute shape, which exists for exactly
  this residue.
- **end-T** completes it, in one of three arms: **commit** (produce the
  outputs), **abort** (fire the derived inverse of the partial delta, §7), or
  **expire** (the lease ran out). All three are transitions; all three appear
  in the occurrence journal.
- The in-flight state is **marking like any other** (§1): a token in a place,
  so crash recovery and resync need no special case, and "consumed and
  unavailable" is plain enabledness — the token simply is not in its input
  place.

**Mechanically, a consumption is an entry in a claims collection.** There is
no token row to remove, because tokens were never stored things (§1). The
in-flight state of a leased or effectful firing is a token in the place
`<T>.Running` (§9.2, REV11-running.a); the claims collection holds the
cross-replica half. Rule (REV11-consume.a, NET-class.a): consuming a data-class
token **adds** one entry `(device, occurrence_id, read_version)` to a **grow-only
claims set** on that token. It never overwrites a single "claimed_by" field,
because a CRDT merge of two overwrites silently loses one claim. Two derived
places are computed from the set, identically on every device: `owner` (the
deterministic minimum entry) and `contested` (more than one entry survives).
Each transition declares `contest: idempotent | conflict`. Under `conflict`,
both devices show "contested" after sync and a user decides. A lease adds
`expires_at` to its entry; a retraction (§7) adds a `retract` entry, and
`owner` ignores retracted claims. A multi-token consume adds entries to every
consumed token in one commit (REV11-binding.b).

The collection is deliberately **separate from the entities it claims**, for
three reasons: it works for arbitrary entity kinds — blocks, integrations,
twins — through the one universal entity-URI reference; it is
**holon-native regardless of the claimed entity's home** (a claim on a
Todoist-homed task must not ask Todoist to store our lease); and it never
round-trips through org files, because a claim is runtime coordination state,
not authored content. Enabledness is an ordinary place-predicate over two
entities: the token "exists" for this device iff the entity does and the
derived `owner` is this device or no unexpired claim covers the aspect. Crash:
the holder stops heartbeating, `expires_at` passes, the guard treats an expired
entry as absent *lazily* and an explicit `expire` transition fires when first
observed. Resync: claim entries re-emit as-is, no phantoms. Cross-replica: the
set merges by union, so a losing claim stays visible as `contested` or as the
§3 environment event, never as silent drift. A claim entry inherits the claimed
entity's container for sharing scope; a deleted entity leaves its claims
inert. Renewals update `expires_at` and are not occurrences; begin, end,
abort, and expire are. The occurrence journal records `begin`, so in-flight
work is also queryable as open occurrences — but enabledness authority is the
claims collection, never the journal (journal sync is optional per container;
enabledness must work locally regardless).

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

Cross-replica claims resolve per mode: in Full mode by the claims set's derived
`owner` and `contested` places or by the lease itself; in SqlOnly mode by a
store transaction.

**Consuming reads and leased claims across integrations are not in the first
version.** They arrive with **integrations** — beside editing and field-sessions, the third
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

**Occurrence ids are carried per delta** (per field write), not per entity
row: a delta of *k* names *k*'s occurrence id, so a field written by several
firings still names each writer (NET-mark.a, §9.3).

**Undo of occurrence *k* is a manual transition that declares `undoes`**, built
from *k*'s declared marking delta. It finds the firing through the occurrence id
its delta carries. It is enabled **only while that inverse delta still is** —
undoing a creation requires the created token to still be there. When it is
not, the refusal is **loud**: a stale undo is an error to the caller, never a
panic or a silent no-op; undo never approximates. Firing it sets an **undo
mark** (§9.3) so the automation that produced *k* does not redo it. The loader
refuses an `auto` transition that also declares `undoes`. Undoing a claim adds
a `retract` entry to the claims set (§5), because that set only grows.

**A row a later firing holds is owed, not skipped.** The undo leaves an
`<T>.UndoPending` token keyed by the answer id. The row is reversed only while
nothing has committed to it since: a commit by its holder discloses "cannot
undo `<answer>`: `<place>` of `<subject>` is used by `<holder>`", a commit by
anyone else "... was rewritten by a later write", and the row stays as that
writer left it — a record never clobbers a later write. A later write that
restores the earlier content is still a later write.

**External-effect transitions have no derived inverse.** No marking delta
un-sends a message. Such a transition offers either an **authored
compensation** or an honest "cannot be undone" — the Saga split, taken
deliberately.

**Undoability is statically queryable from the projection**: a transition whose
write set touches an external place is non-undoable *by inspection*, before it
ever fires.

**Undo stays meta-level.** Derived inverses never join the user-facing
transition alphabet — they are not rules a user can bind or name; the undo
command fires them.

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

### 9. Net semantics: runs, firings, marks, classes, watched sources

Revisions 1 and 1.1 of the net (#171). Every rule here is a **net
construction** or a rule of the record step; the executor implements no
special case for producer order, run state, failure or undo. Worked examples
and the experiment evidence are in the design note
`plans/PlacesTransitionsExpressions.md` (holon-work).

#### 9.1 Producers and runs (NET-run.a, NET-cand.a)

**NET-run.a.**

- **Producers share a verb form.** Producers of one place produce the same
  kind of token, so they share one verb form (`MakeTursoReady`,
  `MakeLoroReady` for `Storage`). Inner steps keep their own verbs
  (`OpenTurso`, `Recover`).
- **Every producer is a workflow net with ports** (a subnet when it is a
  chain). Producers of one place are **ordered**. Producer *k* is enabled when
  the earlier producers' last run failed or timed out and no retry is pending.
  The failure cause is the derived "why not" of the output port. Inner
  evidence survives a failed run; outer writes go only through declared fusion
  places.
- **One timer per run** (inner ≤ enclosing). A pending backoff counts as
  running. An abandon timer orphans a hung attempt. Timers are process-class,
  monotonic, and carry a count bound beside the time bound.
- **Run state is a complement place pair per output**: `<Out>.Open` (one token
  while the run is unsettled) and `<Out>.Settled`. Every producer of `<Out>`
  has a read (test) arc on `<Out>.Open`. The firing that fills the output port
  consumes `<Out>.Open` and produces `<Out>.Settled`.
- **A new run is a transition `Settled → Open`** (manual Reopen, an accepted
  upgrade, a declared input change). So "a new run starts only by a declared
  trigger" is structural.
- **Not an inhibitor arc on `<Out>`.** Settled is a property of the run, not of
  the current marking. Once a consumer takes the `<Out>` token, an inhibitor
  would re-enable the earlier producer with no trigger. Inhibitors also leave
  plain P/T analysis.
- **Late results need no special case.** A slow producer read `<Out>.Open` at
  its enabledness check. That token is gone by its record, so its inputs are
  stale, and the stale-record rule (9.2) sends its output to the candidate
  place.
- **Run-state places are process or device class**, never data class. A
  data-class place with alternative producers follows NET-alt-class.a (9.4),
  not producer order.
- **Order is a net construction too**: producer *k* takes an input from the
  failure/timeout port of producer *k−1* and a "no retry pending" place. The
  why-not chain follows from the marking.
- **Example.** `MakeTursoReady` fails: the failure mark is
  `(Storage, h)` for `MakeTursoReady`, where `h` is the hash of its read trace
  (§9.3). `MakeLoroReady` has no mark and is unaffected.

**NET-cand.a.** A **candidate** place holds a late or upgrading result. A
candidate carries its **base**, its **output** and the **read trace** of its
inputs. The **apply transition** is declared per place. The default for value
places is a **3-way merge** (base, current, candidate), with conflicts as
conflict places (CONFLICT-places.a). A **storage switch** is a projection
catch-up (STORE-cap.a), not a value merge. An apply with conflicts is offered,
never automatic.

#### 9.2 Pure and effectful firings (NET-fire.a, REV11-running.a, NET-manual.a)

**NET-fire.a.**

- **Pure** means a transition reads and writes only net places. Anything that
  touches the file system, the network, the OS or a remote is **effectful**
  (for example `Recover`, or acquiring the writer lock).
- A transition decides only on its bound tokens. It never reads outside its
  binding during the firing.
- **Pure firings stage and commit once.** On data places the record IS the Loro
  commit; there is no rollback vocabulary on Loro. A record over several
  consumed tokens is atomic (REV11-binding.b).
- **Effectful firings run lease → effect (with an occurrence id) → record.**
  The **receiver kind** is declared:
  - `Keyed`: the receiver dedups by occurrence id, so an unknown outcome
    auto-retries.
  - `StateProbe`: the transition checks that the goal holds, so an unknown
    outcome auto-verifies. It records "goal holds", not "I did it".
  - `None`: an unknown outcome parks the token in `OutcomeUnknown`, a dead
    letter that is disclosed. The user resolves it with Verify, Retry or Drop.
- An occurrence's outcome derives from all its attempts. It is Done when any
  attempt is Done, and Failed only when every attempt is definitely Failed.
  Else it stays parked. A late result closes only its own attempt.
- A **definite failure** of the occurrence returns the input to its place. **Verify** takes a
  result, or the transition declares outputs derived from its inputs.
- **A record never clobbers.** It has two parts. **Facts** about the outside
  world (what the file now holds = the persisted base; the occurrence is done)
  commit unconditionally. **Writes to authority rows** are a compare-and-set
  of each (place, subject) version in the firing's read trace. On a mismatch a pure step rolls
  back and re-fires against the new rows. An effectful step's row outputs
  follow the stale-record rule.
- **An effect's record is never refused.** Stale inputs at record: on a place
  that declares an apply transition, the outputs become a candidate
  (NET-cand.a). On data class, the outputs commit (Loro merges) and a condition
  "applied over changed inputs" is disclosed. Elsewhere the outputs are
  withheld and the condition "done over changed inputs" is disclosed. An
  effect that happened is a fact, so its occurrence and its input consumption
  commit even over stale inputs; only an effect that did not happen (a
  definite failure) returns the input and sets the failure mark.

**REV11-running.a.** A take moves the token into `<T>.Running`, a place whose
class is the input's class, at most device. The move changes no content and no
version. Consequences:

- Guards see the token's absence honestly; why-not says "in Running of T".
- Crash recovery reads tokens in `<T>.Running` with no live attempt. They become
  Unknown, and the receiver kind decides.
- A rollback moves the token back, again without a version change. A version
  changes only when a record commits changed content.
- **Inhibitor and read arcs read the marking like every other arc.** A token in
  `<B>.Running` is not in `B`. A transition that `B` inhibits may therefore
  fire while another firing holds `B`'s token, and a later rollback does not
  undo that firing. A modeller who means "no X, in flight included" adds an
  inhibitor arc on `<B>.Running` too: a net construction, not an engine rule.

**NET-manual.a.** Manual transitions over MCP take a caller-supplied occurrence
id. Caller-supplied and executor-minted ids are disjoint by type, so they never
collide; a duplicate caller id is an error to the caller. A manual fire answers `ACCEPTED(occurrence id)` at bind, never a bare
`Ok(())`. The caller can await the outcome `Done | Failed(cause) |
OutcomeUnknown` by that id. A definite failure of a manual effect also raises a
disclosed condition on the entity (for example "rewrite not done: a.org changed
since it was read"), cleared by the next success. `OutcomeUnknown` and failure
conditions aggregate per transition, so a full disk or a dead remote shows one
condition per transition, not one per attempt. Every condition text is derived
from the marking, so it is never stale.
For an answer (`Answer.T`), the awaited outcome is that of its work occurrence,
whose id is derived from the answer id; an expired answer settles
`Failed(NoLongerApplies)`.

#### 9.3 Retry, failure and undo marks (NET-mark.a)

**NET-mark.a.**

- **An attempt re-fires only on a committed input change or a declared retry
  timer.**
- **The binding and the guard's read trace are captured once**, at the
  enabledness check that decided to fire. A guard is never replayed at a later
  marking. A derived place is transparent in the read trace: the content of its
  source tokens is captured (a derived place has no identity or version).
- **One mark kind**, keyed by (subject, content hash of the captured read
  trace), at most one per (transition, subject). Every transition has exactly
  one subject (ARC-subjects.a); for a producer of a singleton place the subject
  is that place. Two producers of one place therefore never share or clobber a
  mark:
  - Reason `Failed`: process class. Cleared by a content change in the read
    trace or by a declared retry timer.
  - The mark is lazy: it blocks while the current read trace hashes (blake3) to
    its key, so an edit A→B→A with no firing on B blocks again.
  - Reason `Undone`: data class, so it syncs. Set only by a manual `undoes`
    transition (the loader refuses auto + `undoes`). Addressed by the occurrence
    id that the undone delta carries. A stale undo is an error to the caller.
    Undo of a claim adds a retract entry; `owner` ignores retracted claims.
- **Static check at net derivation**: a transition's write-set closure must not
  feed its own read-set, unless the transition declares a fixpoint measure. Run
  triggers and retry inputs are declared places.

#### 9.4 Token classes and stores (NET-class.a, REV11-consume.a, REV11-devstore.a, NET-alt-class.a)

**NET-class.a.**

- **Classes** are process ⊂ device ⊂ data. Process class lives in one process
  only. Device class survives a restart and stays on this device. Data class is
  synced by a CRDT, which merges and never refuses a write.
- **One class per delta.** Local reads only in guards.
- **Process and device consumption** is a compare-and-set at the record. A lost
  race is a disclosed, normal outcome, never an assertion under a lock.
- **Data consumption** adds a claim entry (§5, REV11-consume.a, REV11-binding.b).
- **Parked tokens** carry a schema version and a stable transition id. Unknown
  shapes load as a disclosed condition with Drop.
- **Derived places are evaluated in process**, never only as Turso matviews.

**REV11-devstore.a.** Device-class tokens live in a device-local, never-synced
Loro document, written only under the vault writer lock. This is modelled as
places: `AcquireWriterLock` (effectful, retry timer) → `WriterLock` token →
`OpenDeviceStore` → `Store<Device>` token. Without a `Store<Device>` token
device class is unavailable, disclosed from the empty place.

**NET-alt-class.a.** A place whose content is recomputed per device (a view, a
ranking, a cache) and whose alternatives, chosen by a local fact, can give
different values is device class. Equivalent alternatives may stay data. A
recorded result (an agent's web research, a summary the user keeps) is data:
produced once under a lease with an occurrence id, replicated, never recomputed
on other devices.

#### 9.5 Watched sources (NET-watch.a, REV11-lockless.a, REV11-xfile.a, ARC-subjects.a)

**NET-watch.a.**

- **`Behind` is derived.** For a projection it is computed from the
  projection's persisted base against the current state, and the record of a
  write updates the base. An echo then ingests as theirs == base (no delta). A
  watcher event that arrives before the record merges as ours == theirs (no
  change). No rule skips an ingest because the bytes equal what Holon wrote;
  the controller keeps those bytes (`last_projection`) only as the text of
  the base.
- **The watcher is a lossy hint.** Correctness never depends on one event. Its
  feed emits only a contentless `DiskChanged`. Other sources of `DiskChanged`:
  every write the content compare-and-set rejects (it proves the disk moved),
  boot, and a slow stat sweep (mtime + size).
- **`ReadDisk` reads the file once** and puts `OnDisk{hash}`. A read with
  an unchanged hash changes nothing. The hash is a stable content hash, the one
  the merge-base row stores.
- **Whole-content removal is refused.** An ingest whose merge would remove ALL
  content of a file (the file ingests to zero blocks, whitespace-only included)
  never auto-applies. The file token goes to `Refused`, disclosed. This covers
  truncate-then-write editors and accidental deletes. `Refused.seen` is the hash
  of the bound `OnDisk` text the decision was made on.
- **`Refused` is a sink.** It is left only by different file content
  (`OnDisk.hash != Refused.seen`, then Resume), by manual `AcceptRemoval`, or by
  manual `RewriteFromStore`. `CatchUp` is not enabled from it.
  - `AcceptRemoval` accepts what the file says: the base merge with the removal
    applied. Blocks the file never held stay.
  - `RewriteFromStore` is a file write like any other: it advances the
    persisted base.
  - A row edit in Holon after the refusal is newer intent than the file's
    emptiness. The store content goes back to the file through the
    compare-and-set write, and a failed `AcceptRemoval` is disclosed.
- **The store holds every block the user can make**, empty and whitespace-only
  blocks included. A file format either represents a block, or its write-back
  omits it with a disclosed per-block condition (for example "not in a.cook: an
  empty step cannot be written"). The base merge keeps an omitted block (base
  and file both lack it), so an omission never comes back as a deletion.
- **`Base` and `Writing` are process class (Q4.a).** Durability is the merge-base
  sidecar and the file row, not the marking. Boot emits `DiskChanged` for every
  tracked file, so a write the crash cut off is a file that is Behind again: the
  next `CatchUp` ingests or writes it. No `StateProbe` exists for it.
- **Disk at the base, store ahead: the store wins (WB-restore.a).** Known
  limit: the crash window between a write and the stamp of its hash. A user
  restore of the exact pre-edit bytes inside that window equals the base, so
  the boot overwrites it with the store's edit. Pinned by
  `a_restore_inside_the_crash_window_between_write_and_stamp_keeps_the_stores_edit`
  and bugfunnel entry
  `2026-10-14-a-restore-inside-the-write-stamp-crash-window-is-overwritten`.
- **The production subnet (WB-net.a).** One net per vault, subject = document
  (`crates/holon-filesystem/src/writeback_net.rs`). Places: `DiskChanged`,
  `OnDisk{hash}`, `Base{hash, authority}`, `Authority{hash}` (the hash
  of the store's render of the document), `Refused{seen, authority}`. `Behind`
  and `InSync` are derived. `ReadDisk` and `Resume` are auto; `CatchUp` is auto
  and effectful, one ingest XOR one compare-and-set write per firing, with a
  retry timer and a count bound, and `Refused` inhibits it. `AcceptRemoval` and
  `RewriteFromStore` are manual with `()` input and answer
  `ACCEPTED(occurrence)` (ops `writeback.accept_removal`, `writeback.rewrite_from_store`).
  The hash is `BaseSidecars::hash`.
- **Authority is written on every store change (Q3.a).** The compare-and-set
  and the row write of a record are not atomic: a store edit that lands in that
  window is applied over changed inputs and caught up by the next `CatchUp`.
  The record's own row write is not foreign: its render hashes to the recorded
  `Base.authority`, so it enables nothing.
- **A write the disk refuses is a `CatchUp` failure mark (Q6.a)** with the retry
  timer, disclosed as a stalled write-back. No poisoned place exists; the mark
  clears when its read trace changes or a retry lands.

**REV11-lockless.a.** A process without the vault writer lock is read-only for
stored state: no ingest, no write-back, no bases, no device tokens. It is
disclosed as "read-only: another Holon process holds the vault".

**REV11-xfile.a.** Org ingest is batched per watcher burst (one transaction). An
id that leaves one file and appears in another within a batch is a move. An id
in two files outside a batch is a conflict place.

**ARC-subjects.a.** One subject per transition. Sort keys are minted at the
ingest boundary. A remaining key collision is a derived place
`NeedsRekey(parent)` with its own `Rekey` transition. Multi-block operations
(`delete_keep_children`, `delete_subtree`, `restore_split`) are subnets of
single-subject transitions, staged and committed once, and undone as one unit.

**Out of scope:** a data-class token type change across a mixed-version fleet
(a CRDT schema migration).

## Deferred — open, not decided

1. **Full-mode claim protocol.** The data-class half is ruled (REV11-consume.a,
   §5). What still waits is the SqlOnly / Turso-side half: measuring whether
   the Turso fork's SELECT-then-INSERT is serializable under concurrent
   claimers. Measurement first.
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
7. **Meta-transitions over the net's own structure** (§2). Which structural
   changes are modelled as transitions, what their marking deltas are over the
   places they introduce and retire, and how their housekeeping on the delta is
   declared rather than hard-coded. Foreseen, not designed.

8. **The general cross-process feed.** In the process that loses a race for a
   process or device token, the winner's output is not in the loser's local
   marking. Options: (a) a declared feed place that carries the winner's
   record to the other process; (b) the loser reads the shared store after a
   lost compare-and-set and builds its marking from it; (c) accept the
   disclosed "lost race" as the whole answer. Not ruled.
9. **A document whose blocks are all unwritable by its format** survives an
   emptied file plus `AcceptRemoval` (§9.5). Options: (a) accept as ruled — each
   block carries its own `NotWritten` condition; (b) add a document-level
   condition "this file cannot hold any of its blocks". Not ruled.

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
