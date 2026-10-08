---
id: 2026-10-08-table-columns-state-toggle-panic-on-authored-input
date: 2026-10-08
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `table(#{columns: …})`, `columns()` and `state_toggle` panicked on authored arguments (a bad
  column spec, a bad `min_width`, no children, an unknown `appearance`/`binding`, a non-bool value
  under `binding: "bool"`), so an authored render could abort a release build. The same class:
  `text(#{ellipsis: …})` with a word other than "start"/"end", `view_mode_switcher` without
  `entity_uri:`, `bottom_dock` without exactly two slots, and `list` with a mistyped
  `horizontal:`/`wrap:`/`gap:` (also on the GPUI view-mode switch).
---

## Bug
Found by the adversarial verifier of the boot-always lane (`lane-logs/c1-verify.md`, defect 1):
the panic sites at `crates/holon-frontend/src/shadow_builders/table.rs` (column spec, width,
`min_width`), `columns.rs` (no positional children and no `item_template:`) and
`state_toggle.rs` (appearance, binding, bool value, also on a live row update). Red: every case
of `crates/holon-frontend/tests/authored_builder_input_draws_error.rs` panicked at those lines.
The fix-round verifier (`lane-logs/c1-fix-verify.md`, D1 and D2) then aborted the process with
`text("x", #{ellipsis: "middle"})` (`text.rs:55`) and `view_mode_switcher(#{modes: …})`
(`view_mode_switcher.rs:10`). The property test below found two more on its first run:
`bottom_dock()` (`bottom_dock.rs:20`) and `list(#{wrap: ""})` (`ItemFlow::parse`,
`reactive_view_model.rs:161`).

## Root cause
`panic!`/`assert!`/`unwrap_or_else(panic)` on authored render arguments; the release profile is
`panic = "abort"`.

## Missing piece
No test interprets a malformed authored render expression; the keystone only renders the shipped
profiles, whose arguments are valid. Per-instance example tests fixed the builders a reviewer
named and left the others.

## Remedy
The parse helpers return `Result` and each builder returns `ViewModel::error` naming the builder,
the argument and the problem. A `binding: "bool"` toggle whose row turns non-bool switches its
node to an `error` node and back when the value fits again. Pinned by
`crates/holon-frontend/tests/authored_builder_input_draws_error.rs`.
The class is pinned by `crates/holon-frontend/tests/every_builder_survives_authored_args.rs`:
for every builder the shadow interpreter registers, a property test builds calls with each
declared or read argument missing, mistyped, empty, an unknown word or an empty list, and fails
on any panic, including one in a task the builder spawned.
