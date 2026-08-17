---
id: 2026-07-21-boot-stack-overflow-crash-unrecoverable-abort
date: 2026-07-21
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Boot stack-overflow crash (unrecoverable abort — `fatal runtime error: stack
  overflow, aborting`, health never comes up) when an org doc `#+ID:` equals a
  heading `:ID:` — the heading becomes its own ancestor (self-parent cycle)
  and the recursive tree/descendants projection recurses without a cycle
  guard. A SINGLE malformed org file kills the whole app on boot with no error
  to the user — a fail-loud violation. Minimal repro confirmed (one file,
  `#+ID: cyc-id` + `* Cyc` with `:ID: cyc-id`).
source_line: 1088
---

## Bug

Boot stack-overflow crash (unrecoverable abort — `fatal runtime error: stack
overflow, aborting`, health never comes up) when an org doc `#+ID:` equals a
heading `:ID:` — the heading becomes its own ancestor (self-parent cycle)
and the recursive tree/descendants projection recurses without a cycle
guard. A SINGLE malformed org file kills the whole app on boot with no error
to the user — a fail-loud violation. Minimal repro confirmed (one file,
`#+ID: cyc-id` + `* Cyc` with `:ID: cyc-id`).

## Missing piece

no duplicate-id rejection at the ingest boundary + no acyclicity/depth guard
in the recursive tree projection; the generator never produces a parent
cycle and no invariant asserts the parent graph is acyclic before recursion

## Remedy

FIXED 2026-07-21 night (woven, verifier CONFIRMED incl. 0-of-66 real-vault
rejection sweep). (1) Parse boundary: holon-org-format parser.rs
reject_id_cycles fails loud on doc-#+ID==heading-:ID self-parent, duplicate
block id, or id==parent_id — names file + colliding id; routes to the
existing loud FileSyncController quarantine on BOTH watch and boot paths
(file skipped + degraded banner, other files keep syncing, app boots; boot
scan proven skip-not-abort). (2) Render backstop: holon-frontend
mutable_tree.rs walk_dfs_into visited-set guard (tracing::error + back-edge
prune; acyclic input byte-identical) — reproduces the exact SIGABRT
red->green. OutlineTree::walk_level cycle-safe by construction, untouched.
Keystone acyclicity invariant + id-collision generator case DESIGNED,
deferred; regression pinned by
parse_rejects_doc_id_equal_to_heading_id_self_parent +
walk_dfs_into_survives_self_parent_cycle.
