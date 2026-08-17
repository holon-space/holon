---
id: 2026-07-20-journal-page-click-sigabrt-user-report
date: 2026-07-20
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Journal-page click SIGABRT (user report, desktop): `scalar_range_to_bytes:
  0..11 exceeds text length 10` at `rich_text_runs.rs:169` — a link
  `MarkSpan.end` outlives its block content. Producers:
  `convert_block_to_page` (`operation_engine.rs:656-676`) writes a full-span
  link mark via a DECOUPLED marks-only `set_field` (content untouched); later
  content-only trailing trim (org writeback `models.rs:523/:904`, SQL
  `trimmed_content` `sql_operation_provider.rs:332`) shortens content N→N-1
  while `end=N` persists; `split_block` produces the same OOB state (row 237).
  NO read boundary clamps marks vs content (`canonicalize_marks`
  `inline_mark.rs:192`, `marks_from_json`, `block.rs:927` all lack content),
  so the corrupt row detonates on every render of that page → page permanently
  un-openable. Same inconsistent content+marks state as the 2026-07-19 undo
  row, now weaponized by the GPUI assert.
source_line: 1041
---

## Bug

Journal-page click SIGABRT (user report, desktop): `scalar_range_to_bytes:
0..11 exceeds text length 10` at `rich_text_runs.rs:169` — a link
`MarkSpan.end` outlives its block content. Producers:
`convert_block_to_page` (`operation_engine.rs:656-676`) writes a full-span
link mark via a DECOUPLED marks-only `set_field` (content untouched); later
content-only trailing trim (org writeback `models.rs:523/:904`, SQL
`trimmed_content` `sql_operation_provider.rs:332`) shortens content N→N-1
while `end=N` persists; `split_block` produces the same OOB state (row 237).
NO read boundary clamps marks vs content (`canonicalize_marks`
`inline_mark.rs:192`, `marks_from_json`, `block.rs:927` all lack content),
so the corrupt row detonates on every render of that page → page permanently
un-openable. Same inconsistent content+marks state as the 2026-07-19 undo
row, now weaponized by the GPUI assert.

## Missing piece

ORACLE: desync state is generatable (convert mirrored
`reference_state.rs:1529`, trim modeled `types.rs:139`) but no invariant
asserts `∀mark: end ≤ content.chars().count()` over RAW projection state —
the model normalizer re-derives marks from trimmed content, masking it
(tests-mirror-the-bug). ENV: the abort path
(`scalar_range_to_bytes`/`build_content_segments`) is GPUI-only, never runs
headless. Remedy: mark-bounds invariant red-first; clamp+disclose at
`block.rs:927` read boundary; renderer clamp+error! instead of assert; fix
producers (convert atomicity, split remap, trim-in-lockstep).

## Remedy

FIXED 2026-07-20 (4 layers: inv-mark-bounds-within-content raw-state
invariant + renderer clamp/error + read-boundary canonicalize_marks_against
heal at block.rs deserializer + convert producer clamp; corrupt persisted
rows now HEAL on read with disclosed warn; verifier CONFIRMED;
fault-injection keystone transition = open coverage follow-up). **RE-LAYERED
2026-08-16 (D27.a, lane-mark-policy) — the 2026-07-20 layering rested on a
WRONG BOUNDARY MAP.** The "read-boundary heal at the block.rs deserializer"
is on the TYPED `Block::from_row` path, which no frontend uses: both
renderers read a projection `DataRow` (`DataRow = HashMap<String, Value>`,
widget_spec.rs:14) through `marks_of` (link_segments.rs:127), which parsed
with `marks_from_json` and never saw `content`, so it could not heal
range-vs-content at all. The renderer clamp was therefore not a redundant
"net" over a healed boundary — it was the ONLY heal on the path that
actually paints, and deleting it as dead weight (the original D27.a scope)
would have re-opened the crash. Corrected shape: `marks_of` becomes the
projection-path read boundary and calls `canonicalize_marks_against(content,
marks, block_id)`; ONE heal per read boundary, TWO boundaries because there
are two consumers. Consequently `clamp_marks_to` + its 2 tests are deleted
as dead for the RIGHT reason, and the 4th layer (convert producer clamp,
operation_engine.rs:997) is deleted as a provable no-op — it clamped a span
against the very string the span was built from (same no-op in
merge_blocks_pbt.rs:418). Inversion (`start > end`) is now unrepresentable
rather than clamped: `MarkSpan`'s hand-written `Deserialize` rejects it,
closing the derived-impl hole the old clamp comment documented. Because that
makes `marks_from_json` fail on inverted spans, the three
`.expect("blocks.marks must be valid JSON")` sites would have turned an
MCP-written span into an app panic; `marks_of` now degrades visibly instead
(ERROR naming block id + parse error, block renders PLAIN TEXT), and gpui
`builders/text.rs:51` was folded onto the same shared read instead of
keeping a second, stricter copy. STILL OPEN, unchanged: the fault-injection
keystone transition (task #20), which also needs an inversion arm —
`inv-mark-bounds-within-content` checks only `end > len`, never `start >
end`, so no keystone can see an inverted span.
