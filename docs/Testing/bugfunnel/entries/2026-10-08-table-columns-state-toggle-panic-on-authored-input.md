---
id: 2026-10-08-table-columns-state-toggle-panic-on-authored-input
date: 2026-10-08
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `table(#{columns: …})`, `columns()` and `state_toggle` panicked on authored arguments (a bad
  column spec, a bad `min_width`, no children, an unknown `appearance`/`binding`, a non-bool value
  under `binding: "bool"`), so an authored render could abort a release build.
---

## Bug
Found by the adversarial verifier of the boot-always lane (`lane-logs/c1-verify.md`, defect 1):
the panic sites at `crates/holon-frontend/src/shadow_builders/table.rs` (column spec, width,
`min_width`), `columns.rs` (no positional children and no `item_template:`) and
`state_toggle.rs` (appearance, binding, bool value, also on a live row update). Red: every case
of `crates/holon-frontend/tests/authored_builder_input_draws_error.rs` panicked at those lines.

## Root cause
`panic!`/`assert!`/`unwrap_or_else(panic)` on authored render arguments; the release profile is
`panic = "abort"`.

## Missing piece
No test interprets a malformed authored render expression; the keystone only renders the shipped
profiles, whose arguments are valid.

## Remedy
The parse helpers return `Result` and each builder returns `ViewModel::error` naming the builder,
the argument and the problem. A `binding: "bool"` toggle whose row turns non-bool switches its
node to an `error` node and back when the value fits again. Pinned by
`crates/holon-frontend/tests/authored_builder_input_draws_error.rs`.
