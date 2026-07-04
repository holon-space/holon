# ADR 0015: Canonical vs display placement — the display-placement contract (dissolving "advice")

**Status:** Proposed (2026-07-06; not implemented). **What is ratified here: the
canonical/display *distinction* (§1) and the display-placement *contract* (§3).**
**P2 *implementation* is GATED**, not ratified — conditional on (1) the plan's
Phase 1b proving per-occurrence focus feasible against the real focus authority,
and (2) acceptance of the separate focus-rekeying ADR (§3 rule 5). The safety
bet is de-risked *analytically*; the decisive PBT run is a required merge gate,
not yet run (§Evidence).
**Deciders:** Martin
**Context:** A "resurface the right lesson at the right moment" use case (advice)
threatened to become a bespoke feature type. The premise of Holon is that it is
generic enough to build many things from a small set of composable blocks. This
ADR decides **one** hard-to-reverse thing: the distinction between *canonical*
and *display* placement, and the contract a display-placed row must obey. The
other primitives that the advice use case also wants (curated-state modeling, a
temporal/event query source, vector ranking) are **out of scope here** and
listed as separate future decisions (§Deferred).
**Relates to:** ADR 0005 (children as ordered list — canonical placement),
ADR 0002 (trait-based unified type system — `TaskState`), ADR 0012
(reference-model capability contract — the invariants that gate this),
`docs/Architecture/Model.md` (invariants 1–4).

## Problem

The use case: **lessons** (reusable patterns) should resurface where a task
would benefit — injected, not manually queried everywhere. The naive design was
a bespoke type (`VirtualOrigin::Advice`). It is the wrong shape:

1. **Genericity is the product.** A primitive that only serves "advice" is not a
   Holon primitive. Whatever we build must also build backlinks, related notes,
   saved searches, and dashboards — or it is the wrong abstraction.

2. **"Advice" was two unrelated things wearing one name.** Holon has two
   "not-in-the-obvious-place" mechanisms: the **ephemeral creation placeholder**
   (`:__virtual:` synthetic row; materialized into a real block on first edit)
   and the **source/`live_query` block** (a stored query whose result rows are
   real blocks — verified: `render_interpreter.rs:475-526` renders real blocks,
   `view_event_handler.rs:183-224` routes their edits to the real block). Advice
   is neither a not-yet-real block nor domain-specific — it is a **real** lesson
   block **rendered in a computed position** under a task.

3. **The consolidator owns order.** Any design that makes a block's *canonical*
   placement a function of a query breaks Model.md invariants 1–3: two replicas
   evaluating the query against divergent pre-merge state produce different
   placements, and the consolidator can no longer mint one convergent order.

## Decision

### 1. Canonical vs display placement (the load-bearing distinction)

- **Canonical placement** — one stored `parent_id`/`sort_key`, minted by the
  consolidator, merged, authoritative. Stays a stored scalar. Never computed.
- **Display placement** — zero-or-more *computed* positions a block appears in a
  rendered view. Never stored, never minted, never merged.

We **reject** the deeper abstraction "a block's parentage is a resolvable
relation of which stored-parentage is one case." Collapsing the two makes an
illegal state (a block with two competing canonical parents) representable and
makes order non-convergent (invariants 1–3). This primitive exists precisely to
keep the two *separate*.

### 1a. Entity identity vs element identity (the principle rule 5 instantiates)

Display placement's whole difficulty is one root cause: **`EntityUri` has been
doing two jobs that were 1:1 until now and are now distinct.**

- **Entity identity** (`EntityUri`) — *which block's data*. Owns content,
  edit-routing-to-canonical (rule 3), CDC keying, the shared field `Cell`. **Data
  is shared by reference**: one `Cell<T>` per `(uri, field)` backs every
  occurrence, so two renderings of `L` cannot silently diverge — a write in one
  propagates to all (`BuilderServices::editable_text(&EntityUri, field)` →
  `BlockCellRegistry`, `reactive.rs`; Model.md invariant 12: holds in Loro and
  SqlOnly).
- **Element identity** — *which render slot*. Owns focus, caret, the editor
  `InputState`, the collection-driver row key, the element id, expand/collapse,
  selection. Per-slot, never shared across occurrences.

