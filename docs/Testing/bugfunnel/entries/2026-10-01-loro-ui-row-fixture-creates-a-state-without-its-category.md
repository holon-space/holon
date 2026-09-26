---
id: 2026-10-01-loro-ui-row-fixture-creates-a-state-without-its-category
date: 2026-10-01
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  `loro_ui_row_matches_sql_row` failed after the Loro create guard: its fixture
  calls the provider create with a `task_state` and no `task_state_category`,
  which only the engine derives.
---

## Bug
Found by the Inc 6 rebase verifier (`lane-logs/inc6rb2v-verify.md`, item 7),
deterministic, also alone. The test was not in the lane's gate set.

## Root cause
`LoroBlockOperations::create` (`crates/holon-loro/src/loro_block_operations.rs`)
refuses a `task_state` without its `task_state_category`: the engine derives
the category from the document's `#+TODO:` ring before a provider's create.
The fixture calls the provider directly, below the engine, and passed only the
keyword.

## Missing piece
No product defect: the fixture did not pass the params the engine passes.
holon-app was not in the lane's gate set.

## Remedy
The fixture passes the category the engine derives for `TODO` with no ring
(`TaskState::from_keyword`). Both providers get the same params, so the
parity check is unchanged. holon-app is in the lane's gate set.
