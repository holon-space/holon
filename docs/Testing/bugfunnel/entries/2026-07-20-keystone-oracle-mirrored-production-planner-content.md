---
id: 2026-07-20-keystone-oracle-mirrored-production-planner-content
date: 2026-07-20
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  Keystone `block_to_page` ORACLE mirrored the production planner's
  content-as-path assumption:
  `pbt/transitions/block_to_page.rs::plan_new_page` called
  `PageId::for_path("<ancestor>/<content>")` and `.ok()?`-swallowed the
  failure, so model and prod AGREED on the wrong answer — a `/` in block
  content made the origin silently non-candidate on BOTH sides. Model-prod
  agreement on the wrong answer made the content-as-path defect (row above)
  invisible to random generation.
source_line: 1026
---

## Bug

Keystone `block_to_page` ORACLE mirrored the production planner's
content-as-path assumption:
`pbt/transitions/block_to_page.rs::plan_new_page` called
`PageId::for_path("<ancestor>/<content>")` and `.ok()?`-swallowed the
failure, so model and prod AGREED on the wrong answer — a `/` in block
content made the origin silently non-candidate on BOTH sides. Model-prod
agreement on the wrong answer made the content-as-path defect (row above)
invisible to random generation.

## Missing piece

the reference model duplicated prod's (buggy) path-splitting AND the content
generator never emits `/`, so the composed keystone structurally cannot draw
a `/`-content block→page candidate.

## Remedy

**FIXED THIS LANE (model-first)**: ref model now uses
`PageId::for_page_under` (leaf = single segment), matching the fixed prod
planner; both sides agree on the RIGHT answer and now ACCEPT `/`-content
candidates. Deterministic red→green lives in `convert_block_to_page_e2e`.
Residual COVERAGE gap (global content alphabet never yields `/`, so the
composed keystone still can't spontaneously draw such a case) is documented
here, not forced — a `/` in the shared alphabet risks destabilizing
unrelated org/link invariants
