---
id: 2026-07-13-live-edit-link-marks-wiped-blur
date: 2026-07-13
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Live-edit link marks WIPED on blur/refocus (Martin dogfood): a typed
  `[[Page]]` survives the FIRST commit (content stripped to `Page`, `marks`
  populated, junction row created) but is silently replaced by plain text
  after clicking away and back — the marks + `block_links` row vanish. Root
  cause: the links-increment-3 follow-up (`OperationDispatcher`, task #66,
  sibling of row 83) ran `extract_inline_marks` on EVERY block
  `set_field("content")` and unconditionally issued a second
  `set_field("marks", …)` — writing `marks=Null` whenever the new content bore
  no `[[…]]` syntax. In SqlOnly the editor hydrates its buffer from the stored
  STRIPPED `content` and re-commits THAT verbatim on blur (see
  `frontends/gpui/src/views/editor_view.rs` blur path +
  `render/builders/editable_text.rs` seed-from-`content`), so the re-commit
  carried the mark-free label `Page` → the follow-up nulled the live marks.
  The same over-dispatch also fired a spurious second `set_field` on
  plain-text edits, doubling the undo replay tally (broke
  `undo_foundation::{persistence_survives_reload,
  undo_applies_and_redo_staleness_is_symmetric}`).
source_line: 971
---

## Bug

Live-edit link marks WIPED on blur/refocus (Martin dogfood): a typed
`[[Page]]` survives the FIRST commit (content stripped to `Page`, `marks`
populated, junction row created) but is silently replaced by plain text
after clicking away and back — the marks + `block_links` row vanish. Root
cause: the links-increment-3 follow-up (`OperationDispatcher`, task #66,
sibling of row 83) ran `extract_inline_marks` on EVERY block
`set_field("content")` and unconditionally issued a second
`set_field("marks", …)` — writing `marks=Null` whenever the new content bore
no `[[…]]` syntax. In SqlOnly the editor hydrates its buffer from the stored
STRIPPED `content` and re-commits THAT verbatim on blur (see
`frontends/gpui/src/views/editor_view.rs` blur path +
`render/builders/editable_text.rs` seed-from-`content`), so the re-commit
carried the mark-free label `Page` → the follow-up nulled the live marks.
The same over-dispatch also fired a spurious second `set_field` on
plain-text edits, doubling the undo replay tally (broke
`undo_foundation::{persistence_survives_reload,
undo_applies_and_redo_staleness_is_symmetric}`).

## Missing piece

`crates/holon/tests/live_edit_link_marks.rs` (increment 3) only exercised
ONE-SHOT commits (add link / remove link / id-link); it never committed a
block's own already-stripped label a SECOND time — the exact blur/refocus
re-commit sequence — so no case distinguished "re-commit of the stored
label" (must preserve marks) from "user removed the link" (must clear). No
idempotence/no-op-commit oracle existed either: the follow-up nulled marks
with nothing asserting a mark-neutral commit leaves marks untouched.
Headless keystone types content through drivers but the
blur→hydrate-from-`content`→re-commit loop needs a live GPUI editor buffer
(same environment class as rows 65/66).

## Remedy

FIXED (2026-07-13): the follow-up is now DERIVED from a comparison, decided
BEFORE the content write lands so it reads the block's PRIOR stored state
(new `OperationProvider::read_block_content_marks`, `Ok(None)`-default =
unreadable; `SqlOperationProvider` answers via `read_field_old_value`).
Contract (marks = truth, per links-ruling): fire EXACTLY when the extracted
mark set differs from stored — (a) readable provider: skip when marks
unchanged, skip a null-producing re-commit whose stripped label already
equals stored `content` (the blur path), else dispatch (incl. a legitimate
`marks=Null` when a genuine edit REMOVES the link — new label ≠ stored
`content`); (b) unreadable provider (Loro CRUD authority / test stubs): fail
SAFE — dispatch only when the new content actually yields marks (a link was
typed), NEVER null on unknown prior state. Content is still ALWAYS stripped
to the label (idempotent). Red-first:
`blur_recommit_of_stripped_label_preserves_marks` (marks + junction survive
a re-commit of the stripped label) + the 3 pre-existing increment-3 tests
stay green + the 2 undo_foundation tests go green. Files:
`crates/holon-core/src/traits.rs`,
`crates/holon/src/core/sql_operation_provider.rs`,
`crates/holon/src/api/operation_dispatcher.rs`. Keystone COVERAGE residue
OPEN: a headless case cannot drive the blur→hydrate loop; the catching rung
is the live-MCP GPUI twin — add a type-link → blur → refocus →
assert-marks-survive sequence there (rows 65/66 environment family).
