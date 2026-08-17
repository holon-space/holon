---
id: 2026-08-08-trailing-open-nested-page-chevron-never
date: 2026-08-08
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  The trailing "open this nested page" chevron never opens it — the gate that
  decides open-vs-closed is seeded from a store that ONLY the test driver
  writes.
source_line: 1186
---

## Bug

(Martin dogfooding his live instance; reproduced in a throwaway vault at
main e3cc10fe) **The trailing "open this nested page" chevron never opens it
— the gate that decides open-vs-closed is seeded from a store that ONLY the
test driver writes.** `shadow_builders/expand_toggle.rs:61` seeds the gate
from `services.block_expanded_view(&target_id).unwrap_or(default_expanded)`;
the production handler
`frontends/gpui/src/render/builders/expand_toggle.rs:69-88` writes only
`set_field("collapsed")` and never calls `set_block_expanded_view`, whose
sole writer in the tree is the TEST driver at
`crates/holon-frontend/src/user_driver.rs:878`. The store stays `None` in
prod, every rebuild re-seeds `default_expanded=false`, and because the gate
always rebuilds collapsed each click writes `collapsed=false` idempotently —
the row can never reach expanded. Three-part probe: a NINE-click x-sweep
across the whole chevron rect never moved the glyph off the collapsed arrow;
the LEADING `tree_item` chevron control worked at measured coordinates
(child `not_painted`, `collapsed=1`); and the click IS received and DOES
write (forced `collapsed=1`, two clicks logged two `op=set_field` and drove
SQL 1 to 0 while the UI stayed collapsed), localizing the loss to the read
side. ENVIRONMENT — the interaction is generatable (`expand_toggle_id_for`
is registered for the layout PBT's `ToggleCollapse`), but
`DirectUserDriver::set_block_expanded` reaches the gate through the one path
that makes it work, so harness and production diverge at exactly the
affordance under test. Secondary ORACLE: nothing asserts the store a widget
READS is the store its handler WRITES.

## Root cause

Martin dogfooding his live instance, reproduced in a throwaway vault at main
e3cc10fe — **the trailing "open this nested page" chevron never opens it:
the gate that decides open-vs-closed is seeded from a store that ONLY the
test driver writes**.
`crates/holon-frontend/src/shadow_builders/expand_toggle.rs:61` seeds the
gate `services.block_expanded_view(&target_id).unwrap_or(default_expanded)`
— deliberately, because profile-driven embedded pages "carry no `collapsed`
document field" (ratified 2026-07-16, Option B). The production GPUI handler
`frontends/gpui/src/render/builders/expand_toggle.rs:69-88` sets its
per-render Mutable and dispatches `set_field("collapsed")`, and NEVER calls
`set_block_expanded_view`; the sole writer of that store in the whole tree
is `crates/holon-frontend/src/user_driver.rs:878`, the TEST driver. So the
store stays `None` in prod, every structural rebuild re-seeds from
`default_expanded=false`, and the click is discarded. The write cannot
rescue it either: because the gate always rebuilds collapsed, every click
computes `new_val = !false = true` and writes `collapsed = !true = false` —
IDEMPOTENT, so the row can never be driven to expanded at all. PROOF IS A
THREE-PART PROBE, not an inference: (a) an x-sweep of NINE clicks across the
entire chevron rect (x=1160..1198, y=454) left the glyph at ▶ and the lazy
content UNEVALUATED every time, killing "you missed the target"; (b) the
CONTROL — the LEADING `tree_item` chevron on an ordinary parent row, clicked
at measured coordinates in the same session — worked (child `ABSENT:
not_painted`, SQL `collapsed=1`), so clicks and chevrons both function; (c)
the click IS received and DOES write — after forcing `collapsed=1`, two
chevron clicks logged two `entity=block, op=set_field` and moved SQL
`collapsed` 1→0 while the UI stayed collapsed, which localizes the loss to
the READ side, after the write. ENVIRONMENT, not COVERAGE: the interaction
is perfectly generatable — `expand_toggle_id_for` is registered in the
bounds registry precisely so the layout PBT's `ToggleCollapse` can click it
— but `DirectUserDriver::set_block_expanded` reaches the gate through the
one path that makes it work, so the harness and production diverge at
exactly the affordance under test. Same class as the same-day
`send_key_chord` row. Secondary ORACLE: no invariant asserts that the store
a widget READS is the store its production handler WRITES. Evidence:
`docs/Testing/fixture-logs-2026-08-08/triage5-nested-page-chevron-gate-write-only.txt`)

## Missing piece

