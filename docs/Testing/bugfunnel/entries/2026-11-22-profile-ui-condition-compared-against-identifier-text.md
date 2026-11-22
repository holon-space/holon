---
id: 2026-11-22-profile-ui-condition-compared-against-identifier-text
date: 2026-11-22
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A profile variant condition such as `available_width_px > viewport_width_px` was accepted and became a UI
  predicate comparing a number with the text "viewport_width_px", so the variant never matched and nothing said why.
---

## Bug

Found by the per-crate code review of holon-profiles, not by a test. `parse_entity_profile` accepted a UI-side
conjunct whose right side was an identifier or an expression (`100 + 20`) and stored the right side as a string value.
The frontend then compared a number with that string; the variant stayed inactive, a row rendered with the wrong
variant, and no error or disclosure named the condition.

## Root cause

- `parse_literal_value` ended in `Value::String(s.to_string())` for any text it did not recognise
  (crates/holon-profiles/src/lib.rs, the fallback arm), so every unparsed right side became a string literal.
- The classifier `has_non_ui_references` started an identifier inside a number (`0x100` gave `x100`, `1e3` gave `e3`)
  and skipped only `"` and `'` literals, so a conjunct was put on the data or UI side by fragments of literals.
  The classifier is only safe once the fallback is gone: `viewport_width_px > 0x100` otherwise moves to the UI side as
  the string `0x100`.

## Missing piece

No test fed the splitter a condition whose right side is not a literal, or a literal form with letters in it (hex,
exponent, backtick, raw string). The existing split tests used only decimal numbers and quoted strings, and nothing
asserted that a UI predicate carries a literal.

## Remedy

- `parse_literal_value` accepts a quoted string, `true`, `false`, `()`/`null` and decimal numbers, and refuses
  everything else with an error that quotes the condition and the right side.
- `has_non_ui_references` and `find_comparison_operator` skip every literal form through the same scanner as
  `top_level_conjuncts` and read a digit-led run as one number.
- crates/holon-profiles/tests/ui_condition_split.rs asserts, at `parse_entity_profile`, that each conjunct lands on the
  side its identifiers name with the author's literal, or the profile is refused quoting the conjunct.
