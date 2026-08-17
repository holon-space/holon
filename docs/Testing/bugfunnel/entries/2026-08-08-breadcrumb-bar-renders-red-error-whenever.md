---
id: 2026-08-08-breadcrumb-bar-renders-red-error-whenever
date: 2026-08-08
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  The breadcrumb bar renders a red error whenever the caret sits in the
  creation slot
source_line: 1188
---

## Bug

(Martin dogfooding; reproduced in a throwaway vault at main e3cc10fe) **The
breadcrumb bar renders a red error whenever the caret sits in the creation
slot** — the app's most common interaction: `Breadcrumb unavailable:
breadcrumb: block block:__virtual:… has no path in block_with_path` (doubled
noun included). `frontends/gpui/src/lib.rs:851-869` re-resolves the trail
from `ui_state().focused_block()` with no `is_creation_placeholder` guard,
so the virtual id reaches `crates/holon/src/api/query_engine.rs:134-143`,
whose `SELECT path FROM block_with_path` cannot match because a placeholder
has no SQL row by construction (`row_origin.rs:36-40`), and the Err is
painted red at `breadcrumb.rs:158-172`. The fail-loud machinery works as
written; the state it flags is legal, so the bar cries wolf. CONTROL: a real
nested block on the same page renders the ordinary trail. Second-order cost
hit during this triage: the bar is layout-shifting (+28px to every row),
which invalidated measured click coordinates and produced two false
observations before it was caught. ENVIRONMENT — focusing the slot is
generatable, but `breadcrumb.rs` is GPUI-only so the failing path never runs
in the keystone wiring.

## Root cause

Martin dogfooding, reproduced in a throwaway vault at main e3cc10fe — **the
breadcrumb bar renders a red error whenever the caret sits in the creation
slot**, i.e. during the app's single most common interaction. Verbatim:
`Breadcrumb unavailable: breadcrumb: block
block:__virtual:aaaa1111-0000-4000-8000-000000000001 has no path in
block_with_path` (note the doubled noun: the query layer prefixes
`breadcrumb: ` and the id already carries its `block:` scheme).
`frontends/gpui/src/lib.rs:851-869` re-resolves the trail straight from
`ui_state().focused_block()` with no `RowOrigin::is_creation_placeholder`
guard, so the virtual id reaches
`crates/holon/src/api/query_engine.rs:134-143`, whose `SELECT path FROM
block_with_path WHERE id=…` cannot match — a creation placeholder has no SQL
row BY CONSTRUCTION (`crates/holon-frontend/src/row_origin.rs:36-40`) — and
the Err is painted in red at `frontends/gpui/src/breadcrumb.rs:158-172`. The
fail-loud machinery is working exactly as written; what is wrong is that
this is a normal state, not an error, so the bar cries wolf and trains the
user to ignore it. CONTROL: focusing a real nested block on the same page
renders the ordinary trail. Non-obvious second-order cost, hit during this
very triage: the bar is LAYOUT-SHIFTING (+28px to every sidebar and
main-panel row while displayed), which silently invalidated
previously-measured click coordinates and produced two false observations
before it was caught — anything driving this app by coordinates must
re-measure after any action that can seat the caret in a slot. ENVIRONMENT:
focusing the creation slot IS generatable (the driver has creation-slot
support), but `breadcrumb.rs` exists only in the GPUI frontend, so the
failing path does not run in the keystone's wiring at all. Secondary ORACLE:
no invariant asserts "no error text is rendered while the app is in a legal
state". Evidence:
`docs/Testing/fixture-logs-2026-08-08/triage5-breadcrumb-error-on-virtual-block.txt`.
**FIXED 2026-08-09, task #36** under ruling (C) 2026-08-08 + sub-ruling (B)
2026-08-09 ("a creation slot becomes a real born block the moment it can
receive input"): the affordance now renders through
`reactive_view::creation_affordance_template()` with NO `editable_text` on
either render path, and focus reaching it is intercepted into a birth
(`ReactiveEngine::birth_creation_affordance`), so the caret is only ever in
a real block and `block_with_path` always resolves. Fixed STRUCTURALLY, not
by a breadcrumb guard — `breadcrumb.rs` is untouched. Covering test
`holon-frontend::sidebar_creation_slot::the_creation_affordance_mounts_no_editor`
(mutation-proven red, `lane-logs/36-red-affordance.log`). HONESTLY SCOPED:
no windowed rung asserts the rendered breadcrumb text on a fresh slot, so
the ENVIRONMENT gap this row names — `breadcrumb.rs` running in no headless
wiring — is NOT closed, and neither is the secondary ORACLE gap ("no
invariant asserts that no error text is rendered in a legal state"). Both
remain open; only the defect is gone.)

## Missing piece

no `RowOrigin::is_creation_placeholder` guard before `breadcrumb_trail`;
breadcrumb widget absent from headless wiring, and no invariant forbids
error text in a legal state

## Remedy

OPEN — P2, triage only, no fix in this lane; evidence
`docs/Testing/fixture-logs-2026-08-08/triage5-breadcrumb-error-on-virtual-block.txt`