production chevron writes `set_field(collapsed)` while the gate reads
`block_expanded_view`, a store only `user_driver.rs:878` writes

## Remedy

FIXED 2026-08-08 — UNIFIED on the store the gate reads: the view-local
expansion store is the `expand_toggle` gate's single authority, and every
writer now writes it. `BuilderServices` gains `set_block_expanded_view` (the
write half of the pre-existing `block_expanded_view` reader, engine impl
delegating to `UiState`); the production GPUI chevron handler
(`frontends/gpui/src/render/builders/expand_toggle.rs`) records the click
there beside its existing `set_field(collapsed)` dispatch; the shadow
builder's live-follow subscription records external `collapsed` changes
there too, so a rebuild cannot undo them either; and
`DirectUserDriver::set_block_expanded` writes it in BOTH branches, so the
harness no longer writes anything production does not. Store, NOT
`collapsed`, is the gate's authority — deliberately: a block's default
`collapsed = 0` means "not explicitly folded", so seeding the gate from
`collapsed` would auto-expand every nested page on first paint (the
2026-07-16 lazy-section rationale, and the 2026-07-16
nested-pages-not-collapsed-by-default row). RED-FIRST windowed PBT
`frontends/gpui/tests/nested_page_chevron_gate.rs`: a real window renders
the `embedded_page` shape, the test clicks the trailing chevron at measured
bounds through the production `on_mouse_down`, then asserts a STRUCTURAL
REBUILD (what recursive resolve does every resolve) comes back open — gate,
materialised subtree, and painted glyph. Red before / green after, and
MUTATION-PROVEN: neutering only the handler's store write reds exactly the
rebuild test while the control `chevron_click_opens_the_live_row` stays
green, so the assertion convicts the store, not the hit-test. OBSERVABILITY
added as part of the ENVIRONMENT fix: the trailing chevron registered bounds
but NO `displayed_text`, so open-vs-closed was unreadable from a painted
frame — it now records its glyph and its vm node, like `tree_item`'s
chevron. ROUND 2 — a fresh-context verifier REFUTED the first version of the
live-follow half and it was fixed red-first: propagating BOTH `collapsed`
edges into the view store meant unfolding the OUTLINE row (the leading
`tree_item` chevron, which owns `collapsed`) durably opened the TRAILING
nested-page gate and eagerly materialised a subtree nobody clicked — the
very "auto-expand every nested page" class that ruled out option (a). The
subscription is now DIRECTIONAL: only the FOLD edge is durable (`collapsed
0->1` writes `false`); unfold edges write nothing. Red first
(`an_external_unfold_leaves_the_nested_page_closed`: store `{"nested-page":
true}` vs `{}`), green after, and the fold direction is separately pinned
and separately mutation-proven
(`an_external_fold_closes_the_nested_page_across_a_rebuild`). That round-1
line ALSO had zero test coverage — the windowed tests' services returned
`try_runtime_handle() -> None`, so the subscription never spawned in them;
the directional tests run it for real. ROUND 3 — the dogfood gate REFUTED
this fix as user-visible and it went back: the chevron toggled, the state
recorded, and the opened page painted ZERO children (its own ORACLE row
above, FIXED same day by routing `expand_toggle`'s materialised `live_query`
through `live_query::render_content_height`). The residual below is restated
because the dogfood contradicted its earlier wording in the OPPOSITE
direction: this row previously said the `HeadlessBuilderServices` gap makes
MCP `describe_ui` report every toggle CLOSED; in the live app `describe_ui`
reported the toggle OPEN with the `live_query` node present while the window
painted nothing. The accurate, narrower statement: `HeadlessBuilderServices`
implements neither `block_expanded_view` nor `set_block_expanded_view`, so a
toggle driven purely through that surface has no view-store seed to read —
untested either way, and NOT the behaviour observed live. ORACLE HALF STILL
OPEN: `inv-embedded-page-collapsed-lazy` — the invariant that most directly
concerns this widget — engages in only 6 of the 39 hand-authored rows (1..3
cases each) and is not engaged AT ALL in the keystone-smoke draw (absent
from its engagement summary, 0 draws). It caught none of the three defects
in this arc (write-only gate, unfold regression, opens-to-nothing). No
generated draw renders an embedded page and toggles it; closing that needs a
generator arm and is NOT closed here. Evidence:
`docs/Testing/fixture-logs-2026-08-08/triage5-nested-page-chevron-gate-write-only.txt`
(original probe),
`docs/Testing/fixture-logs-2026-08-08/task16-chevron-gate-red-green.txt`
(red/green/mutation, incl. the ROUND 2 appendix)
