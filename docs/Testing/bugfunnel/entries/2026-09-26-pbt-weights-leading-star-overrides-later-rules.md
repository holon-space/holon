---
id: 2026-09-26-pbt-weights-leading-star-overrides-later-rules
date: 2026-09-26
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `HOLON_PBT_WEIGHTS='*:0,DenseProjectionEdit:100'` zeroed every transition
  including the named one, because the first matching pattern won, so the
  keystone panicked with "no transition applicable".
---

## Bug
A verifier probe of `just keystone-mcp 8720 16 '*:0,DenseProjectionEdit:100'`
panicked in `aggregate_transitions` with `declare_e2e_transitions!: no
transition applicable`. The companion spec `*:1,DenseProjectionEdit:100`
silently ran every transition at weight 1: the boost never applied.

## Root cause
`variant_weight_multiplier`
(`crates/holon-integration-tests/src/pbt/transition_dispatch.rs`) returned the
multiplier of the FIRST matching pattern. A leading `*` matches every variant,
so no later rule could take effect. The hypothesis that
`DenseProjectionEdit`'s precondition needs setup transitions that `*:0`
forbids does not hold: its gate is `app_started` plus a Main focus-root page
whose children are all non-seed text blocks, and the booted fixture already
satisfies it. With the parser fixed, a 16-case run drawing only
`DenseProjectionEdit` is green (`lane-logs/kmb-green-star0-16.log`).
Malformed entries were also skipped with an `eprintln!`, so a typo silently
became "no override".

## Missing piece
No test of the weight-spec parser. Two documented specs, the `keystone-mcp`
recipe's own example (`justfile`) and `*:0,TypeChars:1,...` in
`docs/Archive/TESTING_PHASE1_SESSION_SUMMARY.md`, never worked.

## Remedy
The last matching rule now wins, and a malformed entry panics. Pinned by
`weight_spec_tests::a_later_rule_overrides_a_leading_star` (red:
`lane-logs/kmb-red-weights.log`, green: `lane-logs/kmb-green-weights.log`).
