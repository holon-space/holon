---
id: 2026-08-11-task-authored-typing-folds-own-keyword
date: 2026-08-11
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A task authored by typing folds its own keyword into its title the moment
  the editor moves to another block.
source_line: 731
---

## Bug

(task #68 dogfood re-entry gate for the rebuilt task-keyword feature, arm d
at main `8570a14a`; found by DOGFOODING the live GPUI app in the shipped
SqlOnly default; no automated test produced it) **A task authored by typing
folds its own keyword into its title the moment the editor moves to another
block.** "TODO x" typed into an empty block leaves the store correct
(`content="x"`, `task_state=TODO`); clicking another block makes `content` =
"TODO x" with `task_state` still TODO, and the org file gains a second
keyword: `* TODO TODO x`. Reproduced 15/15 under the default ring and under
a `#+TODO: NEXT WAITING \ | DONE` page. One-shot, not unbounded: once
`content` carries the keyword the surface and the content column agree
again. The operation history names the seam — every keystroke is a
source-channel write, then one op later `set_field content "" -> "TODO"`,
`origin=user`: the blur commit takes the CONTENT channel carrying the
SURFACE text. Control: a task loaded from disk and merely focused/blurred is
untouched, so what is stale is the seed classification of a block promoted
DURING its own editing session.

## Root cause

task #68 dogfood re-entry gate for the rebuilt task-keyword feature (arm d,
main `8570a14a`), found by DOGFOODING the live GPUI app in the shipped
SqlOnly default (`loro: false` in the boot line): **a task authored by
typing loses its keyword INTO its own title the moment the editor moves to
another block.** Type "TODO x" into an empty block — the store is correct
(`content="x"`, `task_state=TODO`) — then click another block: `content`
becomes "TODO x", `task_state` stays TODO, and the org file gains a second
keyword (`* TODO TODO x`). Reproduced **15/15**, under the default ring and
under a `#+TODO: NEXT WAITING | DONE` page alike, and confirmed one-shot (a
second focus/blur cycle is stable, because surface and content column now
agree). The operation history convicts the seam exactly: every keystroke of
the burst is a source-channel write, and one op later comes `set_field
content "" -> "TODO"` with `origin=user` — the blur commit takes the CONTENT
channel carrying the SURFACE text. Control: a task loaded from disk and
merely focused/blurred is NOT corrupted, so the stale judgement is the seed
classification of a block that was promoted DURING its own editing session.
COVERAGE primary: the blur gesture is not producible — a second `I focus the
editor of block …` in one scenario is refused (`preconditions FAILED for
FocusEditableText`) and `I click block …` moves navigation focus, not the
editor, so no authorable sequence exists in which one editor loses focus to
another. ORACLE secondary: the resulting state is self-consistent across
render, SQL and disk — only the reference model's task facet convicts it.
Missing piece: a blur / focus-transfer transition (editor A -> editor B)
plus a rung that promotes by typing and then blurs.)

## Missing piece

COVERAGE: the blur gesture is not producible — a second `I focus the editor
of block …` in one scenario is refused (`preconditions FAILED for
FocusEditableText`), and `I click block …` moves navigation focus rather
than the editor, so no authorable sequence transfers focus from one editor
to another. ORACLE (secondary): render, SQL and disk all agree on the
corrupted state; only the reference model's task facet convicts it. Missing
piece: an editor-to-editor focus-transfer transition plus a rung that
promotes by typing and then blurs.

## Remedy

CLOSED (task #99). GAP CLOSED FIRST:
`frontends/gpui/tests/task_keyword_blur_windowed.rs` — the windowed rung the
ledger asked for, a REAL editor-to-editor focus transfer
(`SimUserDriver::click_entity` on a sibling row) after promoting by typing,
in BOTH storage arms, with two non-vacuity guards (engine focus actually
moved; the blur added a `block_history` row, 6→7 measured). RED at
`8570a14a` in both arms with the ledger's signature (`content="TODO alpha
two"` beside `task_state=TODO`); GREEN after. CORRECTION to the diagnosis
above: the stale-seed theory is WRONG — the blur funnel never consulted the
seed classification at all. `EditorViewModel::on_blur` /
`pending_commit_intent` build their `set_field` in
`ViewEventHandler::handle_text_sync` from the editable node's own field
(`content`), with no reference to `Surface` or `commits_as_source`; only the
per-keystroke sink was ever routed. FIX: both blur sites now route through
the same `commits_as_source` match
(`EditorViewModel::route_commit_channel`), which preserves the ruled
`Refused` session-pin for free because it is the same match.
