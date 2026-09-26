---
id: 2026-09-30-sql-authority-accepts-an-empty-question-the-loro-authority-refuses
date: 2026-09-30
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  A dense_patch that turns `* TODO Plan` into `* ?` is refused at its content
  write under the Loro block authority, but under the SQL authority both writes
  land and the block reads back with no task state.
---

## Bug
Found by the Inc 6 round-12 lane while writing the partial-apply test for the
SQL authority (`lane-logs/inc6r12-item4-probe.log`, `[zz12 sql]`). The same
edit gives `ROLLED BACK` (empty question refused) in the full_headless session
and `APPLIED INEXACTLY: all 2 op(s) ARE in the store` in an `{Org, Turso}`
session: `row {#0}` reads back with `task_state: None`, not `?`.

## Root cause
Not yet traced. The two authorities treat a content write that leaves a `?`
block with no text differently: one refuses it, the other lands it and the
`?` state is gone on read-back.

## Missing piece
`dense_patch_engine_exact`'s generated property boots only the full_headless
session (Loro authority), so no generated dense edit runs against the SQL
authority.

## Remedy
Open. The post-apply read-back discloses it by row name, so nothing is lost
silently.
