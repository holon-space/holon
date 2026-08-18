---
id: 2026-08-18-editor-seeded-from-stripped-content-not-source
date: 2026-08-18
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  Entering a block whose content carries inline markup showed the stripped label
  with the styling still applied — the `~` delimiters were invisible while
  editing — because the editable surface was seeded from the content column
  instead of the org source its marks reconstruct.
---

## Bug

Martin, dogfooding the GPUI app 2026-08-18 (bug #7):

> Block content with `~monofont~` does not round-trip properly: when I leave the
> block and enter it again I don't see the `~` any more even though they seem to
> be there in the background. I can even change the `monofont` text and it will
> still be colored properly when I leave the block again.

The ratified position is F2-arm=d (2026-08-11): the editor IS a source
projection. While editing, the block shows its org source — `~monofont~` with
the markers — and the caret is a source offset; the rendered, styled form is the
non-editing state only.

## Root cause

The editable surface was seeded from the CONTENT column, which is the stripped
label. `EditorViewModel::project_authority` took `(content, task_state)` and
applied only the task-keyword projection
(`holon_org_format::source_projection`); the marks that turn `monofont` back
into `~monofont~` were never read. The gap was known and documented in place —
"RAW-seam hook: `text` MAY be a raw reconstruction (`render_inline_marks`) once
raw-edit mode lands; today callers pass stored (stripped) content" — and no
caller ever passed anything else.

Both seeding paths had the same shape:

* `frontends/gpui/src/views/editor_view.rs` `EditorView::project_authority` read
  `task_state` off the shared row cell and nothing else, then handed the raw
  content column to the VM.
* `crates/holon-frontend/src/headless_editor_mirror.rs` seeded from
  `QueryEngine::block_editor_source_by_id`, whose SELECT read `content` and
  `task_state` and not `marks`.

Both halves of Martin's report follow: the delimiters are absent because the
buffer never had them, and the styling survives because the marks live on in the
store and the read projection paints from them — the mark is not in the text the
editor shows, so editing the text cannot disturb it.

This is the SAME seam as
`2026-08-18-split-position-measured-on-the-editor-surface`, from the other side.
That entry is about a caret measured on a surface holding markup the content
column does not; this one is about a surface that dropped the markup entirely.
One mechanism, two symptoms — the caret mismatch shows up in the red log here
too (`reference model cursor_byte=12, SUT tracked caret=8`).

## Missing piece

ORACLE. The keystone can generate the interaction trivially — create a block
carrying markup, focus it — and `inv-editor-text/mirror` compares exactly the
right two strings. It stayed green because the REFERENCE modelled the defect:
`RefEditorMirror::editor_surface_text` answered with the stored content, so
model and prod agreed on the wrong answer.

Secondary COVERAGE: the transition sequence was evidently never drawn either;
it is pinned deterministically now as
`entering-a-block-with-inline-markup-shows-its-source` in
`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`.

RED, after correcting the reference and BEFORE the production fix:

```
[inv-editor-text/mirror] Live editor text mismatch on block:markup-seam:
  reference: "a [[abcd]] b"
  SUT MutableText: "a abcd b"
[inv-editor-caret/mirror] Caret mismatch on block:markup-seam:
  reference model cursor_byte=12, SUT tracked caret=8
```

GREEN after it: `1 test run: 1 passed, 8 skipped`.

## Remedy

The surface is reconstructed from `(content, marks)` before the keyword
projection, at every seed.

* `holon_api::query_engine::EditorSource` replaces the
  `(Option<String>, Option<String>)` tuple `block_editor_source_by_id` returned;
  the Turso implementation selects `marks` alongside and parses them at the
  boundary, failing loud on a row whose marks it cannot read rather than
  answering with an empty span set (which is indistinguishable from a block that
  has none).
* `EditorViewModel::project_authority(content, marks, task_state)` renders the
  marks with `holon_org_format::render_inline_marks` first, then projects the
  keyword on top of that SOURCE. `surface_prefix` is now the keyword alone —
  `seed.text.len() - source.len()` — because the markup delta is not a prefix
  and is crossed by the offset map in `surface_caret_to_content`.
* GPUI's `EditorView::project_authority` reads `marks` off the same shared row
  cell it already read `task_state` from, so a peer's mark edit under an open
  editor changes what the surface shows.
* The reference gained `RefEditorMirror::editor_source_text` (surface minus the
  keyword) so the keyword prefix is derived by subtracting the SOURCE, never the
  content.

Fixing this also made the inline-markup flavour of the split escape reachable
headlessly for the first time; it is pinned as
`enter-after-typed-inline-markup-splits-at-the-content-caret`.

Three harness consequences followed, each a place that had been treating the
content column as the thing the user sees, and each caught by an existing
hand-authored case (`split-of-a-marked-block-keeps-the-right-half-rich-under-loro`)
going red:

* The `SplitBlock` keystroke driver counted `right` presses in the CONTENT
  column. It now asks the driver
  (`UserDriver::surface_chars_before_content`, backed by the mirror's view
  model) where a content byte sits in the SURFACE, and panics rather than
  aiming at a glyph it cannot locate.
* `HeadlessEditorMirror::seeded_editor` mixed authorities — content from the
  live Loro cell, marks from the SQL row. It now renders marks only when the
  two agree; a cell that has outrun the projection is shown unmarked rather
  than with delimiters at offsets that text never had.
* The mirror left a split's newly-focused block with NO editor VM, so
  `editor_live_text` fell back to the Loro content cell and reported the
  stripped column as "what the editor shows". The settle now mounts an editor
  on the focused block, which is what prod's focus response does and what the
  reference already modelled (`open_active_editor`).
