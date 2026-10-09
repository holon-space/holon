---
id: 2026-10-09-column-gap-non-number-silently-zero
date: 2026-10-09
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  `column(#{gap: "wide"}, ..)` and `section_stack(#{gap: "wide"}, ..)` drew a 0 px gap with no
  error: the raw builders read `gap` with `get_f64(..).unwrap_or(0.0)`.
---

## Bug
Found by the adversarial verifier of the c1-honour lane (`lane-logs/c1h-verify.md`, Attack 5) by
code audit; `section_stack` by the follow-up audit of the same file.

## Root cause
`crates/holon-frontend/src/shadow_builders/mod.rs`, the `column` and `section_stack` closures,
while `min_height` two lines below already used `get_f64_strict`.

## Missing piece
`authored_builder_input_draws_error.rs` had no case for the raw container builders' `gap`.

## Remedy
Both read `gap` with `get_f64_strict` and draw an error node naming the builder for a non-number.
Tests `column_gap_not_a_number`, `section_stack_gap_not_a_number`. Red:
`lane-logs/c1h2/red-1.log`, `lane-logs/c1h2/teeth-gap-recipe-red.log`.
