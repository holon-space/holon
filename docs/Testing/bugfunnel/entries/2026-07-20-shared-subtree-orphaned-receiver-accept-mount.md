---
id: 2026-07-20-shared-subtree-orphaned-receiver-accept-mount
date: 2026-07-20
gap: ENVIRONMENT
secondary: PERCEPTION
status: FIXED
summary: >-
  Shared subtree orphaned on receiver — accept mount lands at
  sentinel:no_parent (not requested parent), untagged/empty → invisible in UI
  though in SQL
source_line: 1046
---

## Bug

Shared subtree orphaned on receiver — accept mount lands at
sentinel:no_parent (not requested parent), untagged/empty → invisible in UI
though in SQL

## Missing piece

no test drives accept_shared_subtree + asserts mount reachable under
requested parent

## Remedy

FIXED+WOVEN 2026-07-21 — accept mount lands under a well-known 'Shared with
me' recipient root (ADR 0028 H7) instead of sentinel:no_parent; idempotent
root ensure + SQL projection BEFORE mount. Pinned by
accept_orphan_target_lands_under_shared_with_me_root +
two_orphan_accepts_reuse_one_shared_with_me_root. Verifier CONFIRMED
