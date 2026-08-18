---
id: 2026-08-18-backspace-at-document-start-merges-block-into-page-title
date: 2026-08-18
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Backspace at offset 0 in a document's first block appends that block's text to
  the PAGE block, silently rewriting the page title, and deletes the block.
---

## Bug
Found by the fresh-context verifier probing the `bug-backspace` lane
(2026-08-18) — a PRE-EXISTING defect, not introduced by that lane's fix.

Put the caret at offset 0 of the FIRST block of a document and press Backspace.
The block has no previous sibling, so `join_block` takes its child→parent arm and
the parent is the PAGE block. The page's content — its title — gets the block's
text appended, and the block is deleted. The op returns `Ok`.

Reproduced independently against the `MemStore` fixture
(`crates/holon-core/src/block_operations_tests.rs`, throwaway probe, not
committed):

```
PROBE page-before="Page PG" page-after=Some("Page PGContent B") block-B=None op=true
```

A page title is identity, not content: it names the org file and is the target
of every `[[link]]`. A routine keystroke must not rewrite it.

## Root cause
`crates/holon-core/src/traits.rs` — `BlockOperations::join_block`, the
`into_parent` arm (`prev_opt.is_none()`). It resolves the merge target as
`block.parent_id()` with no check on what that parent IS, so a page parent is
merged into like any text block. The arm predates the 2026-08-18
visible-outline-predecessor fix and is unchanged by it: that fix only altered
the branch where a previous sibling exists.

The reference model mirrors the same rule (`ReferenceState::join_block` falls
back to `block.parent_id` when there is no previous sibling), so model and SUT
agree — the same differential-oracle blind spot as
[[2026-08-18-backspace-merges-into-prev-sibling-not-visible-predecessor]].

## Missing piece
Not coverage: the keystone CAN generate this. `join_block_preconditions`
(`crates/holon-integration-tests/src/pbt/transitions/join_block.rs`) excludes the
FOCUSED block from being a page but places no constraint on the parent, and
`is_text_block` is true for a page block (pages are Text-typed with a `Page`
tag), so the `parent_ok` arm admits a page parent and the transition is
reachable.

What is missing is an invariant asserting that a page block's content changes
only through a rename — nothing in the catalog would go red when a keystroke
edits a page title.

## Remedy
FIXED. Martin ruled **BS-1 = (a)**: when the merge would go `into_parent` and
that parent is a PAGE block, Backspace-at-0 is a NO-OP — no join, no delete,
caret unmoved. The child→parent join stays for non-page parents.

Model first, so the keystone went red for the right reason. Red-first evidence
(`/tmp/bug-backspace-bs1red-*.log`, model fixed, SUT unfixed) — merging into the
title RENAMED the page, which re-homed its org file out from under the render:

```
Applying transition 3/3: DeleteBackward(DeleteBackward { count: 1 })
[FileSyncController] write-back SKIPPED: … doc=block:structural-page
    difference=block:parent@block:structural-page held=3 authority=2
panicked at crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:2120:18:
SutOrgRender: read org file: Custom { kind: NotFound,
  error: "No such file or directory (in-memory): …/structural-page.org" }
test result: FAILED. 8 passed; 1 failed
```

That IS the defect: the page's content is the name of its file, so the keystroke
did not merely edit a title — it moved the whole document.

Changes:
- `holon_pbt_core::capabilities::join_merge_target` returns `None` when there is
  no previous sibling and the parent is a page — the one shared model rule.
- `transitions/delete_backward.rs` and `transitions/press_key.rs` add the page
  check to their child→parent arms; `transitions/join_block.rs`'s `parent_ok`
  precondition excludes page parents, so the structural transition no longer
  generates a join prod refuses.
- `crates/holon-core/src/traits.rs` `join_block` returns
  `OperationResult::irreversible(vec![])` when the parent is a page, read
  through `is_page_authoritative` (the page-boundary-guard rule: a `Page` tag
  committed but not yet in the matview would otherwise read as a non-page).

- `crates/holon-frontend/src/headless_editor_mirror.rs` (Backspace-at-0 arm)
  retires the tracked caret only when the join moved focus off the block. It
  used to `forget` unconditionally, so after a REFUSED join the next keystroke
  re-seeded at end-of-text — the SUT driver diverging from prod GPUI, whose
  `InputState` a refused join never touches. Red on the pristine mirror
  (`/tmp/bug-backspace-bs1mred-31564.log`, `/tmp/bug-backspace-bs1probe6-3580.log`):
  `DeleteBackward(2)` → SUT `"paren"` vs ref `"parent"`, caret 5 vs 0;
  `PressKey(enter)` → SUT sibling order `[parent, new, c1, c2]` vs ref
  `[new, parent, c1, c2]`.

Pinned by the hand-authored keystone cases
`backspace-at-document-start-is-a-noop` (`FocusEditableText(block:parent)` ·
`MoveCursor(0)` · `DeleteBackward(1)`),
`backspace-at-document-start-twice-stays-a-noop` (`… DeleteBackward(2)`) and
`backspace-at-document-start-then-enter-splits-above` (`… DeleteBackward(1)` ·
`PressKey(enter)`), all GREEN, and the unit test
`block_operations_tests::join_block_into_a_page_parent_is_a_noop`.

Still open, deliberately: no invariant asserts that a page's content changes
only through a rename. This entry's defect is now pinned by a case, but the
CLASS — any op silently rewriting a page title — remains unguarded.
