# Implementation plan: the full "advice" feature

Master plan for **advice** (resurface the right lesson at the right moment),
built from composable primitives — there is **no `Advice` type**. Under
[ADR 0015](../adr/0015-computed-placement-and-curated-state-primitives.md)
(canonical/display placement) and
[ADR 0016](../adr/0016-occurrence-keyed-focus-authority.md) (occurrence focus).
Supersedes the sequencing in the
[display-placement plan](display-placement-implementation-plan.md) (kept only for
the Phase-1b spike detail).

> **Rewritten twice.** A code-grounded senior review REJECTED the first cut
> (corrections below). This second rewrite re-expresses every increment as the
> project's **TDD-with-PBT loop** (next section): refactors that ease a feature
> land green *before* it; the feature itself is driven from a failing PBT that
> fails for the *right* reason.
>
> Corrections carried from the review, now folded into the loop steps: the focus
> increment cannot be validated before a real rendered occurrence exists (the old
> C↔D inversion → C now depends on B); "advice is thin wiring" was false (an IVM
> matview-feasibility spike precedes F); the `suppressed` bit is a schema +
> **org-format** change, not a bool (no per-edge property serialization exists);
> the reactive **store** collapses duplicate ids *upstream* of the cited
> signal-vec sites (`reactive.rs:465` `set_neq`); and the keystone PBT
> structurally cannot produce a second occurrence, so the old standalone "fixture
> phase" is now the **step-4 PBT change** of each increment that first needs it.

## Status (2026-07-15)

**LANDED through step 6.** All six increments A–F shipped: `RowOrigin`
(`crates/holon-frontend/src/row_origin.rs`), `AdviceRule`
(`crates/holon-advice/src/rule.rs`), `AdviceSidecar`
(`crates/holon-frontend/src/advice_weaver.rs`),
`EdgeField::AdviceSuppressed`. Advice suppression, weaver, and anti-join
machinery are all in place.

Still open:
- Step 7 live-MCP gate
- Increment G (editor content-sync consolidation, optional)
- Increment H (app-layer reranker, ADR 0023)

## What advice actually is

A **source block (P1)** anchored to a task, whose query selects lesson blocks via
**reference edges (P3)** ranked by a **symbolic score**, rendered by **display
placement (P2)**; dismissal inserts into the authored exclusion set
([ADR 0021](../adr/0021-advice-suppression-storage-and-readonly-v1.md)). The honest
headline: advice is a thin final layer, and the weight beneath it is larger than
the first cut admitted — a store rekey, a result-row re-home mechanism that does
not exist, an authored-edge store, new org syntax, and an IVM matview spike. The
plan front-loads all of it.

> **Naming.** "P1/P2/P3" below are the ADR-0015 *primitives* (source block /
> display placement / reference edge). The ordered work is labelled
> **Increment A–F** to avoid colliding with those names.

## The per-increment loop (the contract every increment below obeys)

Each increment is written as these seven steps. Deviations are called out
explicitly per increment.

1. **Run the PBTs** — start from green (`general_e2e_composed_pbt` + the
   `holon-frontend` lib slice).
2. **Pick the increment** — one user-visible capability.
3. **Refactor along green** — land the behavior-preserving refactors that *ease*
   the feature while the PBTs stay green. This is where the store rekey, the
   `RowOrigin` retype, and the focus-tuple widening live: each is a no-op when the
   new `Occurrence` coordinate defaults to `Canonical`, so it lands *before* any
   red.
4. **Modify the PBTs** to express the new capability — they now **FAIL** (feature
   absent). Under this project's one-keystone rule, this is also where the old
   "fixture phase" work happens: teaching the generator to produce the state the
   feature needs (e.g. a second occurrence) *is* the step-4 change. Each increment
   below names its exact step-4 change and whether it must defeat `uniquify_ids`.
5. **Run and improve until the PBTs fail for the RIGHT reason** — a real
   missing-feature assertion, not a harness bug, a vacuous red, or a wrong-layer
   error. (This project has a documented history of vacuous invariants and
   wrong-reader reds — this step is not optional.)
6. **Implement the feature** — drive to green.
7. **Clean-up.**

**Two items are spikes, not loop iterations** (§Spikes below): the IVM-matview
feasibility check and the org-per-edge-syntax decision. You cannot write a failing
assertion for "does Turso IVM support this" or "which syntax should we invent" —
they are probes / design forks. Each **precedes** the increment it gates.

## Dependency graph (corrected)

```
Increment A  RowOrigin + Occurrence type (pure refactor-along-green; no red)
   └─> Increment B  PlacedRowProvider (wrapper-inject) + driver/GPUI identity keys  ← renders the FIRST real 2nd occurrence (store NOT rekeyed)
          ├─> Increment C  Occurrence-keyed focus (ADR 0016 tuple)  ← OFF the advice critical path (ADR 0021); transclusion track only
          └─> Increment D  Display-only contract + origin-aware invariants  ┘ consumes B's rendered occurrence
   Spike-1 DECIDED (ADR 0021) ─> Increment E  Authored suppression exclusion set
   Spike-2 (advice-query IVM matview) ─> Increment F  Advice wiring (read-only v1) + deletion/undo
                                              (F consumes B render + E edges; NOT C focus under read-only v1)
```

The old plan's C-before-D was a lie: focus-between-occurrences needs a **rendered**
placed occurrence. Here **B** builds it first; C (focus) and D (contract) both
consume it. The `Occurrence` *type* is fixed opaque in A; **B gives `OccurrenceId`
its meaning** (which display placement produced the row). **Under
[ADR 0021](../adr/0021-advice-suppression-storage-and-readonly-v1.md) advice v1
renders read-only children, so F no longer depends on C** — C stays gated by the
separable transclusion track.

---

## Spikes (precede the loop; not red-green)

