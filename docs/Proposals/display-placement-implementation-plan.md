# Implementation plan: display-placement + advice-as-composition

Companion to [ADR 0015](../adr/0015-computed-placement-and-curated-state-primitives.md).
Builds the "resurface lessons / advice" use case out of composable primitives,
never a bespoke advice type.

**This plan was rewritten after a code-grounded senior review** that rejected
the first draft. Two lessons baked in below:
1. **Surface the kill-switch first.** The feature's viability hinges on whether
   a display-placed *editable* block can coexist with its canonical occurrence
   without focus/caret collision. That spike runs **first**, throwaway, before
   any refactor — if it's infeasible, the feature is dead and we spent days, not
   weeks.
2. **Focus re-keying is its own ADR, not a phase here.** ADR 0010 explicitly
   reserves multi-occurrence focus for a dedicated decision. This plan proves
   feasibility (Phase 1b) and then **hands off**; the full rollout is out of
   scope.

**Guiding constraints (from ADR 0015):** canonical placement stays a stored,
consolidator-minted scalar; display placement is computed, display-only, never
merged. Ship on symbolic ranking first. Vectors, P4 (temporal source), and the
`CuratedState<Role>` extraction are each a separate later ADR.

---

## Status (2026-07-15)

**PARTIALLY LANDED.** Phase 1b spike (focus-collision) was greenlit (PROCEED,
2026-07-06). Phase 0 `RowOrigin` retype landed and is in production. The advice
plan was rewritten and split into its own document
(`advice-feature-implementation-plan.md`), which is landed through step 6.

Still open:
- Phase 1a inert-render bit-identity invariant (WIP behind env var)
- Focus re-keying (separate ADR, not yet started)
- Phase 3 reference edges + curation
- Phase 4 advice-as-configuration

## Phase 1b — Focus-collision spike: the real go/no-go (FIRST, throwaway)

**Goal:** prove that an **editable, focusable** display-placed occurrence of a
real block can coexist with its canonical occurrence — focus/caret/undo
addressable per-occurrence — with throwaway wiring. This is the decision that
kills or greenlights everything.

**Why first:** focus and caret are keyed today by bare `EntityUri`
(`reactive.rs:953` `focused_block: Mutable<Option<EntityUri>>`;
`set_focus_with_caret` `reactive.rs:274`; caret seed
`headless_editor_mirror.rs:132/209`). Two editable occurrences of one id would
share focus/caret. The whole feature stands or falls on whether occurrence-keyed
focus is achievable. Everything else (refactor, edges, ranking) is wasted if it
isn't.

### The collision — two surfaces

- **Surface 1 (what 1b tests): focus/caret.** `focused_block:
  Mutable<Option<EntityUri>>` (`reactive.rs:953`) is one global keyed by bare
  id. The headless mirror's `handle_keystroke` (`headless_editor_mirror.rs:127`)
  reads `engine.focused_block()`, keys its cursor map by the `block_id` **string**,
  and resolves `editable_text(&block_uri,"content")` to the one canonical
  `MutableText`. Two occurrences of `L` both satisfy `focused_block == L` → shared
  cursor/caret.
