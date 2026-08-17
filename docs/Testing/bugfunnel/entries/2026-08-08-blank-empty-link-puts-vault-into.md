---
id: 2026-08-08-blank-empty-link-puts-vault-into
date: 2026-08-08
gap: COVERAGE
secondary: PERCEPTION
status: OPEN
summary: >-
  A blank `[[ ]]` or empty `[[]]` link puts the vault into a permanently
  ERROR-logging state
source_line: 764
---

## Bug

(dogfood-explorer gate pass) **A blank `[[ ]]` or empty `[[]]` link puts the
vault into a permanently ERROR-logging state**: both survive ingest
byte-for-byte and do NOT vanish, and no write-back loop was observed, but
the renderer classifies them `rung="Unrepresentable"` and logs `org render
is DEGRADED … NO emission of this block settles; write-back may loop on it`
at ERROR on every render, including a clean cold boot with no user action.

## Root cause

dogfood-explorer gate pass — **a blank `[[ ]]` or empty `[[]]` link puts the
vault into a permanently ERROR-logging state**. Both survive ingest
byte-for-byte and do NOT vanish (the behaviour the padded-link work asked
for), and no write-back loop was observed over 10 sha samples plus a forced
re-ingest — but the org renderer classifies them `rung="Unrepresentable"`
and says so at ERROR on every render, including a clean cold boot with no
user action: `org render is DEGRADED for this block: NO emission of this
block settles; write-back may loop on it block="block:ingest-blank-link"`.
Honest disclosure, wrong consequence: ordinary user-typable content reds
`inv-no-observed-errors` forever and buries real errors. COVERAGE — no
keystone draw generates a link with an empty or whitespace-only target, and
`LinkTarget` has no honest bucket for one (ORG_SYNTAX.md §"whitespace-only
segment" names the same hole from the classifier side). Evidence:
`docs/Testing/fixture-logs-2026-08-08/dogfood-blank-link-unrepresentable-and-misc.txt`
§1)

## Missing piece

No keystone draw generates a link with an empty or whitespace-only target,
and `LinkTarget` has no honest bucket for one — ORG_SYNTAX.md names the same
hole from the classifier side.

## Remedy

**OPEN — reported, not fixed.** Honest disclosure, wrong consequence:
ordinary user-typable content reds `inv-no-observed-errors` forever.
Evidence
`docs/Testing/fixture-logs-2026-08-08/dogfood-blank-link-unrepresentable-and-misc.txt`
§1.
