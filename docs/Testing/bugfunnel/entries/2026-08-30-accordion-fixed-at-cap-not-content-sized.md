---
id: 2026-08-30-accordion-fixed-at-cap-not-content-sized
date: 2026-08-30
gap: COVERAGE
secondary: PERCEPTION
status: FIXED
summary: >-
  The "Linked references" accordion reserved a constant 33% of the main panel
  even with zero backlinks, because a `live_query` child pinned the region at
  its cap instead of letting it size to content.
---

## Bug

Martin, dogfooding v0.0.19 on his phone (screenshot, 2026-08-30): the "Linked
references" section at the bottom of the main panel takes a large, CONSTANT
slice of the screen — the same height whether it has results or none. On a
phone-sized panel that is most of the reading area.

## Root cause

`accordion::render_bounded` had two sizing branches. A "greedy" child — a slot
node such as `live_query`, whose `ReactiveShell` claims
`height: relative(1.0)` under `ShellPlacement::Panel` — took
`h(relative(fraction))`: the region was FIXED at `fraction × panel_height`,
independent of its content. Only `text`/collection children took the
shrink-to-content `max_h(relative(fraction))` branch. The seed's accordion
(`assets/default/index.org:23`) holds exactly one `live_query`, so production
always took the fixed branch: 0.33 × panel, empty or not.

That branch existed to keep the greedy shell visible: a percentage height
inside a shrink-to-content region resolves to 0 and takes every row with it.
The sidebar had already met the same hazard and solved it the other way, by
rendering its `live_query` child content-height (`column::render`, BugFunnel
2026-08-02) — the accordion body was the one content-height container still
handing its `live_query` the greedy `Panel` shape.

Measured red (`scratchpad/RED-accordion-sizing.log`, production code reverted
to the parent rev, 1000×900 window, cap 0.33):

    R1 content-sized VIOLATED: an EMPTY accordion is 297 tall, no shorter than
    the 3-row one (297) - the region is fixed at its cap (297)

Only R1 is red on its own property there. R2/R4 fail on their setup/control
assertions and R5 on its setup, because at the parent rev the Panel-placed
cached shell paints no rows the bounds registry can see and there is no
default-collapse to exercise — the preconditions those rungs need do not exist
yet. R3 (many rows saturate the cap) is green before and after.

A second defect surfaced while pinning the survival rung, with only the
reconcile fix missing (`scratchpad/RED-slot-state-loss.log`):

    R5 survival VIOLATED: the reader expanded the accordion, then a resize
    re-interpreted the tree and it snapped back to collapsed

`ReactiveViewModel::push_down_slot` overwrote a slot's content with the fresh
subtree wholesale (`old_slot.content.set(fresh_slot.content.get_cloned())`),
discarding every Mutable below it — the panel's accordion state, and any
editor draft or hover in a slot — on each structural rebuild. Its lazy sibling
`push_down_lazy_slot` already documents the state-loss hazard; the eager slot
never got the same treatment. Latent before this work (the old `collapsed`
default was always `false`, so the value it clobbered in usually matched), and
load-bearing once the default became viewport-derived.

Recursing into the slot then exposed two more defects in the reconcile itself,
both pinned by `crates/holon-frontend/tests/slot_reconcile.rs`
(`scratchpad/RED-slot-reconcile.log`, 3 of 3 red):

- **Liveness.** Building a merged node hands it `subscriptions: Vec::new()`,
  so the mounted node's `DropTask` aborted and the fresh node's was discarded
  — at a slot root, on EVERY rebuild, because unlike `push_down_children` the
  slot had no keep-the-original-Arc fast path. A `live_query` whose item
  template is a bare `text(col(...))` puts a subscription-bearing node exactly
  there (`render_interpreter.rs` `shared_live_query_build`), and four other
  leaves can sit there too (`editable_text`, `rendered_text`, `expand_toggle`,
  `state_toggle`). Effect: the node silently stopped updating from its row and
  kept painting pre-rebuild text — `S1 VIOLATED: ... left: Some("before-rebuild")`.
- **Identity.** `push_down_children` keys by position plus widget name, so
  reordering two same-widget siblings moved one's state onto the other:
  `S2 VIOLATED: after the swap the section titled B is expanded`. Pre-existing
  for direct children; recursing into slots made it reachable there, where
  wholesale replacement could previously only lose state. Wrong state is worse
  than lost state, which is worse than kept state.

## Missing piece

