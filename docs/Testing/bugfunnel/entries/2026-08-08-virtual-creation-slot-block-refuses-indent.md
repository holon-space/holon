---
id: 2026-08-08-virtual-creation-slot-block-refuses-indent
date: 2026-08-08
gap: PERCEPTION
secondary: ENVIRONMENT
status: OPEN
summary: >-
  A virtual (creation-slot) block refuses indent and refuses Enter-on-empty,
  and BOTH refusals are completely silent.
source_line: 1191
---

## Bug

(Martin dogfooding; reproduced in a throwaway vault at main e3cc10fe) **A
virtual (creation-slot) block refuses indent and refuses Enter-on-empty, and
BOTH refusals are completely silent.** Enter on an empty slot:
`{"keystrokes_sent":1,"keystrokes_handled":1,"dropped":0}` with the page
child count 4 before and 4 after — no way to make an empty block. Tab with
uncommitted text: `{"status":"executed","matched":[{"action":"indent",…}]}`
while the slot stays at x=376.0, level with its unindented siblings (an
indented row measures 396.0). CONTROL: with text present, Enter commits
normally. The behaviour is DESIGNED —
`editor_view.rs:1391-1396`/`:1409-1414` swallow tab/shift+tab, `:1369-1372`
backspace-at-0, `:1267-1301` routes slot Enter only to
`commit_creation_slot`, and `view_event_handler.rs:233-235` bails on empty
text, all keeping the hard guard `editor_view_model.rs:806-810` unreachable.
The DEFECT is the silence, the same class as the same-day invisible ADR-0028
outdent refusal; it also re-confirms the `send_key_chord` row (`"executed"`
reported for an indent that did not run). Two incidental observations:
uncommitted slot text SURVIVES a full page navigation (real unpersisted
state, no unsaved indication), and `describe_ui` reports the focused slot as
`editable_text ""` while the screen shows its content, so no driver can read
uncommitted text. Whether virtual blocks SHOULD support indent/Enter-empty
is a DESIGN question this row does not settle — options written up in
`lane-report-triage5.md` for a ruling.

## Root cause

Martin dogfooding, reproduced in a throwaway vault at main e3cc10fe — **a
virtual (creation-slot) block refuses indent and refuses Enter-on-empty, and
BOTH refusals are completely silent**. Enter in an empty slot: keystroke
reply `{"keystrokes_sent":1,"keystrokes_handled":1,"dropped":0}`, page child
count 4 before and 4 after — there is no way to make an empty block from the
slot. Tab on a slot holding uncommitted text: reply
`{"status":"executed","matched":[{"action":"indent",…}]}` while the slot's
`editable_text` stays at x=376.0, identical to its unindented siblings (an
indented row measures 396.0 in the same layout, verified against the real
nested child). CONTROL: with text present, Enter commits normally and the
block appears in SQL — so the refusals are specific to (empty Enter) and
(structural ops), not to the slot. The behaviour is DESIGNED, not broken:
`frontends/gpui/src/views/editor_view.rs:1391-1396` and `:1409-1414` swallow
tab/shift+tab with "nothing to indent", `:1369-1372` swallows
backspace-at-0, `:1267-1301` routes slot Enter ONLY to
`commit_creation_slot`, and
`crates/holon-frontend/src/view_event_handler.rs:233-235` bails on empty
text — all of it keeping the hard guard
`crates/holon-frontend/src/editor_view_model.rs:806-810`
(`assert!(!is_creation_placeholder(), "structural … dispatched against
creation-slot id")`) unreachable. The DEFECT is the silence: a common
keystroke does nothing with no toast, no shake and no disabled cue — the
same class as the same-day invisible ADR-0028 outdent refusal. TWO
incidental observations worth their own attention, both recorded in the
evidence: (i) uncommitted slot text SURVIVES a full page navigation (typing,
navigating away and back, then typing again interleaved at the caret to
produce `ViDraft textrtual draft`) — real unpersisted user state carried
across navigation with no unsaved indication; (ii) `describe_ui` reports the
focused slot as `editable_text ""` while the screen visibly shows its
content, so no driver can read what the user typed but has not committed.
Also re-confirms the same-day `send_key_chord` row: `"executed"` was
reported for an `indent` that provably did not run. PERCEPTION — a refused
affordance with no user feedback admits no headless assertion. Secondary
ENVIRONMENT for the driver-reply lie. Whether virtual blocks SHOULD support
indent/Enter-empty is a DESIGN question and is NOT settled by this row;
options are written up for a ruling in `lane-report-triage5.md`. Evidence:
`docs/Testing/fixture-logs-2026-08-08/triage5-virtual-block-indent-and-enter-refused.txt`.
**DESIGN QUESTION RULED, DEFECT FIXED 2026-08-09, task #36.** Ruling (C)
2026-08-08 + sub-ruling (B) 2026-08-09: there is no virtual block to refuse
anything — the affordance mounts no editor, and focus on it births a real
empty block, so by the time a key can be pressed the target is an ordinary
block. Tab dispatches `indent` and Enter dispatches `split_block` at offset
0 (a fresh empty sibling — the same gesture as everywhere else, deliberately
NOT a special case). All five GPUI structural swallows, the slot-Enter
commit arm, the dioxus refusal, `commit_creation_slot`, the
`apply_local_edit` placeholder branch and the `structural_block_action`
assert are DELETED, so no refusal path survives to be silent. Covering tests
`editor_view_model::tests::{an_empty_born_block_indents_immediately,
enter_in_an_empty_born_block_splits_like_any_other_block}`. HONESTLY SCOPED:
this row's PERCEPTION gap is closed only by construction (no refusal exists
to be unreported); the two OBSERVABILITY defects it also names are NOT fixed
— `describe_ui` still reports a focused editor's uncommitted text as
`editable_text ""`, and the `send_key_chord` reply still says `"executed"`
for an op that did not run (that one is task #28's residual).
Unpersisted-draft-across-navigation is likewise untouched.)

## Missing piece

a refused affordance with no user feedback admits no headless assertion; and
the driver reply reports what MATCHED, not what DISPATCHED

## Remedy

OPEN — P3 for the silence, design ruling pending; triage only, no fix in
this lane; evidence
`docs/Testing/fixture-logs-2026-08-08/triage5-virtual-block-indent-and-enter-refused.txt`
