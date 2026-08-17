---
id: 2026-08-11-structural-chord-flushes-focused-editor-stale
date: 2026-08-11
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A structural chord flushes the focused editor's STALE buffer before
  executing, silently reverting any store write that did not go through that
  editor — and after a `split_block` the tail block survives the revert, so
  the text is DUPLICATED.
source_line: 735
---

## Bug

(task #92 Cucumber-dogfood rehearsal, found by DOGFOODING at main
`644a399d`, shipped SqlOnly default; no automated test produced it) **A
structural chord flushes the focused editor's STALE buffer before executing,
silently reverting any store write that did not go through that editor — and
after a `split_block` the tail block survives the revert, so the text is
DUPLICATED.** Reproduced 3/3 (2 window-inactive, 1 window-fronted, so
`WINDOW-INACTIVE` is not the mechanism). Evidence is the tool's own reply:
`send_key_chord {keys:["tab"]}` returns `dispatched:
[{operation:"set_field", target:<the split block>, outcome:"succeeded"},
{operation:"indent", ...}]` — an unrequested `set_field` ahead of the
indent. Concretely `EXTERNALZZZZ` → `split_block(8)` → SQL `EXTERNAL` +
`ZZZZ` (correct) → Tab → `EXTERNALZZZZ` with child `ZZZZ`, persisted to the
org file, no error, `run_self_checks` 1 pass / 0 fail. Controls that name
the seam: an MCP `set_field` under the same focused editor was NOT reverted
(the re-seed backstop covers plain content writes, misses `split_block`),
and a plain click-away blur did NOT revert either (so the trigger is the
chord's pre-flush, not blur).

## Root cause

task #92 Cucumber-dogfood rehearsal, found by DOGFOODING against a live app
at main `644a399d` in the shipped SqlOnly default (`loro: false` in the boot
line): **a structural chord flushes the focused editor's STALE buffer before
executing, so any store write that did not go through that editor is
reverted — and when the write was a `split_block`, the tail block SURVIVES
the revert, so the text is silently DUPLICATED.** Reproduced 3/3, twice with
the window OS-inactive and once with it fronted (so `WINDOW-INACTIVE`, the
leading suspect in the 2026-08-10 row below, is NOT the mechanism here). The
tool's own reply is the evidence: `send_key_chord {keys:["tab"]}` on the NEW
block returns `dispatched: [{operation:"set_field", target:<the SPLIT
block>, outcome:"succeeded"}, {operation:"indent", target:<new block>,
outcome:"succeeded"}]` — an unrequested `set_field` the user never asked
for, ahead of the indent. Concretely: block content `EXTERNALZZZZ`, editor
focused by a click; `split_block(pos 8)` → SQL correctly `EXTERNAL` + new
`ZZZZ`; then Tab → SQL `EXTERNALZZZZ` with child `ZZZZ`. "ZZZZ" now exists
twice, on disk too (org write-back projected it faithfully), with no error,
no warning, and `run_self_checks` still 1 pass / 0 fail. SINGLE-VARIABLE
CONTROL that names the seam precisely: an MCP `set_field` under the same
focused editor was NOT reverted by the same Tab — so the
`ReseedGesture`/render backstop covers plain content writes and misses
`split_block`; and a plain click-away (blur) did NOT revert either, so the
trigger is the CHORD path's pre-flush, not blur. Primary COVERAGE: no rung
follows a non-editor-origin structural write with a structural CHORD and
then asserts the store; the headless keystone structurally cannot, because
its editor mirror is converged unconditionally by the harness settle and
therefore can never hold a stale buffer. Secondary ORACLE: even at the
windowed rung the resulting state is self-consistent (render, SQL and disk
all agree on the duplicated text), so only a comparison against the
reference model convicts it. Extends, does NOT duplicate, the 2026-08-10
`undo`-then-blur row: same stale-buffer seam, different trigger (structural
chord, no undo involved) and a worse outcome (duplication rather than
revert). FIXED 2026-08-11 (task #94), with the mechanism PROBED rather than
inferred: a FOCUSED editor receives NO data-sync echo at all — one echo at
the seed, none for its own four keystrokes and none for the external split —
so its buffer never learns the row moved. The chord's pre-flush
(`EditorView::dispatch_structural_as_commit_point`) diffed the visible field
against the handler's baseline, which the keystroke sink does NOT advance,
so it re-committed text `apply_local_edit` had already dispatched; against a
row an external origin had moved, that redundant write IS the revert. Fix: a
distinct `EditorViewModel::chord_commit_intent` refuses to flush a buffer
the keystroke sink already persisted (`live_text == buffer`) and still
flushes text the sink never saw (IME, programmatic `set_value`). The
focus-leave funnel (`pending_commit_intent`) is deliberately NOT narrowed —
the dogfood's own blur control shows it does not revert, and its redundant
commit is exactly what task #99's landed vacuity guard asserts. Gap closed
by `frontends/gpui/tests/structural_chord_stale_flush_windowed.rs`:
windowed, real click + real keystrokes + `execute_operation
block.split_block` as the non-editor origin + a real Tab, RED at base in
BOTH storage arms with the exact duplication signature (`content="alpha
twoZZZZ"` beside a surviving `ZZZZ` block) and green after; plus three
`editor_view_model` unit rungs. THIRD-FUNNEL ANSWER for #99: the chord path
DOES route through `route_commit_channel`/`commits_as_source` (it delegates
to `pending_commit_intent`), so a tasked block's chord-flush cannot
re-introduce the fold — pinned by
`a_chord_on_a_tasked_row_re_commits_nothing_after_typing`. STILL OPEN and
broader than this funnel: a focused editor receiving no data-sync echo at
all leaves EVERY focused buffer stale against non-editor writes.)

## Missing piece

COVERAGE: no rung follows a non-editor-origin structural write with a
structural CHORD and then asserts the store; the headless keystone
structurally cannot, because its editor mirror is converged unconditionally
by the harness settle and can never hold a stale buffer. ORACLE (secondary):
the resulting state is self-consistent across render, SQL and disk, so only
a comparison against the reference model convicts it. Missing piece: a
windowed rung that dispatches a structural op from a non-editor origin while
an editor is focused, then presses a structural chord, and asserts the store
against the model.

## Remedy

FIXED 2026-08-11 (task #94). Mechanism PROBED: a focused editor receives NO
data-sync echo (one at the seed, none for its own keystrokes, none for the
external split), so its buffer never learns the row moved; the chord
pre-flush diffed the visible field against a baseline the keystroke sink
never advances and re-committed already-dispatched text, which against a
moved row is a revert. Fix: `EditorViewModel::chord_commit_intent` refuses
to flush a buffer the keystroke sink already persisted (`live_text ==
buffer`) and still flushes text the sink never saw (IME / programmatic
`set_value`); the focus-leave funnel is deliberately left as-is (the dogfood
blur control shows it does not revert, and #99's landed vacuity guard
asserts its commit). Gap closed by
`frontends/gpui/tests/structural_chord_stale_flush_windowed.rs` — RED at
base in both storage arms with the duplication signature, green after — plus
three `editor_view_model` unit rungs. Extends (does not duplicate) the
2026-08-10 undo-then-blur row. STILL OPEN, broader: the missing data-sync
echo to a focused editor leaves every focused buffer stale against
non-editor writes.
