---
id: 2026-08-08-typed-into-editor-never-parsed-link
date: 2026-08-08
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  `[[wiki links]]` typed into the editor are never parsed — no Link mark, no
  `block_links` row, no styling, nothing clickable, no backlink, even when the
  target page exists — and the next app start silently rewrites the user's
  text when the file is re-ingested
source_line: 762
---

## Bug

(dogfood-explorer gate pass) **`[[wiki links]]` typed into the editor are
never parsed — no Link mark, no `block_links` row, no styling, nothing
clickable, no backlink, even when the target page exists — and the next app
start silently rewrites the user's text when the file is re-ingested**
(`"see [[Journals]] now"` → `"see Journals now"`, `block_links` rows[0] →
rows[5], journal sha 342c6888→483e489b). The same typing path extracts
Bold/Italic correctly, and the identical text authored in an .org file
ingests perfectly, so only the Link kind is missing from the editor write
path.

## Root cause

dogfood-explorer gate pass — **`[[wiki links]]` typed into the editor are
never parsed, and the next app start silently rewrites the user's text**.
Typing `see [[Journals]] now` (target page EXISTS) leaves `content="see
[[Journals]] now"`, `marks=null`, `block_links` rows[0], `backlinks` rows[0]
— no styling, nothing clickable, no backlink. The same typing path DOES
extract other marks (`*bold* mid /ital/ end` →
`[{0,4,Bold},{9,13,Italic}]`), so only the Link kind is missing from the
editor write path; authoring the identical text in an .org file ingests
perfectly (`content="ingest link to Journals here"`,
`marks=[{15,23,Link,target={name:"Journals"}}]`, junction row resolved to
`block:journals`). CHURN, which is the sharp end: those blocks reach disk as
literal `[[Journals]]`, so the NEXT cold boot re-ingests them and the parser
the editor skipped now runs — `"see [[Journals]] now"` → `"see Journals
now"`, `"link to [[Some Page ]] here"` → `"link to Some Page here"`,
`block_links` rows[0] → rows[5], journal file sha 342c6888→483e489b. The
user's characters change without an edit. The padded case additionally spent
the whole session with SQL holding `[[Some Page ]]` while disk held `[[Some
Page]]`. COVERAGE: the keystone's link coverage arrives through
ingest/BulkExternalAdd, never through a typed GPUI keystroke burst, so no
draw exists in which a link is TYPED; the oracle already compares the Marks
facet and would red instantly. Evidence:
`docs/Testing/fixture-logs-2026-08-08/dogfood-typed-wiki-links-not-parsed.txt`.
Note the ingest half of the 2026-08-08 padded-link work is GREEN and was
re-verified here: padded `[[Journals ]]` trims and resolves, write-back
emits the scheme-carrying `[[block:journals][Journals]]` per ORG_SYNTAX.md,
and ten sha samples over 10s plus a forced re-ingest are byte-identical with
zero ERROR/WARN. FIXED 2026-08-08 (task #12) and the triage SHARPENED by the
fix: the editor write path was never the missing half —
`set_field("content")` has adopted links since links-increment-3 and all 5
pre-existing `crates/holon/tests/live_edit_link_marks.rs` tests pass at
base. The unparsed path is `block.create`: the creation slot commits the
whole typed line through `create`, not `set_field`, so a block BORN with a
link kept raw bytes and NULL marks (`PROBE create: content="see [[Journals]]
now" marks=None links=[]`, driven through the real dispatcher). Fix = parity
at the same boundary: `create` runs the SAME `extract_inline_marks_with`
when the caller supplies no `marks` of its own (org ingest and `split_block`
do, and are left untouched). The COVERAGE call stands and gains a measured
ENVIRONMENT sibling: even a link-carrying create draw could not have red'd,
because the keystone's settle re-ingests its own write-back IN-SESSION and
launders the raw content before any invariant looks, while prod does not
re-ingest its own writes until the next boot — which is precisely why the
user sees the rewrite at startup. Third hole found while probing, filed
separately and NOT closed here: the keystone cannot TYPE AT ALL under the
shipped SqlOnly default (`SutEditorMirrorWrite` is withheld when Loro is
off, components.rs:4013, though prod's SqlOnly editor is cell-free and
works). Fix evidence:
`docs/Testing/fixture-logs-2026-08-08/typed-wiki-links-create-boundary-fix.txt`.
COVERAGE NOT CLOSED — say it plainly: no GENERATED draw creates a block
whose content carries link markup, and the hand-authored row added here
stays GREEN with the product fix reverted, because the keystone's settle
launders raw content through an in-session re-ingest. The ONLY regression
lock on the product half is the crate test
`creating_a_block_with_a_typed_link_adopts_it_at_the_write_boundary`. The
generator arm — mint markup-shaped content on a create draw — and the
laundering that would still mask it remain OPEN)

## Missing piece

The keystone's link coverage arrives through ingest/BulkExternalAdd; no draw
exists in which a link is TYPED keystroke-by-keystroke into the editor. The
oracle already compares the Marks facet and would red on the first such
draw.

