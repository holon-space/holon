---
id: 2026-07-12-empty-typed-literal-first-commit-benign
date: 2026-07-12
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Empty `[[]]` typed → literal on first commit (benign), but RE-EDITING the
  block converts it to a zero-width Link mark (start==end, name "") and
  DELETES the chars from content; org writeback then serializes the empty mark
  as `]][[` on disk, and each subsequent boot/writeback cycle COMPOUNDS it
  (`]][[` → `]][[]][[`) — the known latent "]][[" corruption CONFIRMED, root =
  editor mark-parse accepts empty link + org renderer emits reversed brackets
  for zero-width marks
source_line: 903
---

## Bug

Empty `[[]]` typed → literal on first commit (benign), but RE-EDITING the
block converts it to a zero-width Link mark (start==end, name "") and
DELETES the chars from content; org writeback then serializes the empty mark
as `]][[` on disk, and each subsequent boot/writeback cycle COMPOUNDS it
(`]][[` → `]][[]][[`) — the known latent "]][[" corruption CONFIRMED, root =
editor mark-parse accepts empty link + org renderer emits reversed brackets
for zero-width marks

## Missing piece

empty-label links not in the marks generator alphabet; org round-trip oracle
would flag it if generated; editor mark-parse rung not driven headless

## Remedy

FIXED (2026-07-12). Semantics decision (from the links ruling: disk =
`[[id][label]]` resolved / `[[label]]` dangling): a zero-width link span has
NO content and NO target, so DROP it entirely (no mark, render nothing) —
the ruling's "represented state" needs a label or a target, and an empty
`[[]]` has neither. Three coordinated boundary fixes, all in
`holon-org-format` + `holon-api` (the SAME functions the ref model delegates
to, which is why the keystone self-cancelled): (1) PARSE boundary
`inline_marks::emit_mark` (Link arm) drops the link when the rendered label
is empty, so `extract_inline_marks("[[]]")`→`("",[])` — a zero-width Link
mark is never created (parse-don't-validate). (2) READ boundary
`holon_api::canonicalize_marks` now takes `&mut Vec` and `retain`s only
`start != end`, stripping any collapsed mark that the Loro Peritext layer
leaves behind when an edit deletes a marked span's entire text — this is the
editor/live-edit fix ("zero-width marks don't survive edits"), covering
`read_marks_from_text`, `marks_from_json`, and the PBT block-compare
normalizer at one choke point. (3) WRITEBACK boundary `render_inline_marks`
filters zero-width spans BEFORE bucketing events, so a close can never
precede its open — the final safety net guaranteeing `]][[` can never reach
disk regardless of source. ORACLE STRENGTHENING: since the ref model
(`pbt/types.rs::normalize_content_for_org_roundtrip`) delegates to these
fixed functions, its expected on-disk form flipped from the corrupt `]][[`
fixed point to a clean drop — a SUT that reintroduces `]][[` now DIVERGES
from the ref and fails the keystone (previously both agreed on the
corruption). Pinned by:
`inline_marks.rs::{empty_link_is_dropped_at_extraction_no_zero_width_mark,
render_zero_length_link_span_emits_nothing_never_reversed_brackets,
empty_link_typed_round_trip_is_stable_no_doubling}`,
`inline_mark.rs::canonicalize_drops_zero_width_marks`, and
`pbt/types.rs::org_roundtrip_tests::empty_link_normalizes_to_clean_fixed_point_no_reversed_brackets`.
Note the sibling P1 row below (matview duplicate rows on re-ingest) is the
SEPARATE mechanism that made the on-disk corruption COMPOUND across
restarts; this fix removes the corruption at the source, but that
duplicate-row escape is its own workstream.
