---
id: 2026-08-11-backspace-joining-block-into-task-above
date: 2026-08-11
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Backspace-joining a block into a task above it puts the caret INSIDE the
  keyword, and the next keystroke destroys the task.
source_line: 741
---

## Bug

(task #93, filed and fixed inside the task #78 arm-(d) lane; disclosed by
the lane as a fixture-only residual, upgraded to prod-reachable by the
fresh-context verifier's code-chain trace) **Backspace-joining a block into
a task above it puts the caret INSIDE the keyword, and the next keystroke
destroys the task.** `structural_focus_target` (`reactive.rs:4546`) arms the
`join_block` / `split_block` `cursor_offset` — a CONTENT offset — into a
buffer that now holds `KEYWORD + " " + content`, so the caret lands
`keyword.len() + 1` short; typing yields `TODOX milkbread`, which names no
keyword, so the store clears `task_state` and folds the corrupted keyword
into the text.

## Root cause

task #93, filed and FIXED in the task #78 arm-(d) lane; the lane disclosed
it as a fixture-only residual and the fresh-context verifier upgraded it to
prod-reachable by tracing the code chain — no automated test produced it:
**backspace-joining a block into a task above it puts the caret INSIDE the
keyword, and the next keystroke destroys the task.**
`structural_focus_target` (`reactive.rs:4546`) takes `cursor_offset`
straight off the `join_block` / `split_block` response — an offset into the
block's CONTENT — and arms it for a buffer that, since the editable surface
became a source projection, holds `KEYWORD + " " + content`. The caret
therefore lands `keyword.len() + 1` bytes short; typing there produces
`TODOX milkbread`, which names no keyword, so the store clears `task_state`
and folds the corrupted keyword into the text — the same silent-demotion
class as the vocabulary hole above. Backspace-at-start on a plain block
under a TODO task is an ordinary gesture. COVERAGE primary: no rung joins or
splits INTO a tasked block — every structural fixture operates on plain
blocks, so the two coordinate spaces never differed. Secondary ORACLE, and
it is why the keystone could not have caught it even with the draw: the
reference model reads the SAME content-coordinate boundary
(`delete_backward.rs`, `split_block.rs`), so both sides would have been
wrong together and `inv-editor-caret/mirror` would have stayed green. FIXED
2026-08-11: the offset crosses the prefix at every seed CONSUMER —
`EditorViewModel::content_offset_to_surface`, applied in GPUI's
`grab_focus_and_seed_caret`, in both `HeadlessEditorMirror` consumers, and
in the reference model's join and split arms. Red-first at the mapping
(`a_caret_seed_in_content_coordinates_crosses_the_keyword_prefix`, red
because the method did not exist) with a no-op arm proving a plain AND a
refused surface are untouched. STILL OPEN as a generator gap, deliberately
not closed here: no keystone draw performs a structural op against a tasked
block.)

## Missing piece

COVERAGE: no rung joins or splits INTO a tasked block, so the two coordinate
spaces never differed. ORACLE (why the draw alone would not have sufficed):
the reference model reads the same content-coordinate boundary in its join
and split arms, so both sides would have been wrong together and
`inv-editor-caret/mirror` would have stayed green.

## Remedy

FIXED 2026-08-11: the offset crosses the prefix at every seed CONSUMER —
`EditorViewModel::content_offset_to_surface`, applied in GPUI's
`grab_focus_and_seed_caret`, both `HeadlessEditorMirror` consumers, and the
reference model's join and split arms. Red-first at the mapping
(`a_caret_seed_in_content_coordinates_crosses_the_keyword_prefix` — red
because the method did not exist) with a no-op arm proving a plain AND a
refused surface are untouched. STILL OPEN as a generator gap: no keystone
draw performs a structural op against a tasked block.
