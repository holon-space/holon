---
id: 2026-10-02-advice-suppressed-misclassified-lost-on-rebuild
date: 2026-10-02
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  The rebuild classification lists the `advice_suppressed` junction as Lost ("dismissed
  advice"), but the org files and the Loro store restore it, so a rebuild discloses a
  loss that does not happen.
---

## Bug

A read-only architecture study (2026-10-02, D26.b, K1 writer table) found that the table class
and the code disagree. Nothing ran; this comes from reading the code.

## Root cause

`crates/holon-turso/src/table_classes.rs:92` puts `("advice_suppressed", "dismissed advice")`
in `LOST`. Every `Lost` table starts empty after a rebuild, and the disclosure names it with the
rows it held (`:1-5`). But `advice_suppressed` is an edge field in the same way as
`requires`/`tags`:
- `EdgeField::AdviceSuppressed` (`crates/holon-api/src/edge_field.rs:41,50`) is in
  `EdgeField::ALL`, which the Loro→SQL projection iterates to fill the junctions
  (`crates/holon-loro/src/loro_sync_controller.rs:2461-2463`, `:2556-2559`).
- The org format parses and renders it as a drawer property
  (`crates/holon-org-format/src/parser.rs:2353`, `models.rs:1557-1563`), and a round-trip test
  covers it (`parser.rs:3107`).
- Loro stores it in node meta (`crates/holon-loro/src/loro_backend.rs:574`, `:4320-4340`).

So the code is right, and the classification is wrong. The junction belongs in `REBUILT` with
`block_tags`, `block_requires` and `block_contributes_to` (`table_classes.rs:24-29`). Effect:
after a rebuild, the user is told that dismissed advice is lost, but it comes back from the
files. The disclosure is false, and it trains the user to ignore real loss banners.

## Missing piece

`crates/holon-app/tests/database_rebuild_disclosure.rs::every_table_is_classified_as_rebuilt_or_lost`
(`:296`) checks only that each table has a class. No test checks that the class is true: a
`Lost` table must actually come back empty after a rebuild, and a `Rebuilt` table must come back
full.

## Remedy

OPEN. Rung that closes the gap: extend `database_rebuild_disclosure.rs` so that the probe vault
holds one block with a dismissed advice lesson. After the rebuild, assert that each `LOST`
table is empty and that each `REBUILT` junction holds its rows again. It goes red today because
`advice_suppressed` is classed `Lost` but is refilled. Fix: move `advice_suppressed` from
`LOST` to `REBUILT`.
