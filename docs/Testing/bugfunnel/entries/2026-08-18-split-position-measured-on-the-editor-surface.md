---
id: 2026-08-18-split-position-measured-on-the-editor-surface
date: 2026-08-18
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Enter dispatched `split_block` with a caret measured on the editable surface
  while the op cuts the content column, so a split after inline markup or a task
  keyword was refused and Enter did nothing.
---

## Bug

Martin, dogfooding the GPUI app (log `/private/tmp/holon-cold.log`, lines
~11821 and ~12134):

```
ERROR live_data.subscribe_actor{source="focus_roots"}: holon_frontend::reactive:
dispatch_intent_chain: block.split_block failed — aborting remaining intents:
dispatch_intent_sync: block.split_block failed: Operation 'split_block' on entity
'block' failed: Split position 68 exceeds content length 66
```

The block read `Add column ~installation_source~ which can contain Ansible
reference` — 68 bytes as authored, 66 stored. Enter at the end of the line did
nothing at all; the whole intent chain aborted.

## Root cause

Two coordinate systems meet at the Enter key and neither side converted.

* The editable buffer is VAULT SYNTAX. The user's `~installation_source~` is
  written back through `set_field("content")`, and
  `crates/holon/src/api/operation_dispatcher.rs:744` extracts inline markup
  there: the value becomes the stripped label and the `~` pair becomes a `Code`
  mark. The buffer keeps all 68 bytes; the content column keeps 66. The same
  delta exists for a task keyword — a task shows `TODO seam` and stores `seam`.
* `crates/holon-frontend/src/editor_view_model.rs:1222` (the Enter arm of
  `structural_block_action`) put the raw editor caret into `split_block`'s
  `position`, and `crates/holon-core/src/traits.rs:1299` validates and cuts that
  position against the content column.
* The one conversion that existed, `EditorViewModel::content_offset_to_surface`
  (`editor_view_model.rs:485`), modeled the delta as a keyword PREFIX only
  (`offset + surface_prefix`) and ran only in the content→surface direction, for
  caret seeding. There was no surface→content conversion anywhere.

Out-of-range was the loud half. In range the same mismatch cut silently in the
wrong place: with `~a~ bcd` on screen the caret after the space is surface byte
4, and splitting the content `a bcd` at 4 yields `a bc` / `d`.

## Missing piece

Two, one per gap.

COVERAGE — the escaping sequence is expressible in the keystone catalog today
(`CreateBlockUnderFocus` → `ToggleState(TODO)` → `FocusEditableText` →
`PressKey(Enter)`; focus parks the caret at end of the surface) but was never
drawn. Pinned as the deterministic hand-authored case
`enter-at-end-of-a-task-line-splits-at-the-content-caret`
(`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`),
which reproduces the escape verbatim against the pre-fix tree:

```
Applying transition 4/4: PressKey(PressKey { chord: KeyChord({Enter}) })
panicked at crates/holon-integration-tests/src/pbt/driver_input.rs:565:37:
[PressKey] send_raw_keystroke(enter) failed: dispatch_intent_sync:
block.split_block failed: Operation 'split_block' on entity 'block' failed:
Split position 9 exceeds content length 4
```

ORACLE — even once drawn, only the driver's dispatch panic could have flagged
it: `split_block_apply_to_ref` (`transitions/split_block.rs`) had been taught to
refuse a surface caret "the way prod does", so the reference model agreed with
the defect and no invariant described the right answer.

The INLINE-MARKUP flavour could not be driven headlessly at first: a
`TypeChars("[[abcd]]")` case diverged one step earlier, because the editable
surface was seeded from the stripped content column. That is its own escape —
`2026-08-18-editor-seeded-from-stripped-content-not-source` — and once it was
fixed the markup flavour became reachable and is pinned as
`enter-after-typed-inline-markup-splits-at-the-content-caret`. Martin's exact
68/66 string is also pinned at unit level
(`a_caret_after_typed_inline_markup_lands_on_the_stripped_content`).

## Remedy

One coordinate system, converted at the boundary, refusing rather than clamping.

* `holon_org_format::source_content_offsets` (`inline_marks.rs`) records, in the
  single extraction pass, which source bytes produced which content bytes, and
  answers `content_offset` / `source_offset` in either direction. Out of range
  is an `Err` naming both numbers.
* `holon_frontend::editor_view_model::surface_caret_to_content` composes that
  with the keyword prefix. It is the ONE seam crossing: production's
  `EditorViewModel::structural_caret` and the keystone's reference model
  (`transitions/press_key.rs`) both call it.
* `structural_block_action` now takes a `StructuralCaret` carrying both
  coordinates rather than one `usize`, so no caller can hand a surface byte to a
  content-cutting op. Enter reads the content byte; Backspace-at-0 keeps reading
  the surface byte, because "is the caret at the very start" is a question about
  what the user sees.
* GPUI (`editor_view.rs`) and dioxus-web (`editor.rs`) drop the keystroke with a
  logged error when the caret does not land on the surface it was measured on,
  rather than splitting somewhere else.
