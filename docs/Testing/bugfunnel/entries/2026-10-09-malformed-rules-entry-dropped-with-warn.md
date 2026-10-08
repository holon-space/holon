---
id: 2026-10-09-malformed-rules-entry-dropped-with-warn
date: 2026-10-09
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A malformed `rules:` entry (or a `rules:` that is not an array) on a collection was dropped with
  a `tracing::warn`, so the rows drew the default way with nothing said on screen.
---

## Bug
Found by the adversarial verifier of C1 fix 3 (`lane-logs/c1-fix3-verify.md`, D4):
`row_pipeline::parse_rules_arg` skipped every entry that did not deserialize as a
`RuleSpec` and answered an empty list for a non-array.

## Root cause
The parse was written fail-soft ("rules are advisory metadata"), a fail-loud
violation: the author cannot see a log line.

## Missing piece
The property test generated `rules:` only as flat values, never nested rule
maps, and judged only "no panic".

## Remedy
`parse_rules_arg` returns `Result`; every collection builder (and the
`Collection` param of `widget_builder!`) draws an error node naming itself.
Property `a_malformed_rules_entry_draws_an_error_node_naming_the_builder` in
`crates/holon-frontend/tests/every_builder_survives_authored_args.rs`; red
before the fix in `lane-logs/c1r4/red-property.log`.
