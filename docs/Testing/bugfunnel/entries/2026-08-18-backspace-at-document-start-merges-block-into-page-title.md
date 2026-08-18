---
id: 2026-08-18-backspace-at-document-start-merges-block-into-page-title
date: 2026-08-18
gap: ORACLE
secondary: null
status: OPEN
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
OPEN — deliberately NOT fixed in the `bug-backspace` lane. The correct behaviour
needs a ruling, posted to Martin's decision inbox as **BS-1**:

- (a) no-op when the parent is a page block, keeping the child→parent join for
  non-page parents — matches LogSeq, no title corruption;
- (b) refuse loudly with an `Err`, surfacing a banner on a routine keystroke;
- (c) keep merging into the page title.

Whichever is ruled, the fix follows the same order as the sibling entry: teach
the reference model the rule FIRST so the keystone goes red, add the
page-title-immutability invariant, then change the SUT.
