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

## What advice actually is

A **source block (P1)** anchored to a task, whose query selects lesson blocks via
**reference edges (P3)** ranked by a **symbolic score**, rendered by **display
placement (P2)**; dismissal flips the edge's `suppressed` state. The honest
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
          ├─> Increment C  Occurrence-keyed focus (ADR 0016 tuple)          ┐ both consume B's
          └─> Increment D  Display-only contract + origin-aware invariants  ┘ rendered occurrence
   Spike-1 (org per-edge syntax fork) ─> Increment E  Authored edge store + suppressed
   Spike-2 (advice-query IVM matview) ─> Increment F  Advice wiring + deletion/undo
                                              (F consumes B render, C focus, E edges)
```

The old plan's C-before-D was a lie: focus-between-occurrences needs a **rendered**
placed occurrence. Here **B** builds it first; C (focus) and D (contract) both
consume it. The `Occurrence` *type* is fixed opaque in A; **B gives `OccurrenceId`
its meaning** (which display placement produced the row).

---

## Spikes (precede the loop; not red-green)

**Spike-1 — org per-edge-property syntax fork (gates Increment E).** Org has
**zero** per-edge property serialization today: `REQUIRES` renders as a bare-ID
drawer (`models.rs:710-721`, `parser.rs:414-423`), links are inline `[[…]]` marks.
A `suppressed` flag has nowhere to round-trip. This is a **design fork**, not a
failing test: decide (a) invent per-edge org property syntax so `suppressed`
survives a reload, **or** (b) explicitly accept "dismissal does not survive reload"
and encode that limitation in E's gate. Output: a one-paragraph ADR addendum fixing
the choice. E's step-4 PBT is written against whichever branch wins.

**Spike-2 — advice-query IVM-matview feasibility (gates Increment F).**
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

**Step 6 — implement the re-home mechanism (net-new): a `PlacedRowProvider` in the
`VirtualChildRowProvider` mold.** Wrap the anchor collection's provider; chain a
suffix derived from the placement query's `ReactiveRenderedRows` live cell
(`row_mutable` → one-element `SignalVec`), keyed `(L, Placed(occ))`, with a
synthesized display-local `parent_id` = anchor and a sentinel-family `sort_key`
(display-only → ADR 0015 rule 1 honored; rule 3 is automatic — the row's `id` column
stays `L`, so `view_event_handler` routes edits to canonical). This gives
`OccurrenceId` its meaning. `resolve_virtual_parent` (`render_interpreter.rs:640-668`)
is creation-slot-only and is NOT the vehicle. **GPUI first**; dioxus/tui/worker
render of display placement is explicit later scope.
- **Smallest first step (proves the mechanism in ~a day, BEFORE the trait widening):**
  hardwire one `PlacedRowProvider` mirroring one real block's live cell under an
  anchor whose collection does **not** canonically contain L — with bare-id keys
  nothing collides yet, so it renders a real second occurrence immediately. Then land
  the step-3 key widening to unlock the same-collection (same panel shows L twice)
  case, then the step-4 transition.

**Step 7:** gate — the step-4 transition yields two occurrences that render as two
distinct GPUI editors; canonical unaffected; live updates to L propagate to both
(the suffix derives from L's live cell, so this is free — unlike the cut store-rekey,
which would have stranded the placed entry).

**Risk:** medium — a net-new provider-wrapper (well-precedented by
`VirtualChildRowProvider`) + a 5-consumer signal-vec key widening + GPUI cache. The
expensive/hazardous store rekey is CUT.

---

## Increment C — Occurrence-keyed focus authority (ADR 0016, widened tuple)

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

## Increment E — Authored edge store + `suppressed` (re-scoped from "a bool")

**Preceded by Spike-1** (org syntax fork). The first cut's "`suppressed: bool`, low
risk" was false:
- `block_link` is a **derived index re-extracted from content on every re-index**
  (`link_event_subscriber.rs`, `turso_block_link_indexer.rs`) — any field added is
  wiped on the next content write.
- `block_requires` is `(block_id, required_id)` with **no payload column**;
  `EdgeField` is a closed enum of `Vec<EntityUri>` (`edge_field.rs`).

**Step 3 — refactor along green:** verify/add the `requires`-reverse index for
backlinks (content-link reverse already exists via `block_link.target_id`).
Behavior-preserving.

**Step 4 — PBT change:** express "flipping `suppressed` removes the placement, and
the state round-trips through the consolidator AND org render" — *written against
Spike-1's chosen branch* (real round-trip, or the explicit no-reload gate). FAILS:
no authored store, no org syntax.

**Step 5 — right-reason check:** red is "dismissal state has nowhere durable to
live / is dropped on org round-trip," not a query-filter bug.

**Step 6 — implement:** an **authored** edge store (NOT the regenerated
`block_link`) carrying `suppressed`, plus Spike-1's org syntax (or the accepted
limitation).

**Step 7:** gate — the round-trip (or the stated no-reload gate) is green. **Not
"parallel / low risk."**

**Risk:** high — schema + org-format change.

---

## Increment F — Advice wiring + deletion/undo semantics

**Preceded by Spike-2** (IVM matview feasibility). The feature, plus the semantics
the first cut omitted.

**Step 3 — refactor along green:** minimal; the primitives exist by now.

**Step 4 — PBT / e2e change:** express the composed feature and the omitted
semantics: a lesson surfaces under its task; dismissal flips `suppressed` and
persists (subject to E's reload decision); **deleting the canonical lesson or
anchor task while a display row is live** leaves no dangling row (`block_link` has
**no target-side cleanup** → dangling `target_id`); **undo across occurrences**
(deferred in the spike; ADR 0016 §5 — decide a gate here or defer with reason). All
FAIL until wired.

**Step 5 — right-reason check:** reds are missing-wiring / missing-cleanup, not
Spike-2 IVM surprises (those were resolved before this increment).

**Step 6 — implement:** wire the source block (P1) anchored to a task, query =
lessons via edges (E) ordered by symbolic score, rendered by display placement (B);
dismissal flips `suppressed` (E); deletion cleanup; undo grouping.

**Step 7 — gate (loop-deviation, flagged):** the final proof is an **end-to-end run
via the `holon` MCP on a running instance** — lesson surfaces under its task,
dismissal removes it and persists, deletion leaves no dangling row — **plus**
keystone green. The live-instance leg is not a keystone red-green; it is the
integration check the loop's "run the PBTs" does not cover.

**Risk:** low for the wiring; the omitted semantics carry the residual risk.

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

## Sequencing & honest scope

A (retype, refactor-only) → **B (PlacedRowProvider wrapper-inject + driver/GPUI key
widen — renders the first 2nd occurrence; store NOT rekeyed)** → { C focus, D
contract } — both consuming B — with
**Spike-1 → E (edges/org)** and **Spike-2 → F (advice wiring)** feeding F. Advice
(F) ships only after A–E and both spikes. **G (editor content-sync consolidation)
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
