---
id: 2026-07-20-gpui-dogfood-sqlonly-desktop-block-containing
date: 2026-07-20
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  GPUI dogfood (SqlOnly desktop): `split_block` on a block containing
  `[[link]]` marks DESTROYS the links. Splitting "Owner is Ada Lovelace and
  reviewer Charles Babbage" (marks `[9,21]`+`[35,50]`) at position 8 leaves
  the retained block "Owner is" (8 chars) carrying the ORIGINAL, now
  OUT-OF-BOUNDS marks `[9,21]`+`[35,50]` (all offsets > content length — the
  exact `scalar_range_to_bytes exceeds text length` crash condition at
  `rich_text_runs.rs:169`), while the split-off block "Ada Lovelace and
  reviewer Charles Babbage" gets `marks=NULL`. Both links render as plain text
  and are persisted link-free to disk (`* Owner is` / `* Ada Lovelace and
  reviewer Charles Babbage`, no `[[...]]`). The renderer tolerated the OOB
  marks (no crash this build) so the reported `scalar_range` panic did NOT
  fire, but this is its mechanism. `split_block` does not split/shift marks at
  all.
source_line: 1033
---

## Bug

GPUI dogfood (SqlOnly desktop): `split_block` on a block containing
`[[link]]` marks DESTROYS the links. Splitting "Owner is Ada Lovelace and
reviewer Charles Babbage" (marks `[9,21]`+`[35,50]`) at position 8 leaves
the retained block "Owner is" (8 chars) carrying the ORIGINAL, now
OUT-OF-BOUNDS marks `[9,21]`+`[35,50]` (all offsets > content length — the
exact `scalar_range_to_bytes exceeds text length` crash condition at
`rich_text_runs.rs:169`), while the split-off block "Ada Lovelace and
reviewer Charles Babbage" gets `marks=NULL`. Both links render as plain text
and are persisted link-free to disk (`* Owner is` / `* Ada Lovelace and
reviewer Charles Babbage`, no `[[...]]`). The renderer tolerated the OOB
marks (no crash this build) so the reported `scalar_range` panic did NOT
fire, but this is its mechanism. `split_block` does not split/shift marks at
all.

## Missing piece

No invariant asserts (a) every mark range stays within `[0, len(content)]`
and (b) marks are preserved/correctly partitioned across a split. The
keystone likely mirrors the defect (model split may also drop marks) so
random PBT never diverges — needs a mark-integrity invariant + a
split-preserves-marks oracle (model-first red). Add a `content`-length ⊇
all-mark-ranges check to `invariants/`.

## Remedy

FIXED 2026-07-20 (marks PARTITION across the cut via shared
holon_api::split_content_marks used by prod AND reference model; straddling
links drop both sides, formatting truncates; real-Turso red->green
reproduced the exact dogfood signature; multibyte scalar-bridge locked by
executed cases; verifier CONFIRMED. OPEN residue: Loro-authority marks land
in SQL projection only — Peritext-native split follow-up; marks-equality
keystone oracle still missing)