This is **not a new model** — it is `Model.md`'s existing **"Cell vs Mutable (the
UI state cut)"**: `Cell<T>` keyed `(uri, field, type)` is entity-tier shared
state; `Mutable<T>` on the ViewModel node is per-render-slot state, with Model.md's
own warning *"two same-id rows in different panes need independent state — never
collapse these into a `(uri, field)` registry."* Pre-P2, entity-id and slot-id
coincided, so a single `EntityUri` served both and the sites below quietly fused
them. P2 breaks the 1:1; every mechanism keyed on bare `EntityUri` for a
*render-slot* concern (focus, caret, editor cache, driver keys) becomes ambiguous.

Rule 5 (focus/caret/undo re-keying) is therefore **an instance of restoring this
cut**, not a bespoke patch: it moves render-slot concerns onto element identity
while data stays entity-shared. Element identity is encoded as the structured
`(EntityUri, Occurrence)` tuple, *not* an opaque id — deliberately entity-projectable
because focus is set overwhelmingly by entity-first writers (backend op responses,
navigation, MCP, the worker envelope); see ADR 0016.

### 2. The decision: a display-placement edge (P2) — CONDITIONAL, and the cost center

The distinction (§1) and contract (§3) are ratified. **P2 itself is adopted only
conditionally** — its hard prerequisite (per-occurrence focus, §3 rule 5) is
deferred to a separate ADR and its projection-inertness is still unverified
(§Evidence). Read §2–§3 as "the shape P2 must take *if* Phase 1b clears it," not
"P2 is greenlit."

`live_query` (P1) already renders real blocks with edits routed to the real
block. What does **not** exist: placing such a computed view as a display-only
child of an **arbitrary anchor**. Today the only anchoring is `virtual_parent`
hardwired to the query block's own context (`render_interpreter.rs:640-667`).

Arbitrary-anchor display placement is **the expensive part of this ADR**, not a
cheap enabler. Its cost is: the contract (§3), the focus/caret re-keying (§3),
and the origin-aware invariants (§Evidence). "Advice is cheap once P2 exists" is
true only *after* that cost is paid.

### 3. The display-placement contract

A display-placed child is **display-only in its host**:

1. mints **no** `sort_key` in the host and does **not** change the referent's
   canonical `parent_id`;
2. does **not** count as one of the host's stored children (rollups, "N
   subtasks", matview aggregates);
3. **edits route to the canonical home** of the referent;
4. carries a **display-origin marker** so ref-aware consumers can distinguish a
   *canonical* occurrence of `L` from a *display-placed* one. This marker
   **must not be an id-infix** like `:__virtual:` — encoding origin in the id
   string (`view_event_handler.rs:252` `split_once(":__virtual:")`,
   `viewmodel_tree_virtual_slots.rs:60` `contains(":__virtual:")`) is the
   `match str.as_str()` sin; the marker is typed metadata on the rendered node.
