---
id: 2026-07-18-gpui-first-click-into-unfocused-text
date: 2026-07-18
gap: COVERAGE
secondary: PERCEPTION
status: FIXED
summary: >-
  GPUI: the FIRST click into an UNFOCUSED text block places the caret at text
  END instead of the clicked char (subsequent clicks into the same,
  now-focused block are fine). The gpui-component `Input` hit-test is CORRECT
  (the caret initially lands at the clicked offset); holon's focus-GAIN
  convergence then clobbers it — `editable_text.rs:151` sees `just_focused`
  and calls `EditorView::converge_input` (`editor_view.rs:726`), which, when
  `InputState.value` ≠ the content authority, calls the fork's `set_value`
  that UNCONDITIONALLY resets the selection (end for single-line, 0 for
  multi-line). Holon's caret-restore uses a Loro cursor anchor
  (`anchor_cursor`/`resolve_cursor`) which is a NO-OP in SqlOnly mode (no
  anchor exists), so the caret is left at end. Second+ clicks
  converge-early-return (`editor_view.rs:751`, content already matches) and
  are unaffected. Found by hand-diagnosis of a dogfooding UI report, not by an
  automated test.
source_line: 1003
---

## Bug

GPUI: the FIRST click into an UNFOCUSED text block places the caret at text
END instead of the clicked char (subsequent clicks into the same,
now-focused block are fine). The gpui-component `Input` hit-test is CORRECT
(the caret initially lands at the clicked offset); holon's focus-GAIN
convergence then clobbers it — `editable_text.rs:151` sees `just_focused`
and calls `EditorView::converge_input` (`editor_view.rs:726`), which, when
`InputState.value` ≠ the content authority, calls the fork's `set_value`
that UNCONDITIONALLY resets the selection (end for single-line, 0 for
multi-line). Holon's caret-restore uses a Loro cursor anchor
(`anchor_cursor`/`resolve_cursor`) which is a NO-OP in SqlOnly mode (no
anchor exists), so the caret is left at end. Second+ clicks
converge-early-return (`editor_view.rs:751`, content already matches) and
are unaffected. Found by hand-diagnosis of a dogfooding UI report, not by an
automated test.

## Missing piece

the keystone's `ClickBlock`/`click_entity(id, region)` driver clicks to
FOCUS a whole block by entity id — it carries NO intra-text char/pixel
offset (`driver_input.rs:319,372`), so it cannot express "click at char N
inside an unfocused block", which is the exact interaction that triggers the
caret jump; and there is no caret-byte-offset invariant, so even a focusing
click has no oracle to flag the misplacement (PERCEPTION: caret position is
a visual/UX property with no formal oracle in the current harness). Closing
this needs an offset-bearing click driver AND a post-click caret-position
oracle

## Remedy

FIXED — `converge_input` now captures the pre-`set_value` caret byte offset
(`state.cursor()`) and, when no Loro anchor resolves (the SqlOnly case),
restores it UNCONDITIONALLY via a new pure helper
`preserved_caret(old_offset, new_text)` that clamps to the new text's length
and snaps down to a UTF-8 char boundary; the Loro-anchor path is kept as the
refinement for the length-changed / concurrent-edit case. Red-first proven
on the extracted pure helper (`views::editor_view::caret_preservation` mod,
6 tests: mid-text preserved, past-end pins to length, equal-to-length, zero,
multibyte-boundary snap-down, multibyte valid-boundary kept) — a stubbed
"caret-to-end" impl fails exactly the placement + multibyte cases (e.g.
mid_text expected 3, got 11). Coverage gap for pixel-click caret placement
remains OPEN (no pixel-click driver in the keystone; a pixel harness was
deliberately NOT built this session).
