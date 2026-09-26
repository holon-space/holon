---
id: 2026-10-02-sql-property-write-splits-a-dotted-key
date: 2026-10-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  On the SQL write authority, setting a property whose key holds a `.`
  (`x.y`) stored the nested object `{"x": {"y": …}}`, so the row read back
  without the property; Loro stored the key as written.
---

## Bug
Found by the dense_patch engine generated test on the SQL authority
(`a_generated_dense_edit_on_the_sql_authority_reads_back_exactly_or_is_refused`),
which first ran in lane decision Inc 6 round 7b. Minimal case: `SetProperty("x.y",
"65")` on a seeded row; the store held `"x": {"y": "65"}`.

## Root cause
`json_path` in `crates/holon/src/core/properties_bag_write.rs` built the path
`'$.x.y'`, which SQLite/Turso JSON reads as two nested steps.

## Missing piece
No test wrote a property key with a `.` on the SQL authority; the engine
property tests ran on Loro only.

## Remedy
`json_path` quotes the member (`'$."x.y"'`) and refuses a key holding `"` or
`\`, which a quoted member cannot name. Pinned by
`a_dotted_property_key_reads_back_as_one_key` (both legs); teeth:
`lane-logs/inc6r7b-teeth.log` (unquoted path restored: `[sql]` red).
