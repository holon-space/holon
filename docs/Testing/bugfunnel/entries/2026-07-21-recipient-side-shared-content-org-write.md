---
id: 2026-07-21-recipient-side-shared-content-org-write
date: 2026-07-21
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  Recipient-side shared-content org write-back stuck — accepted share content
  shows persistent "org file pending" toasts; shared blocks apparently never
  reach a materialized org file on the recipient
source_line: 1067
---

## Bug

Recipient-side shared-content org write-back stuck — accepted share content
shows persistent "org file pending" toasts; shared blocks apparently never
reach a materialized org file on the recipient

## Missing piece

keystone has no recipient-side share→org-writeback rung; decide intended
semantics (should shared mounts write back to recipient org files at all?)
then lock

## Remedy

OPEN (dogfood-round4; needs semantics ruling)
