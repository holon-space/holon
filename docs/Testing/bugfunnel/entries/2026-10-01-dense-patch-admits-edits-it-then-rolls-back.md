---
id: 2026-10-01-dense-patch-admits-edits-it-then-rolls-back
date: 2026-10-01
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  dense_patch admitted a new drawer key `:\_x:`, the removal of an authored
  `:_secret:` or `:org_properties:` line, and a `#+CAPTION:` line between
  blank lines; the writes landed and verify-after-apply took them back.
---

## Bug
Found by the Inc 6 rebase verifier (`lane-logs/inc6rb2v-verify.md`, D2), lane
decision Inc 6. The contract is: an edit applies exactly, or it is refused by
row name before any write. These edits were written and then rolled back.

## Root cause
- The planner used authored drawer keys as bag keys
  (`PatchOp::SetProperty.key: String`), not `AuthoredKey::property()`, the
  backslash escape of `_`-prefixed and reserved keys.
- A `#+CAPTION:` line after a blank line, in a row with no drawer: the parser
  reads the blank line after a bare headline into the keyword line, so the row
  read back with other keyword lines than the text had.

## Missing piece
The test judge accepted a refusal "op(s) landed, but ... does not read back"
as a correct refusal, so a rollback looked like a refusal. The generator draws
no authored `_`-prefixed drawer key.

## Remedy
`PatchOp::SetProperty.key` is an `AuthoredKey`; the removal and the `\_x` key
apply exactly. `refuse_inexact` compares the row's keyword lines with the
dense render and with the org file render (`DenseBlock::file_keyword_lines`)
and refuses a difference by row name. The judge treats a rollback as a broken
contract. Pinned by `a_text_org_writes_otherwise_is_refused_before_any_write`.