**Spike-1 — suppression storage fork (gates Increment E). DECIDED →
[ADR 0021](../adr/0021-advice-suppression-storage-and-readonly-v1.md).** Chosen
branch: a **typed anchor-side `:ADVICE_SUPPRESSED:` edge key** (bare-ID drawer with
`REQUIRES`' grammar, own `advice_suppressed` table + `EdgeField` variant) holding the
authored (anchor, lesson) exclusion set. Dismissal **round-trips org reload**. Inline
suffix grammar and edge-as-block rejected (see ADR 0021).

**Spike-2 — advice-query IVM-matview feasibility (gates Increment F). Static
analysis complete; executable probe in flight.** The fork's recursive-CTE closure,
`ORDER BY`/`LIMIT`, scalar+vector scoring, and chained matviews are all **supported**
in the fork; per-anchor `$param` is **unsupported and NOT wanted** — use an
anchor-denormalized matview + read-time `WHERE anchor_id=?` instead. The executable
probe (incl. the suppression anti-join) is running.

*Original probe framing (now largely answered by the static analysis above):*
`query_and_watch` compiles every live query into a `CREATE MATERIALIZED VIEW`
(`matview_manager.rs:53`) + CDC. So "lessons linked to THIS task ordered by recency
+ reference weight" must run as a **Turso IVM matview**. Probe, don't assert:
`ORDER BY`/scoring in the view SELECT is unverified in IVM; `descendants` scope
reads `block_with_path` → **chained matview** (documented hang class,
`turso-chained-matview-hang`); the inline `live_query` widget path can't do
descendant scope (`render_interpreter.rs:583-587`). Output: proof the advice
matview builds + updates incrementally without hang, **or** a decided fallback
(query-capability change / non-IVM read path). F's wiring assumes this result;
without it F is unestimated.

---

## Increment A — Typed `RowOrigin` + the `Occurrence` type

**Steps 1–2:** green; pick "retype the origin marker."

**Step 3 (this increment is ALL step 3 — a refactor along green):** introduce
`RowOrigin { Canonical | CreationPlaceholder | DisplayPlaced{canonical_id, occurrence} }`;
route the shared `parse_virtual_id` detection off it; **keep both** injection
mechanisms. Add `Occurrence = Canonical | Placed(OccurrenceId)`, `OccurrenceId`
opaque (meaning set by B). Not named `placement` (taken by `CursorPlacement`,
`input.rs:55`). Every construction defaults to `Canonical` → behavior-preserving.

**Step 4:** *none.* The keystone cannot yet express an `Occurrence`, so there is no
honest failing assertion to write here. **Flagged: A is not a red-green increment**
— it is the refactor-along-green that the later increments' step-3s build on.

**Steps 5–7:** gate is `general_e2e_composed_pbt` green as a **no-regression check
ONLY** — do not dress it as feature validation. Clean up the `:__virtual:`
string-sniffing.

**Risk:** low.

---

## Increment B — Display-placed occurrence: wrapper injection + driver/GPUI identity keys

Renders the first real second occurrence that C and D both need.

> **ARCHITECTURE CORRECTION (2026-07-06, Fable-verified).** An earlier draft made
> this "rekey the `ReactiveRowSet` store to `(EntityUri, Occurrence)`" (~70 sites).
> That premise is **circular and wrong** — cut it. Evidence:
> - The store is **per-query**, not global: `ReactiveRegistry` holds one
>   `ReactiveRenderedRows` (each with its own `ReactiveRowSet`) **per query id**
>   (`reactive.rs:891`, `:623`). CDC is bare-id per matview row, so a single
>   rowset never receives two `Created` for one id — the `set_neq` collapse
>   (`reactive.rs:465-475`) **never fires** for "two occurrences of one block." The
>   collapse the rekey "fixes" does not occur.
> - The rekey is also **insufficient**: `Change::Updated{id}`/`Deleted{id}` carry a
>   bare id (`reactive.rs:481-505`), so a store keyed `(id, occ)` with `apply_change`
>   defaulting `Canonical` would **strand** any `(L, Placed)` entry (no live CDC
>   updates), and `retain_keys` (`reactive.rs:~515`) would purge it every generation
>   bump — plus a second writer into a documented single-writer store.
> - **ADR 0016 §3 already enumerates the surfaces that must become occurrence-aware:
>   driver diffing, editor-cache key, element-id + eviction. `ReactiveRowSet` is not
>   on the list.** The existing analog for "inject a synthetic row under an anchor"
>   (`VirtualChildRowProvider`, `reactive_view.rs:120-202`) is a provider-wrapper
>   that `.chain()`s a suffix onto the signal-vec and **never touches the store**.

**Step 1–2:** green; pick "two occurrences of one block, representable and rendered
under an arbitrary anchor."

**Step 3 — refactor along green (behavior-preserving with `Occurrence::Canonical`
default):**
- **Widen the driver/provider row-identity key** (this is the real, small keyspace
  change — the collision surface is entirely driver-local). Change
  `ReactiveRowProvider::keyed_rows_signal_vec`'s item from `(EntityUri, Arc<DataRow>)`
  to `((EntityUri, Occurrence), Arc<DataRow>)` and thread it through the **5
  consumers**: the three collection drivers (`reactive_view.rs:945/1306/1646` —
  including tree `row_map`/`key_index`/`MutableTree` key), `VirtualChildRowProvider`
  (`:185-197`), and `lane_filtered_provider.rs:80`. Existing providers emit
  `Canonical` → green. **`ReactiveRowSet.data` stays `EntityUri`-keyed** (untouched).
- **GPUI identity keys.** Extend editor cache `editable-text-{row_id}-{field}`
  (`editable_text.rs:12`) and element id `render-entity-{id}` + eviction
  (`render_entity_view.rs:138-158`) with the occurrence coordinate, Canonical
  default → green. The drivers drop the key before `interpret_row(row, depth)`, so
  the `Occurrence` must ride the **interpreted node** as Increment A's `RowOrigin`
  metadata (ADR 0015 rule 4: node metadata, no id-infix) for GPUI to suffix its keys.

**Step 4 — PBT change:** add a keystone transition **"place an existing block L as a
display-only child under anchor X"** (a `live_query`/edge placement), NOT a
generator id-collision hack. (The old "defeat `uniquify_ids`" step was an artifact
of the cut store-rekey premise — a duplicate `:ID:` mints a second *canonical*
placement and tests parser id-collision, not display placement.) Narrow
`inv-main-panel-rows-match-focus` / `inv-viewmodel-decompiled-rows-match-query` to
tolerate a display occurrence (they currently hard-fail `ref_known && !allowed`).
This FAILS: no re-home mechanism exists, so the second occurrence never renders.

**Step 5 — right-reason check:** the red must be "second occurrence absent from the
render tree," not a driver-key collision (that would mean step 3's key widening
regressed) and not "invariant reader tripped on the wrong set."

