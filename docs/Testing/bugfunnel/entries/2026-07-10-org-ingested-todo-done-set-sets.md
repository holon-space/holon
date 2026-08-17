---
id: 2026-07-10-org-ingested-todo-done-set-sets
date: 2026-07-10
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Org-ingested TODO/DONE set `task_state` but not `task_state_category` (UI
  `cycle_task_state` sets both) → category-filtering queries never see
  file-originated tasks
source_line: 889
---

## Bug

Org-ingested TODO/DONE set `task_state` but not `task_state_category` (UI
`cycle_task_state` sets both) → category-filtering queries never see
file-originated tasks

## Missing piece

ingest-vs-op parity for task properties never asserted

## Remedy

FIXED (stream 2026-07-10): `build_block_params` wrote `task_state` but never
`task_state_category` although the parser already derives it
(`TaskState::from_keyword_with_done_list` off `#+TODO:` config) — now
mirrored into ingest params. Test
`ingested_todo_and_done_blocks_carry_task_state_category`
