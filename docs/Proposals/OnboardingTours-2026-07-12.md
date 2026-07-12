# Onboarding Tours — interactive walkthroughs as block-substrate data

**Status:** Proposed 2026-07-12. Vertical-slice spike landed alongside (see §9).
**Relates to:** [Model.md](../Architecture/Model.md) (five layers; invariant 3/4 —
intent boundary; Cell vs Mutable state cut), ADR 0015 (canonical vs *display*
placement — the "system speaks into the UI" precedent), ADR 0021/0022 (advice as
composition; `advice_weaver`'s session-level `AdviceSidecar`), ADR 0024 (unified
action execution — deterministic effect IDs, provenance/history relations,
rules-as-blocks — the observation substrate for gated steps),
[Templating-2026-07-12.md](Templating-2026-07-12.md) (the "X-as-block-subtree,
instantiation-as-operation" pattern this doc copies wholesale).

---

## 0. The five questions, answered up front

1. **What is this called?** An **interactive walkthrough with gated (action-required)
   steps**, rendered with a **spotlight / coach-mark overlay** primitive. Industry
   splits the family by *scope*: a **product tour** is broad passive orientation; an
   **interactive walkthrough** is narrow and task-focused — each step can require the
   user to actually *do* the thing before advancing. Its sibling is **contextual /
   reactive onboarding** (hints triggered by user state, not a fixed script). Martin's
   description (highlight + help text + next/prev + "occasionally perform an easy
   task") is exactly an interactive walkthrough with a mix of *manual-advance* and
   *action-gated* steps. "Guided tour", "feature tour", "coach marks", "spotlight
   onboarding" are synonyms or sub-patterns, not distinct architectures.

2. **Are there GPUI / Dioxus components?** **No, for both.** Nothing tour/coach-mark
   /spotlight exists in GPUI core, `gpui-component` (longbridge), or Zed itself —
   Zed's onboarding is a *full-page wizard* opened in the center pane (theme/keymap
   pickers gated by a `FIRST_OPEN` key), not anchored overlays over the live UI. No
   Dioxus tour component exists either; web-target Dioxus *could* bolt on Driver.js /
   Shepherd.js via JS interop (real DOM), but that buys nothing for desktop/GPUI. We
   build the overlay primitive ourselves. The JS libraries (Shepherd.js, Driver.js,
   react-joyride, Intro.js) are the **vocabulary source**, not a dependency — see §2.

3. **How does it integrate with MVVM / ReactiveViewModel?** A **`TourViewModel`**
   projects the *active step* (anchor, copy, index/total, advance condition) as a
   session-level reactive value — the same shape as `advice_weaver`'s `AdviceSidecar`,
   which is the existing precedent for "the system, not a render slot, authoring state
   into the UI". The View layer reads the anchor's rect from `BoundsRegistry` and
   paints a spotlight overlay + tooltip. **No tour logic lives in the view beyond
   geometry.** See §5.

4. **How is it generalized for reuse?** The tour decomposes into four reusable
   primitives (§7): (P1) an **anchored-overlay/decoration layer**, (P2) a **step
   engine** (ordered steps + advance conditions), (P3) an **advance-on-observed-state
   "wait-for" subscription**, and (P4) a **stable anchor-id scheme**. Each independently
   serves feature-discovery hints, "what's new" walkthroughs, contextual help, form/
   wizard flows, and the Watcher's advice display placements. The tour is the *first
   consumer*, not a bespoke feature.

5. **What fundamental capabilities are missing?** Six gaps (§8), ranked by build
   order: **G1** no stable anchor-id scheme for non-block UI regions (+ the
   not-yet-rendered-target race); **G2** no reusable anchored-overlay/decoration layer
   in GPUI (only ad-hoc `deferred` pie-menu/IME) nor a cross-frontend overlay
   abstraction; **G3** no *exposed* "wait until this predicate over engine state
   becomes true" subscription (the rules engine has it internally but private);
   **G4** the ViewModel is render-slot-scoped — there is no first-class *system-authored*
   overlay VM channel (advice hacks one in); **G5** progress-identity (per-replica vs
   synced) is unmodeled; **G6** overlay/anchor state isn't observable to the driver
   ladder, so action-gated steps can't be driven headlessly.

---

## 1. First principles

- **A tour is data on the block substrate.** Like templates, rules, and advice, a
  tour is an ordinary block subtree — no new file format, full org round-trip, editable
  in the app itself. Whatever a step needs (copy, anchor, advance rule) is a block's
  content + typed properties. This is the Holon way (Model.md: "one logical block
  tree"); a bespoke `Tour` store would violate it.
- **Step advancement is an operation at the intent boundary** (Model.md inv. 3/4). "Go
  to next step" and "mark step complete" are ordinary ops with a `set_field` on a
  progress record — so progress *persists* and, if desired, *syncs* through the one
  consolidator, with undo/provenance for free.
- **Gated advance is observation, not polling.** "Advance when the user creates a block
  under X" is a *predicate over engine state / provenance* — exactly what the rules
  engine already watches (ADR 0024 history relation; advice's watch-matview-then-
  recompute). The tour subscribes to a predicate; it does not poll the UI.
- **The view only does geometry.** The overlay is a pure function of (active step's
  anchor rect from `BoundsRegistry`, copy). All *decisions* — which step, when to
  advance — live in the VM. This is the Cell-vs-Mutable discipline (Model.md): the tour's
  authored state has identity and cross-consumer coherence → VM/session-level, not a
  per-render-slot `Mutable`.
- **Fail loud.** An anchor that resolves to no bounds after the target *should* be on
  screen is a visible degraded-mode banner ("tour step N: anchor `sidebar` not found"),
  never a silently skipped step (Model.md error philosophy; mirrors the JS libraries'
  `TARGET_NOT_FOUND`).

---

## 2. Interaction vocabulary (borrowed from the JS libraries, owned by us)

The mature libraries converge on a vocabulary we adopt verbatim as our type names:

| Term | Meaning | Our representation |
|---|---|---|
| **anchor / target** | the highlighted element | `tour_anchor` prop → `AnchorSelector` (§4) |
| **spotlight / mask** | dimmed overlay with a cut-out around the anchor | GPUI overlay element (§5); SVG-mask / 4-div / box-shadow are the web techniques, ours is a painted cut-out |
| **beacon** | small pre-step marker inviting a click | optional `tour_beacon` bool (v2) |
| **step lifecycle** | before-show / show / advance / exit hooks | `TourStep` + `StepPhase` in the VM |
| **gating / advanceOn** | bind a real event/state as the advance trigger | `AdvanceCondition::Observed` (§4) — our equivalent of Shepherd's `advanceOn` and joyride's controlled `stepIndex` |
| **target-not-found / beforeShowPromise** | handle a not-yet-rendered anchor | G1 race (§8); resolved by waiting on `BoundsRegistry` commit, cf. `GeometryProvider::changed` |

Only **Shepherd.js** (`advanceOn` + `beforeShowPromise`) and **react-joyride**
(controlled mode) support true action-gating; that is the feature Martin specifically
wants and the one with the least prior art to copy, so §4/§8-G3 spend the most design
there. Accessibility note from the benchmark: most libraries ship axe violations
(missing ARIA/keyboard) — our overlay must be keyboard-navigable and honor reduced-motion
from day one, since we have no framework defaults to inherit.

---

## 3. Tour representation (block subtree)

A tour is a subtree whose **root** carries a `Tour` tag and tour-level properties; each
**child** is a step. In org (no parser special-casing — ordinary drawers, exactly like
Templating §2):

```org
* Welcome to Holon
:PROPERTIES:
:ID: tour-welcome
:TAGS: Tour
:TOUR_TRIGGER: first-boot
:END:
** This is your sidebar — your pages live here.
:PROPERTIES:
:ID: tour-welcome-1
:TOUR_ANCHOR: panel:sidebar
:TOUR_ADVANCE: next
:END:
** This is the main panel, where you edit.
:PROPERTIES:
:ID: tour-welcome-2
:TOUR_ANCHOR: panel:main
:TOUR_ADVANCE: next
:END:
** Now create your first block here. Press Enter on an empty line.
:PROPERTIES:
:ID: tour-welcome-3
:TOUR_ANCHOR: panel:main
:TOUR_ADVANCE: observed:child-created-under(panel:main.page)
:END:
```

- **Step order** = sibling order (consolidator-minted `sort_key`; Model.md inv. 2). No
  separate index field — reordering steps is an ordinary `move_block`.
- **Copy** = the step block's `content` (marks/links work — it's just a block).
- **Typed props parsed at the boundary** (parse-don't-validate): `TOUR_ANCHOR` →
  `AnchorSelector`, `TOUR_ADVANCE` → `AdvanceCondition`. A malformed selector or
  unknown advance kind is a **parse error that fails the tour load loudly**, never a
  skipped step.

Because it is just blocks, a tour is authored, versioned, translated, and even
*instantiated from a template* (Templating) with zero new machinery. A "what's new in
v0.9" walkthrough is a tour block subtree shipped in the seed vault.

---

## 4. The typed core (parse at the boundary)

```rust
// crates/holon-frontend/src/tour.rs  (frontend-agnostic; no GPUI types)

/// Where a step points. Parsed from `TOUR_ANCHOR`.
enum AnchorSelector {
    Block(EntityUri),      // "block:<id>"  — a specific block occurrence
    Entity(EntityUri),     // "entity:<id>" — any occurrence of an entity (RowIdentity)
    Panel(WellKnownPanel), // "panel:sidebar" | "panel:main" | ... — a UI region (G1)
}

/// When a step advances. Parsed from `TOUR_ADVANCE`.
enum AdvanceCondition {
    Next,                       // manual: user clicks "Next"
    Observed(StatePredicate),   // gated: fires when a predicate over engine state holds
}

/// A predicate the tour subscribes to (P3 / G3). Evaluated against engine
/// state + provenance (ADR 0024), NOT by polling the view.
enum StatePredicate {
    ChildCreatedUnder(AnchorSelector), // "user created a block under X"
    FieldEquals { anchor: AnchorSelector, field: BlockField, value: Value },
    OpObserved { op: OpName, since: ProvenanceCursor }, // any op of a kind since step start
}

struct TourStep { id: EntityUri, copy: String, anchor: AnchorSelector, advance: AdvanceCondition }
struct Tour { id: EntityUri, trigger: TourTrigger, steps: Vec<TourStep> }
```

Parsing lives in one `parse_tour(subtree: &[Block]) -> Result<Tour>` at the block
boundary. The rest of the system never re-validates a raw `TOUR_ANCHOR` string.

---

## 5. MVVM integration

### 5.1 `TourViewModel` — a session-level authored projection

The `ReactiveViewModel` node type (`crates/holon-frontend/src/reactive_view_model.rs`)
is **render-slot-scoped**: nodes are minted by `BuilderServices::interpret()` during a
render walk and own per-slot `Mutable`s. A tour is *not* render-slot state — it is the
**system authoring an overlay across the whole window**, which is precisely the shape of
`advice_weaver`'s `AdviceSidecar` (`Arc<Mutex<HashMap<EntityUri, Vec<Arc<DataRow>>>>>`,
one session-level channel the interpreter reads synchronously). `TourViewModel` is the
same pattern, generalized (§7-P?/§8-G4):

```rust
struct TourViewModel {
    tour:        Tour,                       // parsed once from the tour subtree
    active_step: Mutable<usize>,             // advanced by ops; persisted via progress record
    phase:       Mutable<StepPhase>,         // BeforeShow | Shown | AnchorMissing
    _watch:      DropTask,                   // subscription to the active step's AdvanceCondition
}

// The view reads exactly this:
struct ActiveStepView { index: usize, total: usize, copy: String,
                        anchor: AnchorSelector, advance: AdvanceCondition, phase: StepPhase }
```

- **Active step** is a `Mutable<usize>` derived from the persisted progress record (a
  `set_field` target — §6), so it survives restart and (optionally) syncs.
- The VM installs **one watch** for the *current* step's `AdvanceCondition::Observed`
  predicate (P3). When the predicate fires, the VM dispatches the advance op — it does
  not mutate `active_step` directly; advancement goes through the intent boundary so it
  persists and is undoable (Model.md inv. 3/4). `Next`-condition steps advance on a
  view "Next" click that dispatches the same op.
- **`AnchorMissing` phase**: if the anchor should be resolvable but `BoundsRegistry`
  has no rect after a commit, the VM enters `AnchorMissing` → the view shows a
  degraded-mode banner (fail loud, §1). This is the Holon answer to `TARGET_NOT_FOUND`.

### 5.2 View layer (GPUI first) — geometry only

```
root reactive_shell
  └─ deferred(tour_overlay).with_priority(HIGH)   // above all content, cf. pie_menu.rs
        ├─ scrim with anchor-rect cut-out         // spotlight
        └─ tooltip card near anchor rect          // copy + Prev/Next/step-count
```

- The overlay reads `ActiveStepView`, resolves `anchor` → key → `ElementInfo` via
  `GeometryProvider::element_info(id)` (`frontends/gpui/src/geometry.rs`), and paints a
  `deferred(...).with_priority(HIGH)` layer at the root — the exact technique
  `pie_menu.rs` already uses for its backdrop+overlay. `GeometryProvider::changed`
  (the commit-notify) drives re-resolution when the anchor moves/appears.
- The **only** view logic is: id→rect lookup, cut-out geometry, tooltip placement,
  and forwarding Prev/Next clicks as ops. No step selection, no advance evaluation.
- **Dioxus later**: same `TourViewModel`; a Dioxus view reads the same
  `ActiveStepView` and draws the overlay with DOM/CSS (or, on web only, could delegate
  to Driver.js). The VM is frontend-agnostic by construction.

### 5.3 Anchor-id resolution

`AnchorSelector` → `BoundsRegistry` key:
- `Block(id)` → `"render-entity-{id}"` (existing key convention, `geometry.rs`).
- `Entity(id)` → any recorded occurrence keyed by RowIdentity (needs the entity→
  occurrences map; G1).
- `Panel(p)` → a **well-known panel id** that does not exist yet — G1. Regions
  ("main", "sidebar") are today render-context enums, *not* registered bounds. The fix
  is to `record(...)` the panel container's rect under a stable id (`"panel:sidebar"`)
  during prepaint.

---

## 6. Progress persistence & sync

Active step is stored as `tour_active_step` on a **progress record**. Two options,
disclosed:

- **On the tour root** (simplest; the spike does this): one `set_field` on the tour
  block. Progress then *syncs P2P* — usually **wrong** (device A's tour position
  shouldn't drive device B). Fine for a single-device spike; flagged as G5.
- **Per-replica progress block** (correct target): a small `TourProgress` block keyed
  by `(tour_id, replica/user)`, outside the synced tree or in a per-device lane. This
  needs the replica/user-identity model that doesn't exist yet (G5). Ship option 1,
  migrate to option 2 when identity lands.

Either way advancement is an **op with `OpOrigin::User`** (or a dedicated origin) →
undo/redo and provenance come free, and the keystone's op-observation sees it.

---

## 7. Generalization — the four reusable primitives

The tour is the first consumer of a primitive *set*, each independently useful:

- **P1 — Anchored-overlay / decoration layer.** "Given an anchor id + a decoration
  (spotlight, colored border, badge, tooltip), paint it over the live UI at that
  element's rect, re-resolving as it moves." Reused by: **advice display placements**
  (ADR 0015 — currently inline rows; could be anchored callouts), **feature-discovery
  hints** (a pulsing dot on a new button), **validation callouts** (point at the field
  in error), **selection/debug overlays** (`screenshot_overlay.rs` already hand-rolls
  one). This is G2.
- **P2 — Step engine.** "An ordered list of steps with per-step advance conditions and
  a persisted cursor." Reused by: **template-instantiation wizards** (Templating), a
  **first-run setup flow**, any multi-step form. Frontend-agnostic; lives in
  `holon-frontend`.
- **P3 — Wait-for / observed-state subscription.** "Call me back when this predicate
  over engine state/provenance becomes true." Reused by: **contextual onboarding**
  (reactive hints), **achievements / 'you've done X' nudges**, **rules-engine
  triggers** (which already do this internally — P3 is exposing that as a reusable
  subscription). This is G3, and it is the same machinery as ADR 0024 rules + advice's
  watch-matview. The deepest and most valuable primitive.
- **P4 — Stable anchor-id scheme.** "A durable name for a UI region/element that
  survives re-render." Reused by: everything in P1, plus **command-palette targeting**,
  **deep links to a panel**, **screenshot/e2e assertions**. This is G1, and RowIdentity
  is the block-level half of it.

Naming them explicitly is the point: build P1–P4 as the deliverable, the tour as a
thin composition on top. This mirrors the advice plan's "never a bespoke advice type —
composable primitives" ruling.

---

## 8. Gap analysis (Q5), ranked by build order

| # | Gap | Why it blocks tours | Reuse (P#) | Build order |
|---|---|---|---|---|
| **G4** | **No system-authored VM channel.** `ReactiveViewModel` is render-slot-scoped; only `advice_weaver` hacks in a session-level authored channel. | The tour overlay must exist independent of any render slot, spanning the window. | P2 | **1st** — generalize `AdviceSidecar` into a reusable session-level authored-VM channel (`TourViewModel` + advice both consume it). Small, unblocks everything. |
| **G1** | **No stable anchor-id scheme for non-block UI.** Panels are render-context enums, not registered bounds; entity anchors need the RowIdentity occurrence map; and there is a **not-yet-rendered-target race** (staged/committed) — the `TARGET_NOT_FOUND`/`beforeShowPromise` problem. | Steps 1–2 anchor to `panel:sidebar`/`panel:main`, which cannot be resolved today. | P4 | **2nd** — `record()` panel containers under well-known ids; wait on `GeometryProvider::changed` for late anchors; reuse RowIdentity for entity anchors. |
| **G2** | **No reusable anchored-overlay/decoration layer** in GPUI (only ad-hoc `deferred` pie-menu/IME); no cross-frontend overlay abstraction. | The spotlight+tooltip must render; and Dioxus must later render the same thing. | P1 | **3rd** — extract a `deferred`-based overlay host at the root that takes (anchor-id, decoration). Dioxus impl deferred. |
| **G3** | **No exposed "wait-for predicate" subscription.** The rules engine watches matviews internally but there is no reusable "await this predicate over engine state/provenance" the VM can subscribe to. | Action-gated steps ("create a block under X") need it; polling the UI is wrong (§1). | P3 | **4th** — expose a `watch_predicate(StatePredicate) -> stream` over the same CDC/advice-watch substrate; relate to ADR 0024 history relation + C4. Deepest primitive. |
| **G6** | **Overlay/anchor state not observable to the driver ladder.** The keystone drives ops and reads `describe_ui`/`BoundsRegistry`, but tour overlay + active-step + anchor-resolution aren't surfaced. | Action-gated steps can't be *driven or asserted* headlessly without it — violates the repo rule that UI bugs must be reproducible in the keystone. | P1/P3 | **5th** — extend `describe_ui`/geometry with tour-overlay state (active step, resolved anchor rect, phase). |
| **G5** | **Progress-identity unmodeled.** No per-replica/per-user identity, so progress either wrongly syncs P2P or lives nowhere durable. | Multi-device correctness of "where am I in the tour". | P2 | **6th** — ship on tour-root field (syncs, single-device-correct); migrate to per-replica progress block when identity lands. |

**Critical path:** G4 → G1 → G2 gets a *manual-advance* tour rendering (steps 1–2).
G3 + G6 add *action-gated* steps and headless drivability (step 3). G5 is a
correctness follow-up, not a blocker for a single-device first cut.

---

## 9. Spike — what the vertical slice proves

A minimal, honest slice landed with this doc to de-risk the seams (not to ship a
feature). It lives frontend-agnostic in `crates/holon-frontend/src/tour.rs` with a
directed integration test `crates/holon-integration-tests/tests/tour_spike.rs`.

It proves, against the **real engine and real `BoundsRegistry`**:

1. **Tour-as-data parses** — a 3-step tour seeded as an org subtree parses into a typed
   `Tour` (`parse_tour`); a malformed `TOUR_ADVANCE` fails loudly.
2. **Anchor → rect resolves** — `AnchorSelector::Panel`/`Block` resolves against a
   `GeometryProvider` with a recorded `ElementInfo` (and returns `AnchorMissing`
   otherwise — fail loud, not skip).
3. **Manual advance is an op** — advancing the active step round-trips through
   `execute_operation` (`set_field` on the progress record) and persists.
4. **Action-gated advance is observed** — the `ChildCreatedUnder` predicate evaluates
   `false` before and `true` after a real `create` op under the target panel's page,
   using an engine query (`from children`) — proving G3's evaluation shape without yet
   building the full subscription.

What it deliberately does **not** do: paint real GPUI pixels (G2 overlay host is
described, not built), expose the wait-for subscription as a stream (G3 — the spike
evaluates the predicate directly), or model progress identity (G5). Those are the
ranked follow-ups above.

---

## 10. What to build next (recommendation)

1. **G4 + P2**: promote `AdviceSidecar` → a generic session-level authored-VM channel;
   land `TourViewModel` on it. (Days.)
2. **G1 + P4**: register well-known panel ids in `BoundsRegistry`; wire entity anchors
   through RowIdentity; handle the late-anchor race via `GeometryProvider::changed`.
3. **G2 + P1**: root-level `deferred` overlay host taking (anchor-id, decoration);
   render the spotlight+tooltip for a manual-advance tour end-to-end in GPUI, driven
   over MCP.
4. **G3 + P3**: expose `watch_predicate` over the CDC/advice-watch substrate; convert
   the spike's direct predicate eval into a real subscription; light up action-gated
   step 3. Add **G6** observability in the same pass so the keystone can drive it.
5. **G5**: per-replica progress block once identity exists.

The doc is the deliverable; the spike proves the seams named in §9 are real.