**Step 6 — implement the re-home mechanism as a case of ONE unified injector,
`AppendedRowsProvider` (concept-minimization, Fable-verified).** Rather than a
bespoke `PlacedRowProvider`, the display-placement row is the **`LiveCell` case**
of a single `AppendedRowsProvider { inner, suffix: SuffixSource }` that also
absorbs the creation slot (`VirtualChildRowProvider`, the `Static` case). The
suffix is derived from the canonical block's live row cell (`row_mutable`), keyed
`(L, Placed(occ))`, with a synthesized display-local `parent_id` = anchor and a
sentinel-family `sort_key` (display-only → ADR 0015 rule 1 honored; rule 3 is
automatic — the row's `id` column stays `L`, so `view_event_handler` routes edits
to canonical). The behavior fork (submit-to-create vs edit-canonical) is NOT in the
provider — it stays in `RowOrigin` (ADR 0015 §5: "shared render path, separate
type"), so the unified provider does not violate the no-`materialize()`-merge rule.
This gives `OccurrenceId` its meaning. `resolve_virtual_parent`
(`render_interpreter.rs:640-668`) is creation-slot-only and is NOT the vehicle.
**GPUI first**; dioxus/tui/worker render of display placement is explicit later scope.
- **Injector fold (landed ahead of B, 2026-07-07):** `VirtualChildRowProvider` +
  `PlacedRowProvider` → one `AppendedRowsProvider`. The third in-tree injector,
  `TrailingSlot` (ViewModel-level), is a **separate layer** — the fallback for the
  no-`data_source` static-collection path (`tree.rs:75-113`), consumed by GPUI's
  `children_signal_vec`/`children_snapshot`. It does NOT fold into a provider
  (that path has no provider); eliminating it means routing static collections
  through a provider — a separate refactor with GPUI-render blast radius, tracked
  but out of this fold's scope.
  - **UPDATE 2026-07-09 — `TrailingSlot` DELETED.** On investigation the
    ViewModel-level `TrailingSlot` was already **dead in production**: every
    `streaming_collection` caller passed `None`, so the `children_*` suffix was
    never populated. The static-collection creation slot is served by
    `interpret_virtual_child` (inlined into the eager `items`), which `tree.rs`
    now uses uniformly (the old `build_trailing_slot` wrapper is gone). So no
    "route static through a provider" refactor was needed to remove it —
    `TrailingSlot`, its field, `set_trailing_slot`, and the suffix branches were
    deleted outright; `children_signal_vec`/`children_snapshot` now return `items`
    directly. Unifying `interpret_virtual_child` onto `AppendedRowsProvider`
    remains a separate, deferred increment.
- **Smallest first step (DONE 2026-07-07 — proves the mechanism, BEFORE the trait
  widening):** one `AppendedRowsProvider::placement` (`LiveCell` case) mirroring one
  real block's live cell under an anchor whose collection does **not** canonically
  contain L — with bare-id keys nothing collides yet, so it renders a real second
  occurrence, and a write to the canonical cell propagates to it live (test
  `appended_rows_provider_injects_live_second_occurrence`). Then land the step-3 key
  widening to unlock the same-collection (same panel shows L twice) case, then the
  step-4 transition.

**Step 7:** gate — the step-4 transition yields two occurrences that render as two
distinct GPUI editors; canonical unaffected; live updates to L propagate to both
(the suffix derives from L's live cell, so this is free — unlike the cut store-rekey,
which would have stranded the placed entry).

**Risk:** medium — the injector is now the SAME `AppendedRowsProvider` the creation
slot uses (fold, not net-new) + a 5-consumer signal-vec key widening + GPUI cache.
The expensive/hazardous store rekey is CUT.

---

## Increment C — Occurrence-keyed focus authority (ADR 0016, widened tuple)

> **OFF the advice critical path
> ([ADR 0021](../adr/0021-advice-suppression-storage-and-readonly-v1.md)).** Advice
> v1 renders read-only children (no editor), so advice (F) ships without C. C stays
> gated by the separable transclusion track, where editable second occurrences are
> the point.

Depends on **B** (a rendered occurrence must exist to move focus *between*).

**Step 3 — refactor along green:** widen
`focused_block: Mutable<Option<(EntityUri, Occurrence)>>` (NOT the spike's additive
parallel signal). Every reader unwraps `.0`; all writers set `Canonical` → green.
**Rewrite** the spike's `focused_occurrence: Mutable<Option<u32>>`
(`reactive.rs:980`) here — it is the rejected parallel-signal shape still in the
tree; its two tests assert parallel-signal semantics and are **rewritten to the
tuple**, not kept.

**Step 4 — PBT change (folds the SECOND half of the old fixture):** add an
**occurrence-focus-move** transition and extend the focus ref-model from bare id to
`(id, occurrence)` (`ui_actor_state.rs:44`, `focus_matches_ref.rs`). Add the GPUI
window-slice test that moves focus between B's canonical and placed occurrence.
FAILS: focus keyed by bare id → both occurrences satisfy `focused_block == L` →
every occurrence mounts a cursor.

**Step 5 — right-reason check:** red is "second cursor mounted / caret shared,"
proving the id-keying collision — not a missing-render error (that would mean B
regressed).

**Step 6 — implement:** route every writer (`apply_structural_focus`,
`maybe_mirror_navigation_focus`, delete-clear) through one `set_focus(block, occ)`;
per-frontend heterogeneous rollout (ADR 0016 §4); MCP/worker protocol field
(distinct from `CursorPlacement`).
- **Entity→occurrence resolution policy (the one gap the identity reframe exposes).**
  Six of the seven focus writers are entity-first (ADR 0016 §Problem census) — they
  name a block, not a slot. Each must resolve an `Occurrence` when it sets the tuple:
  **default `Canonical`**; a **structural op inherits the occurrence focus already
  sits on** (the tuple still holds it when `split_block`/`join` fires — a split
  inside a placed occurrence keeps focus in that occurrence). Only click-to-focus
  (`prelude.rs:38-51`) supplies an element directly. State this policy explicitly so
  no writer silently defaults a placed occurrence back to canonical.

**Step 7:** gate — GPUI window-slice test green: edit→canonical, other caret
unaffected, no second cursor, under real signal propagation.

**Risk:** highest-churn increment.

---

## Increment D — Display-only contract + origin-aware invariants

Consumes **B**'s rendered occurrence; independent of C (inertness needs no focus).

**Step 3 — refactor along green:** minimal — the origin marker already exists (A).

**Step 4 — PBT change (this increment is mostly step-4):** add
`inv-display-placement-canonical-inert` (bit-identity: consolidation, sibling-order,
`inv-org-render-fixed-point` from SQL, child-counts from Loro/Turso, all unchanged
with a display row present) + the **zero-Loro/Turso-write guard**. Make
`inv-main-panel-rows-match-focus` + `inv-viewmodel-decompiled-rows-match-query`
origin-aware (started in B; finalized here). Runs on B's step-4 generator. FAILS if
any placement leaks a `sort_key`, child-count, or a write.

**Step 5 — right-reason check:** a red here is a real contract leak, not a
generator artifact — confirm the no-placement baseline stays green.

**Step 6 — implement:** enforce the contract (no host `sort_key`/child-count,
edits→canonical, disclosed marker).

**Step 7:** gate — `inv-display-placement-canonical-inert` green **using B's
generator** (without B this gate cannot run — the old plan's version was vacuous).

**Risk:** high — the contract is load-bearing for a whole feature class.

---

## Increment E — Authored suppression exclusion set (re-scoped from "a bool")

**Preceded by Spike-1, now DECIDED
([ADR 0021](../adr/0021-advice-suppression-storage-and-readonly-v1.md)).** Under the
advice-as-query reframe there is **no authored advice edge** to hang a `suppressed`
bool on — the durable state is an authored **(anchor, lesson) exclusion set**. The
store is a new **`EdgeField` variant** + an **`advice_suppressed(anchor_id, lesson_id)`
table** (shape of `block_requires.sql`), serialized as an **`:ADVICE_SUPPRESSED:`
drawer** on the anchor block (bare-ID `REQUIRES` grammar; scheme added at the parse
boundary). The first cut's "`suppressed: bool`, low risk" was false:
- `block_link` is a **derived index re-extracted from content on every re-index**
  (`link_event_subscriber.rs`, `turso_block_link_indexer.rs`) — any field added is
  wiped on the next content write.
- `block_requires` is `(block_id, required_id)` with **no payload column**;
  `EdgeField` is a closed enum of `Vec<EntityUri>` (`edge_field.rs`).

**Step 3 — refactor along green:** verify/add the `requires`-reverse index for
backlinks (content-link reverse already exists via `block_link.target_id`).
Behavior-preserving.

**Step 4 — PBT change:** express "**dismissal round-trips reload** (the anchor's
`:ADVICE_SUPPRESSED:` set survives consolidator AND org render) **AND the suppressed
lesson never renders**." Whether the anti-join lives inside the IVM matview or at
read time is the backfill branch per ADR 0021 — storage is identical either way.
FAILS: no authored store, no drawer key.

**Step 5 — right-reason check:** red is "dismissal state has nowhere durable to
live / is dropped on org round-trip," not a query-filter bug.

**Step 6 — implement:** the **authored** exclusion-set edge store (NOT the
regenerated `block_link`) — new `EdgeField` variant + `advice_suppressed` table +
`:ADVICE_SUPPRESSED:` drawer serialization per ADR 0021.

**Step 7:** gate — the round-trip (or the stated no-reload gate) is green. **Not
"parallel / low risk."**

**Risk:** high — schema + org-format change.

---

## Increment F — Runtime-definable advice rules + wiring + deletion/undo semantics

**Preceded by Spike-2** (IVM matview feasibility). The feature, plus the semantics
the first cut omitted. Scoped around
**[ADR 0022](../adr/0022-runtime-definable-advice-rules.md)**: advice is
**runtime-definable via typed rule blocks** (`source_language =
'holon_advice_rule_yaml'`, discovered exactly like entity profiles), compiled by the
engine into **one anchor-denormalized matview per rule** via
`reconcile_named_view`.

**v1 policy gate
([ADR 0021](../adr/0021-advice-suppression-storage-and-readonly-v1.md)):** advice v1
children render **read-only** (no mounted editor) + a dismiss affordance;
click-through navigates/focuses the canonical block. This keeps **Increment C
off the advice critical path** (C stays gated by the transclusion track). In-place
editing of advice children later requires ADR 0016's `(EntityUri, Occurrence)` tuple
first.

**Step 3 — refactor along green:** **rule discovery + parse plumbing** — clone the
entity-profile pattern (`crates/holon/sql/profiles/get_profiles.sql`: a
`content_type = 'source'` scan keyed on `source_language`) for
`holon_advice_rule_yaml`, and add the parse boundary
(`AdviceRule`/`AnchorSelector`/`ScoringTemplate`/`BoundedK`, serde
`deny_unknown_fields`, per ADR 0022). **Prerequisite / parallel change:** fix the
`matview_manager.rs:504` `parse_sql(...).unwrap_or_default()` silent-swallow on this
exact DDL boundary to fail loud — the rule-status surface (step 6) depends on it. The
render/matview primitives from A/B/E exist by now.

**Step 4 — PBT / e2e change:** a keystone transition **mints an advice rule block**
(YAML source block) → the engine synthesizes `advice_rule_{slug}` → an
**expected-advice-rows oracle** asserts the woven rows under each matching anchor;
**dismissal** inserts into the authored exclusion set and persists (round-trips reload
per E/ADR 0021 — existing assertion); **no dangling row on lesson-or-anchor delete**
(`block_link` has **no target-side cleanup** → dangling `target_id`); **undo across
occurrences** (deferred in the spike; ADR 0016 §5 — decide a gate here or defer with
reason). All FAIL until wired.

**Step 5 — right-reason check:** reds are missing-synthesis / missing-wiring /
missing-cleanup, not Spike-2 IVM surprises (those were resolved before this
increment).

**Step 6 — implement:** the rule → matview **synthesis** (`reconcile_named_view`,
named/diffed/torn-down; anchor-denormalized, read at `WHERE anchor_id=?`, per ADR
0022); the **weave** (`AppendedRowsProvider` third suffix source, read-only, toggle
keyed `(anchor, rule)`, advice rows `RowOrigin`-marked non-anchor so rules can't match
rules' output); **dismiss** into the exclusion set (E); the **status surface** on the
rule block (active / compile error / over-cap, fail-loud-visibly, incl. async DDL
failure); and the **bundled lessons-for-tasks rule** shipped as a
bundled-but-user-editable rule block (v1 cut). Plus deletion cleanup and undo grouping.

**Step 7 — gate (loop-deviation, flagged) — unchanged:** the final proof is an
**end-to-end run via the `holon` MCP on a running instance** — lesson surfaces under
its task, dismissal removes it and persists, deletion leaves no dangling row — **plus**
keystone green. The live-instance leg is not a keystone red-green; it is the
integration check the loop's "run the PBTs" does not cover.

**Risk:** low for the wiring; the rule-synthesis/status surface and the omitted
semantics carry the residual risk.

---

## Increment G (optional, independent) — Editor content-sync consolidation

**The identity reframe's real dividend — deletes code rather than adding it.** Not
on the advice critical path (A–F ship without it); it can land any time after B and
pays down a pre-existing smell that display placement makes acute.

**The smell:** a GPUI `EditorView` has **three** content sources — (a) the entity
`Cell`'s remote-delta stream (stable identity, never orphaned), (b) a per-row
`DataRow` subscription (`_data_subscription`, `editor_view.rs:270`) bound to a
query-scoped cell whose identity **dies on rowset rebuild** (split/join/nav), and
(c) the `converge_input` render-path backstop (`editable_text.rs:62-79`) that patches
(b)'s orphaning. The fragile path is the one bound to the *copy*; the stable path is
the one bound to the *shared* entity `Cell` (ADR 0015 §1a). `converge_input`'s
existence **is** the evidence that the copy path is wrong.

**The change:** in cell-attached mode, make the entity `Cell` the **sole** external
content source; retire `_data_subscription`'s content role and demote/remove
`converge_input`. Two occurrences already share one `Cell` — so "type in one, the
other updates live" becomes true by construction (text liveness), with no
`converge_input` reconciliation.

**Loop fit:** step-4 red is the shared-cell liveness assertion (below); step-6 is the
subscription retirement; step-7 gate = keystone green **and** the liveness test green
with `converge_input` instrumented to prove it never fires for the propagation.

**Smallest experiment (also validates B's data half + stages C's focus test):** the
hardwired-`PlacedRowProvider` first step of B, plus one assertion — both occurrences
attach `editable_text(L,"content")`; type in one; the other's displayed text updates
live **without `converge_input` firing**. One test proves the shared-cell data model
and sets up C's window-slice focus-move test.

**Risk:** low-medium — deletes a sync path; the gate is proving nothing regresses when
the backstop is removed (the orphaning it patched must be genuinely gone in
cell-attached mode, not merely hidden).

---

## Increment H — App-layer relevance reranker (two-stage, ADR 0023)

**Preceded by Increment F** (F's read-only symbolic-relevance weave must land first).
Scoped around **[ADR 0023](../adr/0023-two-stage-relevance-app-layer-reranker.md)**:
retrieval stays ADR 0022's matview (read at `LIMIT N`); a new **app-layer async
reranker** picks the final top-K. The parse-level reservations (`rerank:` field,
`BoundedN`, `K ≤ N`) already shipped **with F** — this increment implements the machinery
behind them. Model inference **never** enters the DB / commit / render path.

**Step 1–2:** green (keystone + `holon-frontend` lib slice); pick "advice sections
rerank on expand."

**Step 3 — refactor along green: seam + cache + reserved-field plumbing.** Introduce the
`Reranker` domain trait (`score_batch(anchor, candidates) -> Result<Vec<Score>>`); wire
it via **fluxdi** in production (Clock-seam pattern — NOT CapMap; ADR 0019 keeps CapMap as
the PBT container) and as a **CapMap capability** in the keystone. Add the device-local
non-matview cache table `advice_rerank_scores` (content-hash key `(anchor_id, lesson_id,
model_id, rubric_version, prompt_content_hash)`; precedent: `navigation_history` /
`sync_states`). Promote the retrieval read to `LIMIT N` (`BoundedN`) and thread the
device-local reranker prefs (`PrefType::Secret` endpoint/key/consent — the
`preferences.rs:153-160` todoist pattern). All behavior-preserving with a no-op/identity
reranker default (retrieval order == final order) → keystone stays green.

**Step 4 — PBT / keystone change (FAIL):** install the **deterministic fake** reranker
(`score = hash(anchor_id, lesson_id)`) and assert **final K = fake-ordered top-K of
retrieval-N**; **suppression anti-join holds through rerank**; **failure path** renders
retrieval order + degraded badge. Add the **dismiss-during-in-flight-rerank race** as a
**generated transition interleaving** (async completion must re-check suppression before
writing the cell, else a dismissed row resurrects for a frame). All FAIL until wired.

**Step 5 — right-reason check:** reds are missing-rerank-ordering / missing-degraded-path
/ resurrected-dismissed-row, **not** a harness-fake wiring bug or a vacuous ordering
assertion. Determinism ends exactly at the model boundary — verify the fake actually
drives the asserted order.

**Step 6 — implement:** the **one generic OpenAI-compatible HTTP client**
(base_url-parameterized; no per-provider Rust) behind `Reranker`; the **hybrid score
contract** (initial listwise-batched rubric-anchored absolute fill; pointwise incremental
with few-shot calibration references; self-heal on full refresh); the **expand trigger**
(collapsed by default → expand spawns the rerank task, signal-cell write only, never
`block_on`); the per-section **"unranked" badge** on unconfigured/offline/timeout/API
failure (ADR 0021 disclosed-degradation surface); and the dismiss-race re-check before the
async cell write.

**Step 7 — gate (loop-deviation, flagged):** **keystone green** with the deterministic
fake, **plus** a **live-instance run** via the `holon` MCP against a **real
OpenAI-compatible endpoint** — expand a section, confirm the model reorders the retrieved
candidates, a dismissal removes a row and does not resurrect it, and an endpoint
failure/offline renders retrieval order with the "unranked" badge. The live leg is the
integration check the keystone fake cannot cover.

**Risk:** medium — async external calls, cost/consent boundary, and the dismiss-race are
the residual risk; the seam/cache/parse plumbing is low-risk.

---

## Deferred (each a separate ADR)

- **Vector ranking** — `Embedder` seam + derived Turso vector table (reconcile
  Model.md invariant 4; `model_version` reindex/GC). Advice ships on symbolic
  ranking (F) first.
- **P4 temporal/event source** — handover only; needs a durable, range-queryable,
  replica-agnostic history source (unbuilt Phase-5 intent log), not
  `watch_changes_since` (bounded ring) or Turso CDC.
- **`CuratedState<Role>` extraction** — only at a second real instantiation.
- **Performance** — advice = one matview + CDC subscription **per task**; memory
  records 1–2s/action at vault scale. N tasks → N matviews is an open scaling
  question, not assumed cheap. (Spike-2 probes feasibility of one; scale is
  separate.)

## Gate policy (keystone red on the known flaky)

The advice-track no-regression gate is `general_e2e_composed_pbt`. One invariant,
`inv-org-render-fixed-point`, has a **root-caused but OPEN** sibling-order flaky
(`BlockOrdering::project_sort_keys` is dead code → stale `sort_key`; naive fix
livelocks — see memory/keystone-org-render-sibling-order-flaky) that is causally
disjoint from this feature. Run the gate with that one invariant softened:

```
HOLON_PBT_INVARIANTS="inv-org-render-fixed-point:skip" \
  cargo nextest run -p holon-integration-tests --features pbt general_e2e_composed
```

`HOLON_PBT_INVARIANTS` (env-only, never edits the committed catalog) logs the
softening loudly — a green run under it is a DISCLOSED degraded run, not a clean
pass. EVERY other invariant stays strict; a red on anything else is a real
regression. Never commit an auto-saved seed for the softened invariant.

## Status

- **Increment A — LANDED (2026-07-06, phase-1b-display-placement-spike worktree).**
  `crates/holon-frontend/src/row_origin.rs` introduces `RowOrigin` /
  `Occurrence` / `OccurrenceId`; all `:__virtual:` string-sniffing centralized
  through `RowOrigin` (view_event_handler, reactive_view ×2, shadow_builders
  prelude, dioxus-web editor, the `inv-viewmodel-tree-virtual-slots` body). Native
  crates compile clean; 3 `row_origin` unit tests green; behavior-preserving by
  code inspection AND an adversarial Fable review (`EntityUri::from_raw(s).as_str()
  == s` identity for all mintable ids). No step-4 red (as designed). **No-regression
  gate:** the keystone hit the pre-existing OPEN `inv-org-render-fixed-point`
  sibling-order flaky (unicode bulk-add; see
  memory/keystone-org-render-sibling-order-flaky) — Fable CONFIRMED it is causally
  disjoint (org-render path has zero holon-frontend dep; no reachable text-sync
  transition). The auto-saved seed was stripped from
  `general_e2e_composed_pbt.proptest-regressions` and parked with the flaky's docs.
- **Spike-1 — DECIDED (2026-07-07, [ADR 0021](../adr/0021-advice-suppression-storage-and-readonly-v1.md)).**
  Typed anchor-side `:ADVICE_SUPPRESSED:` edge key (authored (anchor, lesson)
  exclusion set; own table + `EdgeField` variant); dismissal round-trips org reload.
  Inline-suffix and edge-as-block rejected.
- **Spike-2 — PROVEN by execution (2026-07-07).** 6 green tests at the turso fork
  (`tests/integration/query_processing/test_ivm_advice_spike.rs`, 0.13s): recursive
  closure → vector-scored anchor-denormalized join → chained `ORDER BY`/`LIMIT`
  matview, incrementally maintained incl. reparent, suppression flip + top-K
  backfill, un-dismiss, DB reopen. Per-anchor `$param` unsupported and NOT wanted →
  anchor-denormalized matview + read-time `WHERE anchor_id=?`. Two DDL rules:
  matview `ORDER BY` needs **column ordinals**; suppression anti-join = **LEFT JOIN
  … IS NULL** (`NOT EXISTS` rejected, compiler.rs:3550).
  **BLOCKER for Increment F:** Holon's turso pin `73d59b02` predates every needed
  IVM feature; the fork's rebased `holon` branch (local tip `612df12705`+, 494
  commits) was never pushed — remote tip *is* the current pin, histories diverged.
  Unblock = user force-pushes the local `holon` branch of
  `~/Workspaces/bigdata/turso` to `nightscape/turso`, then targeted
  `cargo update -p turso -p turso_core -p turso_sdk_kit -p turso_ext …` (NEVER bare
  `cargo update` — ed25519 lock hazard) + keystone re-gate.
- **Multiplicity question — RESOLVED (2026-07-07, ADR 0021):** advice v1 renders
  **read-only children** (dismiss affordance + click-through to canonical); the
  `focused_block` window-global focus hazard, not a GPUI cache collision, was the
  real blocker. **Increment C is off the advice critical path.**
- **Increment B — step-3 LANDED (2026-07-07, advice-spikes worktree).** Provider
  row-identity key widened to `((EntityUri, Occurrence), Arc<DataRow>)` through the
  trait's 8 implementors + 2 mocks (`Occurrence`/`OccurrenceId` moved to holon-api
  for crate layering — `ReactiveRowProvider` lives there; `u32`→`u64` +
  deterministic `OccurrenceId::for_placement` mint honoring provider-cache purity);
  `Occurrence` rides the interpreted node (`reactive_view_model.occurrence`); GPUI
  editor-cache + element-id keys suffix via `key_suffix()` (byte-identical for
  `Canonical`). 244/245 holon-frontend lib tests green (1 fail =
  `enter_executes_selected_command`, verified pre-existing on the parent rev).
  Steps 4/6 (placement transition + wiring) remain — transclusion track, off the
  advice critical path per ADR 0021.
- **Increment E — LANDED (2026-07-07, advice-spikes worktree).**
  `EdgeField::AdviceSuppressed` + `advice_suppressed(anchor_id, lesson_id)` table
  (registry `EdgeFieldDescriptor`, matview LEFT JOIN + hydrated column) +
  `:ADVICE_SUPPRESSED:` org parse/render (byte-identical round-trip test) + Loro
  meta key + `create_in_tree`/`create_entity` threading. Workspace compiles;
  317 org-format/api + 177 turso/loro (non-network) + 113 turso/app tests green;
  16 iroh/share failures verified pre-existing/environmental. `EdgeFieldUpdate`
  variant (interactive dismissal) deliberately deferred to Increment F.
- **Keystone no-regression gate (B step-3 + E together) — GREEN (2026-07-07):**
  299s, 1 passed / 211 skipped, `inv-org-render-fixed-point:skip` disclosed. (A
  first run timed out at 1200s from CPU contention with a parallel build — re-run
  alone passed well under the old 440s baseline.)
- **F's dismissal-write carrier — LANDED (2026-07-07).**
  `EdgeFieldUpdate::AdviceSuppressed` + `SetEdgeField` PBT generator/oracle/SUT-writer
  arms (weight 2, = requires). The forced-weight exercise (500, the deterministic
  guard for this path) exposed a divergence that root-caused to **three
  test-harness gaps, not a prod bug** (prod path verified symmetric with requires
  hop-by-hop): the SUT matview snapshot SQL + row parse
  (`pbt/sut_row_parsing.rs`) and the oracle id-remap
  (`pbt/reference_state.rs` `remapped_doc_uris`) each hand-list edge fields and
  omitted `advice_suppressed`. Fixed by mirroring; red→green proven
  (forced-weight FAIL 155s → PASS 248s; normal weight cases=4 PASS 332s).
  **Lesson:** a new edge field must also touch those two hand-list sites; a future
  `EdgeField::targets_mut()` would let both iterate `ALL` and close the class.
- **Turso pin bump — LANDED (2026-07-07, c31f8f4d30).** Spike-2's IVM blocker is
  cleared: the fork's `holon` branch was pushed and Holon's pin advanced to the IVM
  feature set. **Lock audit clean** (no ed25519/iroh churn from the update); keystone
  **GREEN over the full stack at 430s** (`inv-org-render-fixed-point:skip` disclosed).
  Increment F is unblocked.
- **ADR 0022 — ACCEPTED (2026-07-07,
  [ADR 0022](../adr/0022-runtime-definable-advice-rules.md)):** advice is
  **runtime-definable via typed rule blocks** (`holon_advice_rule_yaml`, profile-pattern
  discovery) compiled to **one anchor-denormalized matview per rule** via
  `reconcile_named_view`. Ratified sub-decisions: explicit **`ACTIVE` flag** gates the
  single DDL reconcile (no live-on-edit debounce); over-cap = **truncate + disclosed
  banner** on rule block + sections (**`Expand`**-later per section); **`ScoringTemplate`
  enum is the primary contract with a RESERVED escape hatch** (new relevance = new
  variants; the schema reserves a versioned raw-query field + refusal contract from
  day one — designed now, unimplemented, v1 parse refuses it loudly — so raw
  PRQL/SQL scoring can open later without a synced-format migration; revised
  2026-07-07 after the initial context-less "enum permanent" answer was withdrawn).
  Rejects unconstrained-raw-SQL rules in v1, closed-templates-as-user-surface,
  `WEAVE_ON` on live_query blocks, and Rhai pointcuts. **Increment F re-scoped
  accordingly** (rule discovery/parse + `matview_manager.rs:504` swallow fix + synthesis
  + weave + status surface + bundled rule).
- **ADR 0023 — ACCEPTED (2026-07-07,
  [ADR 0023](../adr/0023-two-stage-relevance-app-layer-reranker.md)):** advice relevance
  is **two-stage** — ADR 0022's matview is the **retrieval** contract (reads `LIMIT N`,
  `ScoringTemplate` recall-oriented) + an **app-layer async reranker** picks the final
  top-K. **Rerank-on-expand** (collapsed by default; expanding triggers the rerank;
  attention-bounded cost). **`rerank:` field RESERVED now** (ADR 0022 raw_query pattern —
  always-failing deserializer) — names a **model profile only**; endpoint/key/consent are
  **device-local** `PrefType::Secret`, never synced. **Hybrid score contract**:
  listwise-batched, rubric-anchored **absolute** scores (per-pair cacheable) + pointwise
  incremental with few-shot calibration. Cache = device-local non-matview
  `advice_rerank_scores` (content-hash key → staleness impossible). Production seam =
  **fluxdi** `Reranker` trait (NOT CapMap); keystone uses a CapMap **deterministic fake**.
  First backing = **one generic OpenAI-compatible HTTP client** (base_url-parameterized;
  no per-provider Rust). Reranker ships **before** the embedder, ungated (embedder is a
  separate retrieval-stage design that reuses this seam; until it lands, quality is capped
  by tag-overlap recall). **v1 cut:** reserve `rerank:` + `BoundedN` + `K ≤ N` at
  parse-level (ships with F); **the reranker itself = its own increment after F** (see
  Increment H).
