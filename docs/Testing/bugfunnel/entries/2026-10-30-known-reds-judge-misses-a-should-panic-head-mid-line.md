---
id: 2026-10-30-known-reds-judge-misses-a-should-panic-head-mid-line
date: 2026-10-30
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  The known-reds judge called the expected panic of a passing should_panic
  test novel when libtest printed the test's `- should panic ... ` head after
  other stdout on the same line — a test-tooling false failure, not a product
  defect.
---

## Bug

A green one-case replay (`HOLON_HAND_AUTHORED_CASE=<case> just hand-authored`,
exit 0) was judged FAIL by `scripts/keystone-known-reds.sh`: one novel panic,
`hand_authored.rs:61:13` "initial_state carries an INVALID wiring manifest".
Found by the lane agent while collecting replay evidence.

## Root cause

The panic belongs to `hand_authored_regressions::invalid_pinned_wiring_is_loud`,
a `#[should_panic]` test that passed. libtest wrote its head
`test hand_authored_regressions::invalid_pinned_wiring_is_loud - should panic ... `
in the middle of the replay's `[hand-authored regression] running case "…"`
line, and its `ok` on the next line. The judge matched the head only at the
start of a line (`/^ *test .* - should panic \.\.\. /`), so it never marked
that panic as expected.

## Missing piece

No fixture log put a should_panic head after other text on its line; the
split-verdict fixtures only glued text after the verdict or moved the verdict
to a later line.

## Remedy

The judge matches the head after any text that ends in a space or `(`.
Fixture cases `should-panic-head-mid-line` (verdict `ok` on the next line:
pass) and `should-panic-head-mid-line-failed` (verdict `FAILED`: the panic
stays novel) pin it; the first is red with the old match.