- **Surface 2 (flag, don't fix here): collection row identity.**
  `keyed_rows_signal_vec` returns `(EntityUri, DataRow)`; the GPUI collection
  drivers (`reactive_view.rs:945/1306/1646`) diff by that key → duplicate keys
  collide on move/remove. Separate, GPUI-shaped surface the focus ADR must also
  own. The 1b de-risk check confirms whether it manifests in headless.

### Target layer: headless (real authority, one frontend)

Drive production `focused_block` + `HeadlessEditorMirror` + `editable_text` + the
existing `apply_focus_editable_text` (= `click_entity`) — **not** a GPUI-only
local `Mutable`. **A spike that isolates two carets via a throwaway local
`Mutable` is a FAIL of the gate's intent** — two carets are always isolable with
enough hacking; that proves nothing. The question is whether the *real* authority
carries `(id, occurrence)` within a bounded blast radius, i.e. without forcing
ADR 0010's `MutableBTreeMap<Region,…>` graduation across all four frontends.

### The throwaway occurrence key

`Occ = u32` (canonical `L` = `Occ 0`, display placement = `Occ 1`). No `RowOrigin`
(Phase 0), no occurrence model (focus ADR) — hand-rolled. **Which shape is
required is the finding:**
- **(b) additive, try first:** add `focused_occurrence: Mutable<Option<Occ>>`
  *alongside* `focused_block` — no type change to the ~10 existing readers.
- **(a) widened, fallback/measure:** `focused_block: Mutable<Option<(EntityUri,
  Occ)>>` — touches every reader; this is the ADR-0010-graduation-shaped path.

### Build (throwaway branch, headless only)

1. **Injection:** a test-only row-provider wrapper appending a row with
   `entity_id = L` under anchor `A`, tagged `Occ 1`.
2. **Focus authority:** additive `focused_occurrence` on `UiState`;
   `set_focus(Some(L), occ)` sets both handles.
3. **Mirror routing:** `handle_keystroke` keys its cursor map by `(block_id,
   occ)` but resolves `editable_text` by the **canonical** `EntityUri` — caret
   per-occurrence, write to canonical home (contract rule 3).
4. **Entry:** drive via `apply_focus_editable_text` + type actions.

### Assertions (the proof)

Focus `(L, Occ 1)`, type `"x"`: (a) `editable_text(L)` gained `"x"` — **edit
routed to canonical home**; (b) canonical `(L, Occ 0)` cursor **unchanged**;
(c) the injected occurrence produced **zero** extra `editable_text`/create — no
phantom block (ties to Phase 1a's no-write guard). Undo grouping only if headless
undo is wired; else defer.

### Gate — Go / No-Go (bounded vs graduation, not possible vs impossible)

- **PROCEED to Phase 0** if additive `focused_occurrence` suffices, routing is
  correct, and the change stays contained to headless (no `focused_block` type
  change, no frontend/MCP edits).
- **STOP → write the focus ADR first** if correct routing *requires* widening the
  global focus type and threading `Occ` through the collection driver + all four
  frontends + the MCP focus surface. Not "impossible" — "unbounded"; it confirms
  0015's conditional status and *sizes* the focus ADR.

### De-risk RESOLVED (2026-07-06, by code inspection)

Which surfaces the headless path hits — answered without building the injection
(the proposed keystroke probe *is* spike-step-1; the resolution path answered it
more definitively):

- **Surface 2 does NOT manifest in headless.** `HeadlessFrontendComponent::widget_tree_snapshot`
  resolves via the recursive **static** `ReactiveEngine::snapshot()`
  (`reactive.rs:1419`), collecting rows through `rows_snapshot` (Vec-append),
  **not** `keyed_rows_signal_vec`. The keyed-diff collision is GPUI-live-driver
  only → headless cleanly isolates the focus question. Surface 2 is the focus
  ADR's (GPUI rollout) problem.
- **The static snapshot's `VISITED` cycle guard is SCOPED** (RAII `Guard::drop`
  removes the id on exit, `reactive.rs:1469-1484`), so it permits two *non-nested*
  occurrences of `L` — headless can render the canonical + display occurrence.
- **Bonus:** a display placement that nested `L` within `L` renders the inner as a
  `↺ self-reference` placeholder (`reactive.rs:1456`) — free graceful termination
  of infinite transclusion; keep as a contract note.
- **Surface 1 (focus/caret) IS reachable in headless** (mirror keys by bare
  `block_id`) — the spike's target, confirmed present.

**Conclusion:** headless is the correct slice — it exercises Surface 1 (the real
question) while structurally excluding Surface 2. Proceed to build the throwaway
occurrence-key spike as scoped above.

**Out of scope:** GPUI/dioxus/tui/worker rollout, the MCP focus surface, the real
`RowOrigin` type, the collection-driver key redesign (Surface 2), cross-occurrence
undo.

**Risk:** this *is* the risk. Throwaway code; the answer is what matters.

### Phase 1b RESULT (2026-07-06, worktree `phase-1b-display-placement-spike`) — PROCEED

The spike was built and the go/no-go is **PROCEED (bounded, additive)**:

- **Compiles** (`holon-frontend` lib + lib-test). Additive `focused_occurrence:
  Mutable<Option<u32>>` on `UiState` + `ReactiveEngine` accessors; the
  `HeadlessEditorMirror` cursor map re-keyed to `(block_id, Option<u32>)`;
  `handle_keystroke` keys the caret by occurrence while `editable_text(&block_uri)`
  stays keyed by the canonical id.
- **Occurrence-independence proven** (unit test
  `spike_phase_1b_tests::occurrence_keyed_cursors_are_independent`): two
  occurrences hold independent carets; moving/forgetting one leaves the other.
- **Write-routes-to-canonical proven END-TO-END** (integration test
  `spike_display_occurrence_write_routes_to_canonical`, Loro-backed
  `HeadlessFrontendComponent`): focus canonical → type `A`, switch to display
  occurrence `Some(1)` → type `B`, and canonical `block_raw.content` becomes
  `c1AB` — the write resolves by block id regardless of occurrence.
- **No regression** — 16/16 existing `holon-frontend` focus tests green.
- **Blast radius = BOUNDED/ADDITIVE:** 2 files, `focused_block`'s type UNCHANGED
  (~10 readers + all four frontends untouched), **zero external call-site churn**
  (`None` = canonical). This is NOT the ADR-0010 `MutableBTreeMap<Region,…>`
  graduation → P2's focus prerequisite is contained, not a four-frontend rewrite.

**Remaining (not blocking the verdict):** the render-layer injection of a real
second occurrence (the spike drives occurrence via `set_focus_occurrence`
directly, not yet a rendered display-placed row); Surface 2 (GPUI keyed-row
diffing) and the real four-frontend + MCP focus rollout remain the separate focus
ADR's job.

---

## Phase 1a — Inert-render bit-identity (necessary, NOT sufficient)

**Goal:** prove a display-placed row is inert w.r.t. the canonical projection.
Demoted from "go/no-go" — a **non-editable** render row is inert *regardless of
the hard part*, so a green here says nothing about focus.

**Changes:** inject a non-editable `RowOrigin::DisplayPlaced` row under an
anchor. New invariant `inv-display-placement-canonical-inert`: consolidation,
sibling-order, `inv-org-render-fixed-point` (reads **from SQL**), and
child-counts (read **Loro/Turso**) bit-identical to the no-placement run.

**Gate:** the new invariant green in `general_e2e_composed_pbt`. Necessary
before P2 lands; **not** the feature go/no-go (that is 1b).

**Guard (from review M2):** add a test asserting a display-placed row produces
**zero** Loro/Turso writes — so a future accidental serialization path fails
loud instead of silently persisting a phantom.

**Risk:** low. Well-founded by the green baseline (the `:__virtual:` slot is
already an inert non-ref-known render row).

---

## Phase 0 — Pay the `:__virtual:` debt (rewritten; keep BOTH mechanisms)

> **UPDATE 2026-07-09 — `TrailingSlot` DELETED (superseded).** The
> ViewModel-level `TrailingSlot` layer discussed throughout this Phase 0 was
> later found **dead in production** (all `streaming_collection` callers passed
> `None`) and removed. The static/live-query creation slot is served by
> `interpret_virtual_child` (which already uses the `Value::Float(f64::MAX)`
> sentinel noted below); the streaming path uses `AppendedRowsProvider`. The
> "keep BOTH mechanisms" guidance below refers to `TrailingSlot` vs the provider
> — read it as historical: today the two live mechanisms are
> `interpret_virtual_child` (static) and `AppendedRowsProvider` (streaming).

**Correction from review (B1):** the earlier draft was wrong. `TrailingSlot` is
**not** the clean mechanism to standardize on — it is the broken one:
- its `Value::Float(f64::MAX)` sort sentinel (`prelude.rs:75`) sorts **before**
  FractionalIndex hex keys; `VirtualChildRowProvider` uses
  `Value::String("\u{10FFFF}")` (`reactive_view.rs:146-153`) *specifically to fix
  that*;
- its static snapshot never re-resolves to an editable `EditorView` on focus —
  `tree.rs:77-88` documents that the reactive provider exists to fix exactly this
  and **deliberately discards** `TrailingSlot` on the data_source path;
- its doc comment claiming a `create_entity` submit (`reactive_view.rs:96`) is a
  **phantom** — no such call exists.

Also: the `:__virtual:` id-sniffing is **shared** by both mechanisms via
`parse_virtual_id` (`view_event_handler.rs:190-203, 252-258`) — so deleting the
provider would **not** remove the hack Phase 0 targets.

**Goal (corrected):** kill the stringly-typed origin detection **without**
deleting either injection mechanism.

**Changes:**
- Introduce typed `RowOrigin { Canonical | CreationPlaceholder | DisplayPlaced { canonical_id } }`
  metadata on the rendered node.
- Route the **shared** materialization detection (`parse_virtual_id` and
  `viewmodel_tree_virtual_slots.rs:60`'s `contains(":__virtual:")`) off the typed
  origin instead of the infix.
- **Keep both** `VirtualChildRowProvider` (reactive/data_source) and
  `TrailingSlot` (static/live-query) — they serve different branches for the
  documented reasons above.
- Fix the phantom `create_entity` doc comment.

**Gate:** `general_e2e_composed_pbt` green; creation slot still sorts last
(`inv-viewmodel-tree-virtual-slots`) and is still editable on focus. Pure
retype, no behavior change — verified by the existing invariants, not asserted.

**Risk:** medium — the "no behavior change" claim is only true if both
mechanisms survive and only id-detection is retyped.

---

## → HANDOFF: Focus re-keying is its own ADR + plan (out of scope here)

Re-keying `focused_block` from `EntityUri` to `(EntityUri, occurrence)` touches
~21 prod files / 20+ call sites across all four frontends (gpui / dioxus-web /
tui / worker), the `ReactiveServices` trait and every impl, the caret-seed
carrier, and the **MCP focus surface** (`describe_navigation` focus path
`tools.rs:1789`; `send_navigation` `"action":"focus"`; the watch envelope's
`focused_block` string `holon-worker/src/lib.rs:420`). ADR 0010 already reserves
this: *"multi-region editor focus ... graduates to `MutableBTreeMap<Region,
Option<EntityUri>>` — a separate ADR."*

**This plan stops at Phase 0.** The production focus re-keying — proven feasible
by Phase 1b — is a separate ADR with its own phased rollout (occurrence model →
frontend-by-frontend parity → MCP focus surface → invariants). This handoff *is*
"Phase 2" — deliberately absent from this doc's numbering, which is why Phase 3
follows Phase 0 directly. Phases 3–4 below resume **after** that ADR lands.

**Schedule reality (state it plainly):** the motivating use case — advice — now
ships only after Phase 1b + 1a + 0 + **an entire separate focus-rekeying ADR and
its multi-frontend rollout** + edges + wiring. That is the honest cost of doing
it as a reusable primitive rather than a bespoke hack; no one should be surprised
that advice is gated behind another whole ADR.

---

## Phase 3 — Reference edges + curation state (after focus ADR)

**Goal:** the data P2's query reads for relevance and dismissal.

**Changes (M3-corrected):**
- Backlinks over **content links** are largely free — `block_link.target_id` is
  already reverse-indexed (`block_links.sql`; "no matview needed"
  `schema_modules.rs:466`). Only the `requires` dependency edge
  (`block_requires` junction) needs its reverse index **verified / added**.
- Curation state on the edge: **start with `suppressed: bool`** (ADR 0015 lead
  option). Escalate to the `TaskState` pattern (label + boundary-parsed role)
  only if a concrete need for ≥3 states appears; do **not** extract
  `CuratedState<Role>`.

**Gate:** query returns edges filtered by `!suppressed`; flipping the bit removes
the placement; round-trips through the consolidator and org render.

**Risk:** low — the bool keeps it small.

---

## Phase 4 — Advice as configuration (symbolic ranking)

**Goal:** the feature, as pure composition — zero advice-specific types.

**Changes:**
- A source block (P1) anchored to a task whose query selects lesson blocks via
  the edges (Phase 3), ordered by a **symbolic** score (recency + reference
  weight + linked-task-state read as data), rendered by display-placement.
- Dismissal = flip the edge's `suppressed` bit — a real, synced, org-visible
  write.

**Gate:** end-to-end via the `holon` MCP on a running instance — a lesson edge
surfaces under its task, dismissal removes it and persists; keystone green.

**Risk:** low once 1b/0/3 hold; this is wiring.

---

## Cross-cutting contract rules (from review M2 — fold into whichever phase touches them)

- **Drag-and-drop / expand-collapse inertness.** A display-placed row must be
  **drag-inert**, and expanding a transcluded subtree must **not** mutate
  canonical structure. Contract rule + test.
- **MCP visibility.** `describe_ui` renders the tree, so it *will* show display
  rows — it must mark them with `RowOrigin`. `execute_query` stays **verbatim**
  and never emits display rows. Explicit rule + test for both.
- **Multi-frontend parity.** Any focus/occurrence semantics must land in
  gpui/dioxus/tui/worker together (owned by the focus ADR).
- **No-serialization guard.** The zero-Loro/Turso-writes test (Phase 1a) is the
  standing tripwire for the whole inertness bet.

## Sequencing summary

**Phase 1b (editable focus-collision spike — GO/NO-GO)** → Phase 1a (inert-render
bit-identity + no-write guard) → Phase 0 (typed `RowOrigin`, keep both
mechanisms) → **HANDOFF to focus-rekeying ADR** → Phase 3 (edges + bool) →
Phase 4 (wire advice). Nothing past 1a starts until Phase 1b proves
occurrence-keyed focus is feasible.