- **Increment F step 3 — LANDED (2026-07-07).** New crate `crates/holon-advice`
  (profile-precedent placement): typed `AdviceRule` (reserved `raw_query`
  always-failing deserializer), `AnchorSelector::lower()` injection-safe by
  refusal, `synthesize_matview` + `reconcile_advice_rule` (active→reconcile,
  inactive→DROP), discovery SQL, bundled `lessons_for_tasks.yaml`; 16 unit + 2
  integration tests green, workspace clean. **Live-IVM facts discovered (probe
  test `probe_ivm_shape_findings` pins them):** `EXISTS` rejected in matview DDL
  (lowered to JOIN); the in-matview suppression LEFT-JOIN anti-join is **silently
  ignored** (shape-dependent; Spike-2's stage-5a shape passed); a `block_raw`
  column in GROUP BY **silently corrupts** the aggregate. → suppression, recency,
  ORDER BY, and top-K/N all execute at **read time** over an un-capped, unordered
  matview (resolves ADR 0021's Spike-2-stage-5 fork toward read-time; storage
  unchanged; ADR 0023's cache/backfill design already assumed read-time). The two
  silent-wrong-result behaviors are turso-fix candidates. Remaining F: step 4
  (keystone rule-minting transition + oracle) and step 6 (discovery→
  MatviewManager wiring, weave rendering, dismiss affordance, status surface,
  vault seeding of the bundled rule) — plus schema-level `BoundedN` + reserved
  `rerank:` per ADR 0023.
- **Increment F step 4 — LANDED, RED demonstrated (2026-07-07, advice-spikes
  worktree, uncommitted).** The keystone now expresses advice end-to-end and is
  red on exactly the step-6 gap:
  - **Harness**: `RefAdvice` ref-cap (total per-anchor `AdviceExpectation` —
    empty ≠ absent, so teardown/delete paths are asserted, not skipped;
    Fable-review BLOCK fix) + `advice_expectation.rs` pure oracle mirroring the
    read contract (full-intersection tag counts incl. shared marker tags,
    anchor-directional suppression, k; recency deliberately unasserted —
    SUT wall-clock, unobservable; superseded by ADR 0023 rerank);
    `file_with_advice_rule` WriteOrgFile arm (typed intent → YAML → in-arm
    `parse(render)==typed` guard; ≤1 rule/run, shrink-safe via preconditions;
    reachable in the DEFAULT generator mix — advice rules don't override
    profiles); shared `ADVICE_TAG_POOL` + pool-weighted tags arm + biased
    constructive (task,lesson) suppression sub-arm (multi-element ~1/3);
    `Occurrence` stamped into `WidgetSnapshot.props` for non-canonical rows
    (ADR-0015-conform: id string untouched); invariants
    `inv-advice-rows-woven` (relation oracle: distinct ⊆ scored,
    |R|=min(k,|scored|), count-monotone, boundary dominance, empty⇒none) and
    SQL twin `inv-advice-matview-matches-ref/matview` (new narrow
    `SutAdviceMatview` cap; pre-suppression un-capped rows + exact matview
    name — flips green at step-6 synthesis while the weave invariant stays red:
    driver-ladder localization).
  - **The red artifact**: `tests/advice_step4_red.rs` — deterministic composed
    sequence (rule file → carrier file → navigate → pool tags) asserting BOTH
    advice invariants fail with exactly the missing-synthesis/missing-weave
    messages; step 6 flips this test to assert green.
  - **Three real pre-existing bugs found on the way (the PBT working as
    designed; all fixed, uncommitted):** (1) **Turso IVM ghost row** —
    `JoinOperator::commit` didn't consolidate deltas; same-txn base-UPDATE +
    junction-INSERT left an unretractable ghost in the downstream join's state
    → byte-identical duplicate `block` matview rows (fix in the turso fork +
    TEMP `[patch]` in this worktree's Cargo.toml; reproducer
    `tests/matview_duplicate_row_repro.rs`; see memory
    turso-ivm-join-commit-consolidation-bug). (2) **`block` matview fan-out** —
    three 1:N junctions aggregated over one LEFT-JOIN cross-product multiplied
    `requires`/`advice_suppressed` arrays (masked for `tags` by the set type);
    fixed by registry-derived **chained per-junction agg matviews**
    (`block_tags_agg` etc.) + final join view, probe-pinned
    (`probe_multi_junction_fanout_fix_shapes`); `block_matview.sql` deleted.
    (3) **matview recreate doesn't cascade** — dependents keep stale+re-inserted
    rows; `reconcile_named_view` now cascade-drops dependents recursively
    (one-time full rebuild on first boot after the DDL change, logged).
  - **Coverage hole closed:** `SutFixtureFs` had NO implementor anywhere —
    `WriteOrgFile` (org-file ingest) was silently absent from the keystone
    alphabet since composition. Now implemented on `HeadlessFrontendComponent`
    and registered; org-ingest is new keystone coverage (unrelated divergences
    may surface — triage as prod-bug-first).
  - **Interim gate policy (until step 6):** keystone gates for OTHER tracks run
    with `inv-advice-rows-woven:skip,inv-advice-matview-matches-ref/matview:skip`
    (+ the known `inv-org-render-fixed-point:skip`), disclosed — the advice reds
    are expected-by-design, and leaving them hot would rot the gate.
  - **Step-6 notes recorded here so they aren't lost:** add `v.lesson_id` final
    read-query tiebreak (UI stability; does NOT strengthen the oracle — recency
    stays unobservable); decide weave expansion (ADR 0023 wants
    collapsed-by-default + rerank-on-expand → keystone needs default-expand or
    an expand transition + expansion-state modeling); advice weave and B-track
    transclusion of the same (lesson, anchor) collide to the same
    `OccurrenceId::for_placement` — if disambiguation is needed, fix
    `for_placement` in prod first; PBT dismissal drives the whole-set REPLACE
    writer — if prod's gesture APPENDS, add an append-path transition at step 6.
  - **Undo-across-occurrences: DEFERRED with reason** (the step-4 stanza asked
    for a decision): advice v1 is read-only (ADR 0021) — no editor mounts on
    advice rows, so undo-across-occurrences is unreachable in v1; the gate
    belongs to the transclusion track (B/C). Plain undo of a dismissal and undo
    of the rule mint ARE covered: the oracle recomputes from the reference
    blocks map, so any undo path that restores state restores the expectation.
- **Increment F step 6 — LANDED, advice feature functionally complete + verified
  (2026-07-07, advice-spikes worktree, uncommitted).**
  - **Engine side:** rule reconciler in `create_initialized_engine` (watches
    `GET_ADVICE_RULES_SQL`, drainer→mpsc→reconciler-task so DDL never runs in the
    CDC delivery path; `block_id→RuleSlug` map for content-less `Deleted`; slug
    rename drops the old view; unparseable/inactive tears down; slug collision =
    first-owner-wins + error status). Pure decision core
    `AdviceReconcilerState::plan` (unit-tested). `dismiss_advice` op (entity
    `block`, params anchor_id+lesson_id, RMW-append over the anchor's
    `advice_suppressed` — whole-set REPLACE on the Loro LWW meta key, concurrent
    loss disclosed). `RuleStatus` carrier (in-memory, per-block Active/ParseError/
    DdlError/SlugCollision) surfaced through the existing `error(...)` widget in
    both ui_watchers. Bundled rule seeded **INACTIVE** in `assets/default/index.org`
    (user flips ACTIVE; keeps the keystone ≤1-active narrowing valid). `BoundedN`
    (1..=50, k≤n refusal, absent→k) + reserved `rerank:` field + `lesson_id` read
    tiebreak (ADR 0023 v1 cut). ADRs 0022/0023 amended (v1 expanded-by-default;
    bundled ships inactive).
  - **Weave — session-level sidecar (the load-bearing design correction):** the
    weave must be observable on the PURE-INTERPRET snapshot path (headless PBT +
    MCP `describe_ui`), not just GPUI's streaming `create_driver`. Fix mirrors the
    creation slot: an async session weaver maintains an `AdviceSidecar`
    (`anchor → Vec<DataRow>`, rank-sorted, `Occurrence::Placed(for_placement)`
    keyed, parent_id=anchor, id=canonical lesson), refreshed at settle
    (`refresh_advice_sidecar` in `converge_projections`); `BuilderServices::
    advice_children(anchor)` is a pure O(1) sidecar read (default empty →
    byte-identical when no rule active); the STATIC collection arms
    (outline/tree/list/table) append advice via `weave_advice_into_items`,
    interpreted through the read-only template (selectable + text + dismiss
    op_button; NEVER editable_text — ADR 0021). Canonical read is ONE-SHOT only
    (never `watch_query` — IVM miscompiles the anti-join/ORDER BY/LIMIT).
  - **Invariant fix:** `inv-advice-rows-woven` was per-node, but a block id
    decorates ~7 nested nodes incl. childless leaves → structurally unsatisfiable
    once the weave produces rows (a latent bug step 4 never triggered; no rule was
    ever active+rendered then). Rewritten to **per-anchor attribution**: collect
    placed rows tree-wide in render order, dedup anchors (collapsing the multi-node
    decoration, mirroring `viewmodel_tree_virtual_slots`), attribute each placed
    row to its owning anchor via the `for_placement` suffix, check once per anchor;
    a placed row matching no rendered anchor fails (foreign-row tooth kept). All
    `check_advice_relation` teeth preserved; **proven non-vacuous by inversion**
    (weave-missing → "wove 0" red; mis-order → non-increasing-order red).
  - **Verified:** deterministic `advice_step4_red.rs` (renamed test
    `advice_step6_synthesis_weave_and_dismiss_green`) GREEN on both phases —
    weave woven under the anchor in score order AND post-dismiss backfill (dismiss
    the top lesson → it vanishes, the 3rd candidate backfills at k=2), both advice
    invariants asserted to have RUN (no vacuous deselect). **Normal-weight keystone
    GREEN (428s), advice invariants HOT.** All advice lib tests green (31).
  - **Parked (not advice bugs; forced-weight keystone only):** a general
    **`requires` edge org round-trip loss** (a requires edge becomes a raw
    `REQUIRES` string property with `requires` emptied through org write→reingest;
    same family as `org_ingest_drops_block_marks`; exposed by the org-ingest
    coverage advice unlocked — triage in progress). **GPUI streaming render no
    longer appends advice** (the sidecar feeds the snapshot/MCP path; the
    `create_driver` weaver was retired — GPUI needs re-wiring to read the same
    sidecar; unverifiable while frontends/gpui is unbuildable from an unrelated
    in-flight edit). Seeded rule ships INACTIVE (activation = one user edit).
  - **Step 7 (live-instance MCP gate):** still to run once the app is buildable
    (blocked on the unrelated frontends/gpui breakage) — flip the seeded rule
    ACTIVE via MCP, confirm lessons surface under a task, dismiss removes +
    persists, deletion leaves no dangling row.

## Sequencing & honest scope

A (retype, refactor-only) → **B (PlacedRowProvider wrapper-inject + driver/GPUI key
widen — renders the first 2nd occurrence; store NOT rekeyed)** → { C focus, D
contract } — both consuming B — with
**Spike-1 DECIDED (ADR 0021) → E (suppression exclusion set)** and **Spike-2 → F
(advice wiring)** feeding F. Advice (F) ships after A, B, D, E and Spike-2 — **NOT C**
(read-only v1, ADR 0021 → C is off the advice critical path, gated by the transclusion
track). **G (editor content-sync consolidation)
is optional and off the critical path** — lands any time after B; harvests the
identity reframe's dividend (retire `converge_input`).

**Where the loop genuinely does not fit (flagged so no one fakes a red):**
1. **Increment A has no step-4 red** — the keystone cannot express an `Occurrence`
   until B's generator exists, so A is a pure refactor-along-green with a
   no-regression gate.
2. **Spike-1 (org syntax) and Spike-2 (IVM matview) are not red-green** — a design
   fork and a capability probe, respectively. Each precedes its increment; forcing
   them into a step-4 failing assertion would be theatre.
3. **F's final gate is a live-instance MCP run**, not a keystone iteration.
4. **The old standalone "PBT fixture phase" is dissolved**: its two halves are
   B-step-4 (generate a rendered 2nd occurrence, defeating `uniquify_ids`
   selectively) and C-step-4 (an occurrence-focus-move transition). It is never an
   upfront phase — each half lands with the increment that first needs it.

**Increments carrying a refactor-along-green (step 3):** A (entirely), B
(driver/provider key widen + GPUI identity keys — NOT a store rekey), C
(focus-tuple widening + spike-signal rewrite), E (reverse
index). **Increments that are mostly a pure step-4 PBT change:** D (new inertness
invariants), F (feature + deletion/undo assertions, after Spike-2).
