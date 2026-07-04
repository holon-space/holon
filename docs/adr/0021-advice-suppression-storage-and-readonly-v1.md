# ADR 0021: Advice suppression storage + read-only advice v1

**Status:** Accepted (2026-07-07). *Decision 2 ratified by Martin 2026-07-07 with
the condition that read-only v1 must be a stepping stone to editable advice
children, never an architectural dead-end: advice children render through the
normal render path with the editor mount gated off, and editability later =
land Increment C (the ADR 0016 `(EntityUri, Occurrence)` focus tuple) + remove
the gate — no v1 structure may preclude that.* Decided via research + an adversarial
Fable review; records the two forks that were gating the advice track's Spike-1
(suppression syntax) and the multiplicity question.
**Deciders:** Martin
**Relates to:** [ADR 0015](0015-computed-placement-and-curated-state-primitives.md)
(canonical/display placement; entity-identity vs element-identity §1a),
[ADR 0016](0016-occurrence-keyed-focus-authority.md) (occurrence-keyed focus),
`docs/Proposals/advice-feature-implementation-plan.md` (Increments E/F, Spike-1/2),
`docs/Reference/ORG_SYNTAX.md` (bare-ID drawer grammar).

## Context

Under the **advice-as-query reframe** (2026-07-07), advice is a read-only
relevance sub-region under an anchor block: advice results are **query rows**,
not an authored transclusion edge. Two forks were still open:

1. Where does *dismissing* an advice item durably live? (Spike-1's org-syntax
   fork, re-framed by the reframe.)
2. How is advice **multiplicity** resolved — the same lesson visible as advice
   under one block while its canonical occurrence is also on screen?

---

## Decision 1 — Suppression = anchor-side, edge-typed drawer key (typed option-b)

Dismissing an advice item persists as an **`:ADVICE_SUPPRESSED:` drawer on the
ANCHOR block**: a bare-ID list with `REQUIRES`' exact grammar (space/comma-separated
bare IDs; the `block:` scheme is added at the parse boundary per ORG_SYNTAX.md, and
stripped by the renderer). It is parsed as a **typed edge** into its own table
`advice_suppressed(anchor_id, lesson_id)` (shape of `block_requires.sql`), backed by
a new `EdgeField` variant.

Because the reframe deletes the authored advice edge, there is **no per-edge
property** to hang a `suppressed` bool on — "per-edge property" was the wrong frame.
The durable state is instead an authored **(anchor, lesson) EXCLUSION SET**: which
query results this anchor has dismissed. The drawer holds that set.

**Rejected alternatives:**
- **(a) Inline suffix grammar** (`id2[suppressed]`). `EntityUri::from_raw`
  (`entity_uri.rs:190`) is **infallible** and silently coerces garbage into `block:`
  URIs — a malformed suffix becomes a silent corrupt URI rather than a loud parse
  error. Compounded by the documented org-mangling history around `[` and `_`
  (subscript/link mangling). Fails "parse, don't validate."
- **(c) Edge-as-block** (reify each dismissal as a block). Reifies exactly the edge
  the reframe just deleted. Every dismissal block would ride the full
  org→Loro→consolidator→Turso→CDC pipeline — and the **consolidator is the measured
  vault-scale per-commit dominator** — while creating a new orphan/GC class and
  forcing the keystone PBT to model machinery blocks. Buys nothing the typed-(b)
  edge lacks.
- **Flat-string property as the END STATE.** Not SQL-reachable for the matview
  anti-join, and whole-list LWW under concurrent dismissals (vs H3 per-property-key
  granularity) means one device's dismissal can be silently **resurrected** by
  another's write. Note: flat-(b)'s drawer *syntax is identical* — "typed" here is
  about the **parse boundary and storage** (own table, own `EdgeField` variant), not
  new grammar.

**Backfill gate (open, storage-invariant).** Whether the suppression filter runs as
`NOT EXISTS advice_suppressed` **inside the IVM matview** (incremental backfill of
the top-K window) or as a **read-time anti-join** with disclosed over-fetch
(`LIMIT K+m`) is still being probed (Spike-2 stage 5). **Storage is identical in both
branches** — this is a query-shape decision, not a schema one.

---

## Decision 2 — Advice v1 renders read-only children

Multiplicity is real. The gating case is an advice-occurrence of a lesson visible at
the same time as that lesson's **canonical** occurrence (e.g. the lessons page open
in another panel, or the lesson in today's journal). A "advice only under the focused
block" policy does **not** avoid it: visibility is UI state and cannot be filtered in
the DB.

**The feared GPUI collision was checked and REFUTED.** `EntityCache` is
**parent-owned** (`entity_view_registry.rs:77-123`); two anchors' `LiveQuery` shells
have **disjoint caches**, so two advice occurrences of one lesson do not collide there.
Increment B's GPUI-identity-key work is therefore **off the advice critical path**
(it remains correct for the separable general-transclusion track).

**The REAL window-global hazard is the entity-keyed `focused_block` signal.** Two
mounted editors with the same URI both grab window focus
(`editor_view.rs:577-590`, `:465-477`) → nondeterministic focus ping-pong, and the
**loser's Blur commits its buffer** (`:161-165`). This is the ADR 0016 "multiple
cursors" hazard, reached via advice rather than transclusion.

**Decision:** advice v1 children mount **NO editor** — read-only rendering plus a
**dismiss affordance**; click-through **navigates/focuses the canonical block**. This
removes **Increment C** (occurrence-keyed focus) from the **advice critical path**;
C stays gated by the transclusion track, where editable second occurrences are the
whole point.

**Residue:** the id-based lookup in GPUI `user_driver.rs:149-165` needs
disambiguation for the PBT once a second occurrence is producible.

**Forward constraint:** if in-place editing of advice children is later wanted,
ADR 0016's `(EntityUri, Occurrence)` tuple is the **prerequisite** and must precede
that wiring.

---

## Consequences

- Advice track's Spike-1 is **decided**: typed anchor-side `:ADVICE_SUPPRESSED:` edge
  key; dismissal round-trips org reload. Increment E is re-scoped from a per-edge
  `suppressed` bool to the exclusion-set store.
- Increment C is **de-risked off the advice critical path**; advice v1 ships without
  occurrence-keyed focus.
- The suppression matview-vs-read-time anti-join branch (Spike-2 stage 5) is the only
  remaining open query-shape fork; storage is fixed regardless.
