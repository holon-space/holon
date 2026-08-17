---
id: 2026-07-16-advice-rule-source-block-renders-literal
date: 2026-07-16
gap: PERCEPTION
secondary: COVERAGE
status: OPEN
summary: >-
  Advice rule source block renders as literal text "[no result]" in the page
  tree (where journals' holon_rule gets a proper "Automation rule" card) —
  placeholder leaks as content for `holon_advice_rule_yaml` blocks
source_line: 833
---

## Bug

Advice rule source block renders as literal text "[no result]" in the page
tree (where journals' holon_rule gets a proper "Automation rule" card) —
placeholder leaks as content for `holon_advice_rule_yaml` blocks

## Missing piece

no render assertion for advice-rule source blocks

## Remedy

ROOT-CAUSED 2026-07-17 (SEPARATE cause from the feed cluster above —
profile-variant coverage, not focus-render routing).
`assets/default/types/block_profile.yaml`: the `source_language ==
"holon_advice_rule_yaml"` block matches NEITHER `is_holon_source`
(enumerates only `holon_prql`/`holon_gql`/`holon_sql`/`render`) NOR
`is_rule_head` (only `holon_rule`/`action`), so it never reaches the
`rule_card` variant the way `holon_rule` does; it falls through to a
query/source variant and shows the empty-result placeholder. Fix (deferred,
own decision): either add `holon_advice_rule_yaml` to the rule-card routing
(render as an advice-rule card) or give it a dedicated variant — NOT part of
the journal-feed cluster. OPEN
