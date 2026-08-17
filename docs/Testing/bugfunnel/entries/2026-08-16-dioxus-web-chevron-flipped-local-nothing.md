---
id: 2026-08-16-dioxus-web-chevron-flipped-local-nothing
date: 2026-08-16
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The dioxus-web chevron flipped a local `use_signal` and nothing else:
  collapse never reached the engine (unsynced, un-undoable, lost on the next
  snapshot), and expanding a lazy `expand_toggle` opened onto NOTHING because
  a collapsed node's snapshot carries the header alone (`content_deferred:
  true`, `reactive_view_model.rs:1126-1147`).
source_line: 1202
---

## Bug

(D21.b lane-expand, found by the frontend duplication audit — code reading,
no test verdict) **The dioxus-web chevron flipped a local `use_signal` and
nothing else: collapse never reached the engine (unsynced, un-undoable, lost
on the next snapshot), and expanding a lazy `expand_toggle` opened onto
NOTHING because a collapsed node's snapshot carries the header alone
(`content_deferred: true`, `reactive_view_model.rs:1126-1147`).** The gate
lives in the worker and is seeded from the view-local expansion store
(`shadow_builders/expand_toggle.rs:61`), not from the `collapsed` column, so
neither a page-local signal nor a bare `set_field(collapsed)` could open it
(a row already at `collapsed = 0` yields no CDC edge for the builder's
edge-triggered follow at `:121-142`). Three copies of the same three writes
existed: GPUI `expand_toggle.rs:70-95`, `user_driver.rs:907-924`, and the
one-leg web copy.

## Root cause

D21.b lane-expand, found by the frontend duplication audit
(`lane-logs/frontend-dup-audit.md` finding 3), i.e. by code reading rather
than by any test: **the dioxus-web chevron was a lie — it flipped a local
`use_signal` and nothing else, so a collapse never reached the engine (not
synced, not undoable, lost on the next snapshot) and an EXPAND of a lazy
`expand_toggle` opened onto nothing at all.** The second half is the sharper
defect and the audit understated it: it claimed "every collapsed subtree is
fully present in `children_vm`", but `ReactiveViewModel::snapshot`
(`crates/holon-frontend/src/reactive_view_model.rs:1126-1147`) only appends
the materialised content when the gate is open and otherwise emits the
header alone with `content_deferred: true`. The gate lives in the WORKER's
view model and is seeded from the view-local expansion store
(`crates/holon-frontend/src/shadow_builders/expand_toggle.rs:61`), never
from the row's `collapsed` column — so a page-local signal could not open
it, and dispatching `set_field(collapsed)` alone could not either (a row
already at `collapsed = 0` produces no CDC edge for the builder's
edge-triggered follow subscription at `:121-142`). The same three writes
existed in triplicate —
`frontends/gpui/src/render/builders/expand_toggle.rs:70-95`,
`crates/holon-frontend/src/user_driver.rs:907-924`, and the web copy that
performed only one of them. FIXED in this lane: the decision is extracted to
`holon_frontend::expand_toggle::expand_toggle_effects` (view-store pair +
optional `set_field(collapsed)` intent), GPUI calls it, and the web performs
both legs — the store leg through a new `engineSetBlockExpanded` worker RPC
(whose write bumps `viewport_generation`, which `watch_snapshot_stream`
folds into its combined signal, so the builder re-seeds, the lazy slot
materialises and the content arrives in the next snapshot), the document leg
through the existing `engineDispatchIntents`. The web chevron now renders
from the snapshot plus a click-time prediction keyed on the snapshot
SEQUENCE (`render::SNAPSHOT_SEQ`, bumped once per delivered envelope), never
on a value: it survives at most until the next delivery — whatever that
delivery says — or until the store RPC fails, which drops it and shows a
visible error marker. A value-keyed guard was written first and was wrong
for a reachable case: when a later snapshot returns to the click-time value
(an external fold from another device, or an undo of the user's own
`set_field`) the stale prediction re-applies and paints the node open
against an authoritative collapsed — the exact scenario GPUI pins in
`an_external_fold_closes_the_nested_page_across_a_rebuild`. The worker
therefore owns the state as `docs/Architecture/UI.md:25` requires, and the
glyph still moves on the click rather than after a full worker round trip.
GAP DELIBERATELY LEFT OPEN and not faked: there is NO Rust test harness that
renders a dioxus-web builder (`frontends/dioxus-web` has zero `mod tests`;
its only web-arm gate is the 37-line `tests/worker-smoke.spec.mjs` boot
check), and the headless keystone drives `user_driver::set_block_expanded`,
which already performed all three writes — so no existing keystone invariant
could have gone red for this, and none was made to. The red-first evidence
covers the EXTRACTION only (`lane-logs/expand-fx-red.log`: 3 failures on the
missing store + intent legs; green in `lane-logs/expand-fx-green.log`); the
web adoption itself is unpinned until a web-arm harness exists. Lazy
materialisation on the web is out of scope by ruling — the fix routes the
gate to the worker's existing `lazy_slot` rather than giving the snapshot
one.)

## Missing piece

the failing code path is platform-only: no Rust harness renders a dioxus-web
builder (zero `mod tests` in `frontends/dioxus-web`; the sole web gate is a
37-line boot smoke spec), while the headless keystone drives
`user_driver::set_block_expanded`, which already performed all three writes
— so no keystone invariant could go red for the web's missing legs

## Remedy

FIXED 2026-08-16 in this lane:
`holon_frontend::expand_toggle::expand_toggle_effects` decides both legs
once; GPUI adopts it unchanged in behaviour; the web performs the store leg
via the new `engineSetBlockExpanded` worker RPC (bumps `viewport_generation`
-> `watch_snapshot_stream` re-fires -> gate re-seeds -> lazy slot
materialises) and the document leg via `engineDispatchIntents`, and renders
the chevron from the snapshot plus a click-time prediction that is keyed on
the snapshot SEQUENCE, not on a value: it holds at most until the next
snapshot delivery (whatever that delivery says) or until the store RPC
fails, in which case it is dropped and the failure is disclosed with a
visible marker carrying the error. Keying on value equality was tried first
and is WRONG — a snapshot later returning to the click-time value (external
fold from another device, or an undo) would resurrect a long-past prediction
and paint the node open against an authoritative collapsed, the scenario
GPUI pins in `an_external_fold_closes_the_nested_page_across_a_rebuild`. So
the worker owns the state and wins every disagreement
(`docs/Architecture/UI.md:25`) while the glyph still moves within the
interaction budget. Red-first covers the EXTRACTION only
(`lane-logs/expand-fx-red.log` -> `lane-logs/expand-fx-green.log`); the web
adoption stays UNPINNED until a web-arm harness exists — stated rather than
faked. Lazy materialisation on the web remains out of scope by ruling.