`accordion_bounded_pbt` builds its accordion children as plain `text` VMs, so
every rung — including the I2 shrink-to-content invariant — exercised only the
`max_h` branch. No test in the suite put a slot child (production's shape)
under an accordion and asserted its height against its row count, so the branch
production actually takes was unjudged. Collapsed height, the phone-width
default, and slot-state survival across a rebuild were likewise unpinned.

## Remedy

- `column::push_content_child` renders any slot-bearing child under a
  `Nested`-placement context (`GpuiRenderContext::nested`), so the shell sizes
  to its content. The rule covers the slot-node CLASS rather than a
  `live_query` name match: a future shell-bearing widget cannot silently
  reintroduce the 0-px collapse. `column::render`'s private copy is gone.
- `accordion::render_bounded` always uses `max_h(relative(fraction))`: header
  only when empty or collapsed, content height below the cap, capped and
  internally scrolling above it.
- New behavior: an accordion built with less than
  `ACCORDION_MIN_EXPANDED_WIDTH_PX` (600) of available WIDTH starts collapsed
  (`shadow_builders/accordion.rs`). Width is the axis the rest of the app
  splits on (`if_space`, the drawer's Overlay mode); height does not separate
  phone from desktop, since a portrait phone is ~850 logical px tall.
- `push_down_slot` runs its content through `push_down_children` as a
  one-element list, so a slot root gets the child rules exactly — including the
  keep-the-original-Arc fast path that lets its subscription outlive the
  rebuild.
- `push_down_children` pairs a position only when the two nodes are the same
  logical node (`same_logical_node`: the first of `id` / `block_id` / `title`
  present on both props bags, else the data row's `id`). A node whose chosen key
  DISTINGUISHES it from its siblings is matched by identity, so a reorder no
  longer hands its state to a neighbour. Where no key is present on both sides,
  or the chosen key holds the same value for both siblings, the pairing falls
  back to position and behaves as before.
- Pinned by the five rungs of
  `frontends/gpui/tests/accordion_sizes_to_content_windowed.rs` and the three of
  `crates/holon-frontend/tests/slot_reconcile.rs`.

### Residuals in the reconcile

- Siblings whose identity key holds the SAME value still cross-attach on a
  reorder. Two untitled accordions are the concrete case: the builder inserts
  `title` unconditionally (`ba.args.get_string("title") ... unwrap_or_default()`),
  so both props bags carry `title: ""`, the keys compare equal, and the pairing
  falls back to position. Identity matching narrows the cross-attach to
  same-key siblings; it does not eliminate it. A per-instance key minted at
  build time would, and is the fix if this shows up in practice.
- Renaming a title-keyed section discards its expand (and an editor's draft, on
  the same rule): the identity key IS the title, so old and fresh no longer
  match and the fresh node is adopted whole. State loss, not cross-attach —
  the ranked-better outcome, but a visible one for a section the reader
  renamed while it was open.
- A subscription still dies when the merge has to REBUILD a node (its subtree
  changed): `DropTask` owns an `AbortHandle` and is not clonable, so a node
  that survives only as a copy cannot carry it. Unchanged in this work —
  `push_down_children` has always behaved this way — and now bounded to the
  changed-subtree case rather than every slot rebuild.
- `push_down_lazy_slot` carries a materialised cache forward without merging
  fresh into it, so a render-expression change reaches a materialised lazy
  subtree only on re-materialisation. Documented on the function; not changed
  here.

## Suite effect

Full `holon-gpui` A/B. Parent rev
(`scratchpad/AB-parentrev-gpui-full.log`):

    Summary [ 180.653s] 333 tests run: 319 passed (26 slow), 14 failed, 6 skipped

With these changes (`scratchpad/AB-round3-gpui-full.log`):

    Summary [ 163.735s] 338 tests run: 324 passed (23 slow), 14 failed, 6 skipped

Same count, but the failing set is not name-identical:
`nested_page_real_engine a_real_engine_nested_page_paints_its_children_when_opened`
appears and `gpui_gherkin_replay` drops out. The added name passes on its own
(`scratchpad/AB-round3-nested-page-isolation.log`, `1 test run: 1 passed`), and
it was already in the contention-flaky population before the reconcile changes
existed, so this reads as a flake swap rather than a regression — stated rather
than smoothed over, because it involves lazy slots and expand, the machinery
these changes touch.

`windowed_composed_sut_replays_a_fixture_via_replay_steps_green` is genuinely
repaired by the slot reconcile: it passes in isolation here and fails in
isolation at the parent rev. `gpui_gherkin_replay` and
`windowed_composed_sut_drives_a_click_gesture_sequence_green` are NOT claimed —
both still fail in isolation with these changes in place, on the same
`driver_input.rs` "entity block:c1 not in bounds" signature as at the parent
rev, so they pass only in some full-suite orderings.

## Known trade — Linked references is no longer virtualized

Routing the accordion's `live_query` to `Nested` also routes it to
`column::eager_collection_div` (`builders/mod.rs`), which builds EVERY row each
frame. Before, the Panel-placed shell reached the virtualized `gpui::list` and
built only the viewport's rows. So a page with hundreds of backlinks now
constructs that many `selectable(row(icon, spacer, text))` subtrees per frame
instead of ~10 — a render-cost exposure against the p95
interaction→projection-visible < 200 ms SLO. Scrolling stays correct: the body
keeps its own `min_h_0 + overflow_y_scroll` viewport
(`mcp_scroll_wheel_accordion` green).

Position: accepted for now, not measured. Typical backlink counts are small,
and the sidebar's `live_query` has made the same trade since 2026-08-02. The
alternative — a Nested placement that still virtualizes — needs the eager
firewall in `builders/mod.rs` to hand `gpui::list` a definite height, which is
a change to the shell's sizing contract rather than to this widget. Worth
doing if a real vault shows the cost; no rung measures it today.