5. **disambiguates focus, caret, edit-target, and undo grouping by
   `(id, occurrence)`, not by bare id** — i.e. keys these on **element identity**,
   restoring the §1a cut (edit-target still resolves to the entity's canonical
   home per rule 3; only the render-slot concerns move). This is the leak the
   earlier draft missed. Focus and caret are keyed today by bare `EntityUri`
   (`reactive.rs:953` `focused_block: Mutable<Option<EntityUri>>`;
   `set_focus_with_caret(block, offset)` `reactive.rs:274`;
   `headless_editor_mirror.rs:132/209` seeds caret by block URI alone). A
   display-placed **editable real** block (per rule 3) creates a *second*
   editable occurrence of the same id; under id-keying, focusing one focuses
   "the block" and the editor mirror targets whichever the engine resolves. The
   current app tolerates a duplicate id only because the second occurrence
   (sidebar/title) renders a **non-editable** variant — a tolerance P2 destroys.
   The display-origin marker (rule 4) **cannot** fix this, because focus/caret/
   undo are keyed on the id, not the node. **Re-keying focus/caret/undo by
   `(id, occurrence-path)` is a hard prerequisite of P2 — but a separate ADR.**
   ADR 0010 already reserves multi-occurrence focus ("graduates to
   `MutableBTreeMap<Region, Option<EntityUri>>` — a separate ADR"), and it spans
   ~21 prod files across four frontends plus the MCP focus surface. This ADR's
   obligation is only to **prove feasibility** (an editable display-placed
   occurrence with per-occurrence focus/caret) before committing to P2; the
   production rollout belongs to that focus ADR.

### 4. Relevance is queryable data on a typed edge (pattern only; extraction deferred)

For the advice use case, dismissal/curation lives on the **reference edge**
`B → L`, not on task-state (overloading task-state corrupts task rollups). The
edge must be **first-class and bidirectionally indexed** (backlinks need "who
points at me"). Its curation status follows the **`TaskState` pattern**
(`holon-api/src/types.rs:467`): an open user-configurable label + a **closed
role parsed at the boundary**, so logic branches on the role, never the label
string.

- **Lead option — a bit.** If the contract only ever reads "is this edge
  suppressed?", relevance state is a `suppressed: bool` on the edge and none of
  the below is built.
- If it needs ≥3 curation states with distinct role-behavior, model it on the
  `TaskState` pattern. The **generic `CuratedState<Role>` extraction** (refactor
  `TaskState` into `CuratedState<StateCategory>` and share machinery) is
  **deferred until a second real instantiation exists** — a generic with one
  instance is YAGNI, and `TaskState` is shipped and load-bearing.

### 5. The ephemeral creation placeholder stays a separate primitive

It points **inward** (mints a new identity on first edit — how data is born);
P2 points **outward** (resolves an existing identity). Shared render path,
separate type. A single `materialize()` over both would mean two irreconcilable
verbs.

### Advice dissolves

Advice is a source block (P1) whose query ranks lesson blocks by a score,
rendered by display-placement (P2) at the task, with dismissal = flipping the
reference edge's curation state (§4). No `Advice` type. The same P1+P2+P3 build
backlinks, related notes, saved searches, and dashboards.

## Evidence (de-risking) — and the required gate

The riskiest assumption — *a display-placed child is inert w.r.t. canonical
truth* — was checked against the composed keystone PBT catalog (ADR 0012):

- **Canonical invariants are predicted immune — UNVERIFIED.** The green baseline
  (`general_e2e_composed_pbt`, 4 cases, 195s, PASS) ran **without** any
  display-placed ref-known row, because P2 does not exist. So immunity is a
  **prediction**, not a construction proof. `inv-org-render-fixed-point` renders
  org from SQL (a render-layer row is not in SQL) and the `*_match_ref` family
  reads Loro/Turso — the prediction is well-founded, but the decisive run has
  not happened.
- **Required merge gate for P2 (necessary, not sufficient):** a new invariant
  asserting canonical bit-identity (consolidation, sibling-order, org-render,
  child-counts) with a display-placed ref-known row present, green, **before P2
  lands**. This proves *projection* inertness only — it passes trivially for a
  **non-editable** row and says nothing about the focus/caret collision (rule 5).
  The true feasibility gate is an **editable, focusable** display-placed
  occurrence proving per-occurrence focus/caret (the plan's Phase 1b).
- **The two exposed invariants** that would flag a display-placed ref-known row
  and therefore need origin-awareness: `inv-main-panel-rows-match-focus`
  (`main_panel_rows_match_focus.rs:117-121`, `ref_known && !allowed` → hard
  fail) and `inv-viewmodel-decompiled-rows-match-query` (content/order, trips on
  ghost rows). Note `inv-viewmodel-entity-ids-subset-of-data` is **not** exposed
  — it admits ref-known ids (`viewmodel_entity_ids_subset_of_data.rs:71-74`).
- **Working precedent:** the `:__virtual:` creation slot is a display-only row
  living in the widget snapshot today, checked only by
  `inv-viewmodel-tree-virtual-slots`, while canonical invariants stay green —
  evidence that display-only rows *without a ref-known id* already coexist with
  canonical truth. The ref-known case is the untested delta.

## Consequences

**Positive.** Advice dissolves into P1+P2+P3; backlinks, related notes, smart
folders, dashboards fall out of the same primitives. Dismissal has a principled
home (the edge). The canonical projection and convergence are untouched.

**Negative / required work.** P2 is a **cost center**: arbitrary-anchor
placement + the §3 contract + **re-keying focus/caret/undo by `(id,
occurrence)`** (a substantial frontend change) + two origin-aware invariants +
the new bit-identity gate invariant. The contract is now load-bearing for a
whole feature class, so a single leak (stray `sort_key`, child-count, or a
shared caret) corrupts canonical truth or the editor broadly.

## Deferred (each a separate future decision, not ratified here)

- **P4 — temporal/event query source.** Only the session *handover* needs to
  query the change/event stream; advice, backlinks, dashboards do not. It must
  **not** hard-depend on Turso CDC — CDC is the ephemeral *outbound projection
  tail* (state deltas), whereas the handover wants durable *history/intent*.
  **P4 needs a new durable, range-queryable, replica-agnostic history/intent
  source that does not exist yet** — do not mistake existing streaming plumbing
  for it. Specifically, code-checked:
  - `ChangeNotifications<T>::watch_changes_since` (`holon-api/src/streaming.rs`,
    Loro impl `loro_backend.rs:2612`) is **not** durable history: it decodes the
    position as a `u64` watermark into a **bounded in-memory `EventRing`**
    (`event_ring.rs`, cap 4096; eviction → `ReplayWindowExpired` → forced full
    resync). It is **forward-live from a watermark, not a past-range query**, so
    it cannot answer "what changed since last session" once the window has
    evicted. It swaps one ephemeral source (CDC) for another — not a fix.
  - `ChangeSet`/`ChangeOp` (`holon-api/src/change_set.rs`) is **Phase-2
    shadow-only / undispatched** — the consolidator writes raw ops and only
    builds a `ChangeSet` to check agreement. It may *inform* P4's op vocabulary;
    it is not a live source.
  - The `Vec<u8>` stream position has **no shared codec** (u64 seq / u64 seq /
    empty across the three impls), so a real `ChangeSource` needs a redesigned
    position type (enum or associated type), **not** `Vec<u8>` — a git commit
    hash cannot round-trip today.
  So P4's only honest durable backing is the **planned Phase-5 intent log**
  (referenced in `change_set.rs`, **unbuilt** — no `intent_log` in the tree), and
  **SqlOnly P4 is blocked until it exists**. Keep the sound parts: CDC is
  rejected because it is convergent-state, not an event log (Model.md layer 4);
  the goal is a **replica-agnostic** history source. Beware the recurring trap —
  `ChangeSet` (inbound intent) and `watch_changes_since` (outbound deltas) are
  **different abstractions**; the P4 history read is a third. Its own ADR.
  Removed from the primitive set here; P1+P2+P3 stand alone for every case
  except handover.
- **Vector ranking (Embedder seam + derived vector store).** `vector_distance`
  is deterministic arithmetic; the nondeterminism is producing a vector (a model
  call), which belongs behind an `Embedder` seam (like Clock) faked in the PBT.
  **Open before adoption:** (a) reconcile with Model.md **invariant 4 —
  "exactly one writer per *store*"** (not per table): a CDC vector indexer
  writing Turso is a second store-writer unless the vector rows live in a
  *separate* store (then address cross-store consistency) or the existing single
  projection writer owns them; (b) a `model_version` bump invalidates the whole
  corpus at once → reindex-storm/GC cutover story. Advice ships on **symbolic**
  ranking (recency + reference weight) first; vectors are a later, separate ADR.
- **`CuratedState<Role>` extraction.** Deferred until a second real
  instantiation exists (§4).

## Alternatives rejected

- **`VirtualOrigin::Advice` / bespoke advisory cell.** A feature wearing a type
  hat; not a primitive.
- **Parentage as a resolvable relation.** Breaks invariants 1–3.
- **Overloading `TaskState` for relevance.** Corrupts task rollups.
- **Unifying the creation placeholder and display-placement under one "virtual"
  type.** Conflates inbound intent with an outbound view.
- **A display-origin id-infix.** Re-commits the `:__virtual:` string-sniffing
  debt one level up.