## Remedy

**FIXED 2026-08-08 (task #12).** Root cause is the `create` intent boundary,
not the editor: `set_field("content")` already adopted links, but the
creation slot commits typed text through `block.create`, which was unparsed
— so a block born with a link kept raw bytes + NULL marks + no junction.
`OperationDispatcher` now runs the same `extract_inline_marks_with` on
`create` when the caller supplies no `marks` (ingest / `split_block` do, and
keep theirs). Red-first: reference modeled FIRST
(`create_block_under_with_id` runs the org lens, closing the 2026-07-31
ORACLE row that made link cases inexpressible as hand-authored rows) →
keystone row `wiki-link-created-in-the-ui-is-adopted-at-the-write-boundary`
red on 7 invariants, then green; crate red
`creating_a_block_with_a_typed_link_adopts_it_at_the_write_boundary` (`left:
"see [[Linked Page Test]] now"` vs `right: "see Linked Page Test now"`) +
`adopted_create_content_survives_a_re_ingest_unchanged` (the silent-rewrite
half: what the store now holds re-ingests byte-identical), with
`a_create_that_supplies_marks_is_not_reparsed` as a pre-passing no-clobber
guard. Adoption is gated on the authoring ORIGIN (`AuthoredInput::Live`,
declared only by the engine for `OpOrigin::User`/`Agent`), NOT on the
absence of a `marks` param: adversarial verification proved the shape test
breaks undo — `capture_row` filters NULL columns, so a delete-inverse looks
exactly like fresh typing and replay re-parsed the very bytes it exists to
restore. Locked by
`undo_of_a_delete_restores_bytes_verbatim_even_when_adoption_would_apply`.
Only the CREATE arm is gated: the same trap exists on the EDIT arm (present
since links-increment-3, NOT introduced here — A/B: it reds with the create
arm disabled), and gating that arm too is DELIBERATELY NOT DONE
(`operation_dispatcher.rs:637-666`), because `capture_row` filters NULL
columns and so a content inverse cannot express "restore marks to NULL" —
replay-time adoption is what clears stale marks today, and gating the arm
instead regresses `undo_link_add_restores_prior_pair` with an out-of-bounds
mark (`range [10,24)` over `content="Review PR"`, char_len=9; measured, not
predicted). Undo is thus correct for adopted blocks precisely because replay
re-parses, and wrong for raw ones for the same reason; the resolution both
need is inverses that carry marks explicitly, i.e. a `capture_row` change
touching every inverse. ESCALATED to Martin, tracked as task #22;
`undo_of_a_content_edit_restores_raw_previous_bytes` is committed
`#[ignore]`d as the named defect and is the future gap-closing rung (it
FAILS when run: `left: "see Journals now"` vs `right: "see [[Journals]]
now"`). **STATUS CORRECTION 2026-08-16 (D27.a, lane-mark-policy): #22 has
LANDED and this sentence is stale.**
`crates/holon/tests/live_edit_link_marks.rs` carries no `#[ignore]` anywhere
and the whole file is green (13 passed, 0 ignored),
`undo_of_a_content_edit_restores_raw_previous_bytes` included. The shipped
mechanism is the rich inverse this row asked for, documented at
`crates/holon/src/api/operation_dispatcher.rs:721-733`: a content inverse
carries the prior text and marks as one `{text, marks}` Object, and the
adoption arm's `as_string()` match takes String values only, so a replayed
inverse never enters that arm and its restored bytes are never re-parsed.
The EDIT arm is now safe by SHAPE rather than by origin, which is why both
the adopted and the raw populations pass. Recorded because the prose
outlived the fix: a D27.a scope item was written against this row and would
have "re-fixed" already-working code. The measured out-of-bounds mark this
row cites (`range [10,24)` over `content="Review PR"`, char_len 9) is
therefore historical, not live. KNOWN RESIDUAL, not fixed here: neither arm
checks `content_type`, so a `Live` create of a source/code block whose text
contains `*bold*` or `[[x]]` is stripped and marked — symmetric with the
pre-existing edit-arm behaviour, extended to born-blocks by this lane.
COVERAGE NOT CLOSED: no generated draw creates markup-carrying content, and
the keystone row stays green with the product fix reverted (laundering), so
the crate tests are the only product-half locks. Measured ENVIRONMENT
sibling: the keystone launders this defect via an in-session re-ingest prod
does not perform. Fix evidence
`docs/Testing/fixture-logs-2026-08-08/typed-wiki-links-create-boundary-fix.txt`.
The ingest half of the same-day padded-link work was re-verified GREEN here
(padded `[[Journals ]]` trims and resolves; write-back emits
`[[block:journals][Journals]]` per ORG_SYNTAX.md; byte-identical over 10 sha
samples and a forced re-ingest, zero ERROR/WARN). Evidence
`docs/Testing/fixture-logs-2026-08-08/dogfood-typed-wiki-links-not-parsed.txt`.
